use super::*;
#[test]
fn atomic_child_registration_races_for_last_active_slot_without_partial_records() {
    let root = AgentRef::root("root".into(), "session".into());
    let ledger = RootLedger::new(&root, RunLimits::default()).unwrap();
    let occupying = root.temporary_child().unwrap();
    let occupied = ledger
        .admit_child(
            &occupying,
            RunLimits::default(),
            resources::Resources {
                active_children: 1,
                ..Default::default()
            },
        )
        .unwrap();
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
    assert_eq!(ledger.resources().active_children, 2);
    assert_eq!(ledger.total().model_calls, 0);
    drop(results);
    drop(occupied);
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
fn racing_root_and_two_children_cannot_share_the_last_allowance() {
    let (root, child, ledger) = setup(1, 100, 10);
    let sibling = root.temporary_child().unwrap();
    let limits = ledger.0.lock().unwrap().limits.clone();
    ledger.register_child(&sibling, limits).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let results = std::thread::scope(|scope| {
        let handles = [
            root.agent_id().to_owned(),
            child.agent_id().to_owned(),
            sibling.agent_id().to_owned(),
        ]
        .map(|id| {
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
    let sibling = root.temporary_child().unwrap();
    let limits = ledger.0.lock().unwrap().limits.clone();
    ledger.register_child(&sibling, limits).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let winners = std::thread::scope(|scope| {
        let handles = [
            root.agent_id().to_owned(),
            child.agent_id().to_owned(),
            sibling.agent_id().to_owned(),
        ]
        .map(|id| {
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

#[test]
fn execution_identity_is_bound_once_from_registered_ownership() {
    let root = AgentRef::root("root-run".into(), "root-session".into());
    let ledger = RootLedger::new(&root, RunLimits::default()).unwrap();
    let child = root.temporary_child().unwrap();
    assert!(ledger.bind_execution(child.agent_id(), None).is_err());
    assert!(
        ledger
            .bind_execution(root.agent_id(), Some("spoof"))
            .is_err()
    );
    ledger.register_child(&child, RunLimits::default()).unwrap();
    let binding = ledger.bind_execution(root.agent_id(), None).unwrap();
    assert_eq!(binding.run_id, root.root_run_id());
    assert_eq!(binding.agent.agent_id(), root.agent_id());
    assert_eq!(binding.agent.session_id(), root.root_session_id());
    assert_eq!(binding.agent.parent_agent_id(), None);
    assert_eq!(binding.agent.depth(), 0);
    assert!(ledger.bind_execution(root.agent_id(), None).is_err());
    let binding = ledger
        .bind_execution(child.agent_id(), Some("child-session"))
        .unwrap();
    assert_ne!(binding.run_id, root.root_run_id());
    assert_eq!(binding.agent.agent_id(), child.agent_id());
    assert_eq!(binding.agent.root_run_id(), root.root_run_id());
    assert_eq!(binding.agent.root_session_id(), root.root_session_id());
    assert_eq!(binding.agent.session_id(), "child-session");
    assert_eq!(binding.agent.parent_agent_id(), Some(root.agent_id()));
    assert_eq!(binding.agent.depth(), 1);
    assert!(
        ledger
            .bind_execution(child.agent_id(), Some("replacement"))
            .is_err()
    );
    assert_eq!(ledger.total().model_calls, 0);
    assert_eq!(ledger.total().tool_calls, 0);
}

#[test]
fn event_reservations_are_atomic_and_closing_slots_survive_admission_close() {
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let limits = RunLimits {
        max_events: 5,
        ..Default::default()
    };
    let ledger = RootLedger::new(&root, limits.clone()).unwrap();
    ledger.register_child(&child, limits).unwrap();
    let mut root_terminal = ledger.claim_run_event(root.agent_id()).unwrap();
    let mut child_terminal = ledger.claim_run_event(child.agent_id()).unwrap();
    assert!(ledger.claim_run_event(root.agent_id()).is_err());
    assert!(ledger.claim_run_event(child.agent_id()).is_err());
    let barrier = std::sync::Barrier::new(2);
    let reservations = std::thread::scope(|threads| {
        let first = threads.spawn(|| {
            barrier.wait();
            ledger.reserve_events(root.agent_id(), 2)
        });
        let second = threads.spawn(|| {
            barrier.wait();
            ledger.reserve_events(child.agent_id(), 2)
        });
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(reservations.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        ledger.event_counts(),
        events::EventCounts {
            used: 0,
            reserved: 4
        }
    );
    drop(reservations);
    assert_eq!(ledger.event_counts().reserved, 2);
    for _ in 0..3 {
        ledger.admit_event(root.agent_id()).unwrap();
    }
    assert!(ledger.admit_event(child.agent_id()).is_err());
    assert!(ledger.reserve_events(child.agent_id(), 2).is_err());
    ledger.close_admission();
    root_terminal.consume().unwrap();
    child_terminal.consume().unwrap();
    assert!(root_terminal.consume().is_err());
    assert_eq!(
        ledger.event_counts(),
        events::EventCounts {
            used: 5,
            reserved: 0
        }
    );
}

#[test]
fn child_event_exhaustion_cannot_take_the_roots_prepaid_terminal() {
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let limits = RunLimits {
        max_events: 2,
        ..Default::default()
    };
    let ledger = RootLedger::new(&root, limits.clone()).unwrap();
    ledger.register_child(&child, limits).unwrap();
    ledger.admit_event(child.agent_id()).unwrap();
    assert!(ledger.claim_run_event(child.agent_id()).is_err());
    ledger.close_admission();
    let mut terminal = ledger.claim_run_event(root.agent_id()).unwrap();
    terminal.consume().unwrap();
    assert_eq!(
        ledger.event_counts(),
        events::EventCounts {
            used: 2,
            reserved: 0
        }
    );
}

#[test]
fn local_event_ceiling_leaves_other_agents_admission_available() {
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let ledger = RootLedger::new(
        &root,
        RunLimits {
            max_events: 8,
            ..Default::default()
        },
    )
    .unwrap();
    ledger
        .register_child(
            &child,
            RunLimits {
                max_events: 4,
                ..Default::default()
            },
        )
        .unwrap();
    let mut child_terminal = ledger.claim_run_event(child.agent_id()).unwrap();
    for _ in 0..3 {
        ledger.admit_event(child.agent_id()).unwrap();
    }
    assert!(ledger.admit_event(child.agent_id()).is_err());
    ledger.admit_event(root.agent_id()).unwrap();
    child_terminal.consume().unwrap();
    ledger
        .claim_run_event(root.agent_id())
        .unwrap()
        .consume()
        .unwrap();
    assert_eq!(
        ledger.event_counts(),
        events::EventCounts {
            used: 6,
            reserved: 0
        }
    );
}

#[test]
fn root_and_two_children_race_attested_token_and_cost_liability_separately() {
    for token_bound in [true, false] {
        let (root, child, ledger) = setup(
            3,
            if token_bound { 100 } else { 300 },
            if token_bound { 30 } else { 10 },
        );
        let sibling = root.temporary_child().unwrap();
        let limits = ledger.0.lock().unwrap().limits.clone();
        ledger.register_child(&sibling, limits).unwrap();
        let barrier = std::sync::Barrier::new(3);
        let reservations = std::thread::scope(|scope| {
            let threads = [&root, &child, &sibling].map(|agent| {
                let ledger = &ledger;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    ledger.reserve_model(
                        agent.agent_id(),
                        AccountingBounds {
                            tokens: Some(100),
                            cost_microusd: Some(10),
                        },
                    )
                })
            });
            threads.map(|thread| thread.join().unwrap())
        });
        assert_eq!(reservations.iter().filter(|r| r.is_ok()).count(), 1);
        for error in reservations.iter().filter_map(|r| r.as_ref().err()) {
            assert!(
                matches!(error, AdmissionError::Outcome(RunOutcome::LimitExceeded { limit })
                if *limit == if token_bound { LimitKind::TotalTokens } else { LimitKind::Cost })
            );
        }
        assert_eq!(ledger.total().model_calls, 1);
        assert_eq!(ledger.total().charged_tokens, Some(100));
        assert_eq!(ledger.total().charged_cost_microusd, Some(10));
        drop(reservations); // Uncertain delivery keeps its attested liability.
        assert_eq!(ledger.total().charged_tokens, Some(100));
        assert_eq!(ledger.total().charged_cost_microusd, Some(10));
        assert_eq!(ledger.total().usage.input_tokens, None);
    }
}
