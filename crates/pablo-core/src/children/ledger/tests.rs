use super::*;
#[test]
fn atomic_child_registration_and_active_capacity_have_one_winner_without_partial_records() {
    let root = AgentRef::root("root".into(), "session".into());
    let ledger = RootLedger::new(&root, RunLimits::default()).unwrap();
    let children = [
        root.temporary_child().unwrap(),
        root.temporary_child().unwrap(),
    ];
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let handles = children
            .iter()
            .map(|child| {
                let ledger = &ledger;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    ledger.admit_child(
                        child,
                        RunLimits::default(),
                        resources::Resources {
                            active_children: 1,
                            ..Default::default()
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results.iter().position(Result::is_err).unwrap();
    assert!(ledger.agent(children[loser].agent_id()).is_none());
    assert_eq!(ledger.resources().active_children, 1);
    assert_eq!(ledger.total().model_calls, 0);
    drop(results);
    assert_eq!(ledger.resources().active_children, 0);
    let lease = ledger
        .admit_child(
            &children[loser],
            RunLimits::default(),
            resources::Resources {
                active_children: 1,
                ..Default::default()
            },
        )
        .unwrap();
    drop(lease);
    let closed_child = root.temporary_child().unwrap();
    ledger.close_admission();
    assert!(matches!(
        ledger.admit_child(
            &closed_child,
            RunLimits::default(),
            resources::Resources {
                active_children: 1,
                ..Default::default()
            }
        ),
        Err(AdmissionError::Closed)
    ));
    assert!(ledger.agent(closed_child.agent_id()).is_none());
}
fn setup(calls: u32, tokens: u64, cost: u64) -> (AgentRef, AgentRef, RootLedger) {
    let root = AgentRef::root("run".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let limits = RunLimits {
        max_model_calls: Some(calls),
        max_tool_calls: Some(1),
        max_total_tokens: Some(tokens),
        max_cost_microusd: Some(cost),
        ..RunLimits::default()
    };
    let ledger = RootLedger::new(&root, limits.clone()).unwrap();
    ledger.register_child(&child, limits).unwrap();
    (root, child, ledger)
}
#[test]
fn racing_root_and_child_cannot_both_spend_the_last_allowance() {
    let (root, child, ledger) = setup(1, 100, 10);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles = [root.agent_id().to_owned(), child.agent_id().to_owned()].map(|id| {
            let ledger = ledger.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                let admitted = ledger.reserve_model(
                    &id,
                    AccountingBounds {
                        tokens: Some(100),
                        cost_microusd: Some(10),
                    },
                );
                (id, admitted)
            })
        });
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|(_, r)| r.is_ok()).count(), 1);
    assert_eq!(ledger.total().model_calls, 1);
    assert_eq!(ledger.total().charged_tokens, Some(100));
    for (id, result) in results {
        match result {
            Ok(ticket) => {
                ticket
                    .settle(
                        &Usage {
                            input_tokens: Some(5),
                            output_tokens: Some(7),
                            cache_read_input_tokens: Some(0),
                            cache_write_input_tokens: Some(0),
                        },
                        Some(3),
                        false,
                    )
                    .unwrap();
                assert_eq!(ledger.agent(&id).unwrap().model_calls, 1);
            }
            Err(AdmissionError::Outcome(RunOutcome::LimitExceeded {
                limit: LimitKind::ModelCalls,
            })) => assert_eq!(ledger.agent(&id).unwrap().model_calls, 0),
            _ => panic!("unexpected admission result"),
        }
    }
    assert_eq!(ledger.total().charged_tokens, Some(12));
    assert_eq!(ledger.total().charged_cost_microusd, Some(3));
    assert_eq!(ledger.total().usage.input_tokens, Some(5));
    assert_eq!(ledger.total().usage.output_tokens, Some(7));
}
#[test]
fn failed_multi_resource_admission_changes_no_counter_and_uncertainty_is_retained() {
    let (root, child, ledger) = setup(4, 200, 10);
    let bounds = AccountingBounds {
        tokens: Some(100),
        cost_microusd: Some(10),
    };
    let first = ledger.reserve_model(root.agent_id(), bounds).unwrap();
    let before = ledger.total();
    assert!(matches!(
        ledger.reserve_model(child.agent_id(), bounds),
        Err(AdmissionError::Outcome(RunOutcome::LimitExceeded {
            limit: LimitKind::Cost
        }))
    ));
    assert_eq!(ledger.total(), before);
    assert_eq!(ledger.agent(child.agent_id()).unwrap().model_calls, 0);
    drop(first); // A missing settlement cannot refund possibly delivered work.
    assert_eq!(ledger.total().charged_tokens, Some(100));
    assert_eq!(ledger.total().charged_cost_microusd, Some(10));
    assert_eq!(ledger.total().usage.input_tokens, None);
    ledger.close_admission();
    assert!(matches!(
        ledger.admit_tool(child.agent_id()),
        Err(AdmissionError::Closed)
    ));
}
#[test]
fn not_sent_refunds_liability_but_not_attempts_and_bound_violation_keeps_charge() {
    let (root, child, ledger) = setup(3, 200, 20);
    let bounds = AccountingBounds {
        tokens: Some(100),
        cost_microusd: Some(10),
    };
    ledger
        .reserve_model(root.agent_id(), bounds)
        .unwrap()
        .settle(&Usage::default(), None, true)
        .unwrap();
    assert_eq!(ledger.total().model_calls, 1);
    assert_eq!(ledger.total().charged_tokens, Some(0));
    assert_eq!(ledger.total().usage.input_tokens, Some(0));
    let result = ledger
        .reserve_model(child.agent_id(), bounds)
        .unwrap()
        .settle(
            &Usage {
                input_tokens: Some(101),
                output_tokens: Some(1),
                cache_read_input_tokens: None,
                cache_write_input_tokens: None,
            },
            Some(11),
            false,
        );
    assert!(matches!(
        result,
        Err(RunOutcome::Failed {
            code: crate::FailureCode::AccountingBoundViolated,
            ..
        })
    ));
    assert_eq!(ledger.total().charged_tokens, Some(100));
    assert_eq!(ledger.total().charged_cost_microusd, Some(10));
    assert_eq!(ledger.total().model_calls, 2);
}
#[test]
fn tool_admission_races_and_foreign_or_widened_registration_fail_closed() {
    let (root, child, ledger) = setup(2, 100, 10);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let winners = std::thread::scope(|scope| {
        let handles = [root.agent_id().to_owned(), child.agent_id().to_owned()].map(|id| {
            let l = ledger.clone();
            let b = barrier.clone();
            scope.spawn(move || {
                b.wait();
                l.admit_tool(&id).is_ok()
            })
        });
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>()
    });
    assert_eq!(winners, 1);
    assert_eq!(ledger.total().tool_calls, 1);
    let another = root.temporary_child().unwrap();
    assert!(matches!(
        ledger.register_child(&another, RunLimits::default()),
        Err(AdmissionError::InvalidCeiling)
    ));
    let foreign = AgentRef::root("foreign".into(), "foreign-session".into())
        .temporary_child()
        .unwrap();
    assert!(matches!(
        ledger.register_child(&foreign, RunLimits::default()),
        Err(AdmissionError::UnknownAgent)
    ));
    assert!(matches!(
        ledger.reserve_model("unknown", AccountingBounds::default()),
        Err(AdmissionError::UnknownAgent)
    ));
}

#[test]
fn promoted_capacity_is_bound_to_one_root_and_reductions_work_after_close() {
    use resources::Resources;
    let root = AgentRef::root("run".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let ledger = RootLedger::new(&root, RunLimits::default()).unwrap();
    let foreign = RootLedger::new(&root, RunLimits::default()).unwrap();
    let mut lease = ledger
        .admit_child(
            &child,
            RunLimits::default(),
            Resources {
                pending_children: 1,
                queued_input_bytes: 100,
                result_bytes: 100,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!lease.is_active_child(&ledger, child.agent_id()));
    lease
        .replace(Resources {
            active_children: 1,
            context_bytes: 100,
            result_bytes: 100,
            ..Default::default()
        })
        .unwrap();
    assert!(lease.is_active_child(&ledger, child.agent_id()));
    assert!(!lease.is_active_child(&foreign, child.agent_id()));
    assert!(!lease.is_active_child(&ledger, root.agent_id()));
    ledger.close_admission();
    let before = ledger.resources();
    assert!(
        lease
            .reduce(Resources {
                result_bytes: 101,
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(ledger.resources(), before);
    lease
        .reduce(Resources {
            result_bytes: 100,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        ledger.resources(),
        Resources {
            result_bytes: 100,
            ..Default::default()
        }
    );
    drop(lease);
    assert_eq!(ledger.resources(), Resources::default());
}
