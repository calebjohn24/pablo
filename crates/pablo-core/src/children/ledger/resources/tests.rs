use super::*;
fn setup() -> (AgentRef, AgentRef, AgentRef, RootLedger) {
    let root = AgentRef::root("run".into(), "session".into());
    let one = root.temporary_child().unwrap();
    let two = root.temporary_child().unwrap();
    let limits = RunLimits::default();
    let ledger = RootLedger::new(&root, limits.clone()).unwrap();
    ledger.register_child(&one, limits.clone()).unwrap();
    ledger.register_child(&two, limits).unwrap();
    (root, one, two, ledger)
}
#[test]
fn active_transition_is_atomic_and_capacity_is_released_only_by_its_owner() {
    let (root, one, two, ledger) = setup();
    let occupied = ledger
        .reserve_resources(
            one.agent_id(),
            Resources {
                active_children: 1,
                processes: 15,
                mcp_sessions: 15,
                ..Resources::default()
            },
        )
        .unwrap();
    let mut queued = ledger
        .reserve_resources(
            two.agent_id(),
            Resources {
                pending_children: 1,
                queued_input_bytes: 128,
                ..Resources::default()
            },
        )
        .unwrap();
    let before = ledger.resources();
    assert!(matches!(
        queued.replace(Resources {
            active_children: 1,
            processes: 1,
            mcp_sessions: 1,
            context_bytes: 128,
            ..Resources::default()
        }),
        Err(AdmissionError::Capacity)
    ));
    assert_eq!(ledger.resources(), before);
    // Failed multi-resource admission cannot consume the remaining process slot.
    assert!(
        ledger
            .reserve_resources(
                root.agent_id(),
                Resources {
                    processes: 1,
                    mcp_sessions: 2,
                    ..Resources::default()
                }
            )
            .is_err()
    );
    assert_eq!(ledger.resources(), before);
    let root_process = ledger
        .reserve_resources(
            root.agent_id(),
            Resources {
                processes: 1,
                mcp_sessions: 1,
                ..Resources::default()
            },
        )
        .unwrap();
    drop(occupied);
    queued
        .replace(Resources {
            active_children: 1,
            context_bytes: 128,
            ..Resources::default()
        })
        .unwrap();
    assert_eq!(ledger.resources().pending_children, 0);
    assert_eq!(ledger.resources().queued_input_bytes, 0);
    assert_eq!(ledger.resources().active_children, 1);
    // Completion retains a bounded result while releasing the active slot.
    queued
        .replace(Resources {
            result_bytes: 64,
            ..Resources::default()
        })
        .unwrap();
    assert_eq!(ledger.resources().active_children, 0);
    drop(root_process);
    drop(queued);
    assert_eq!(ledger.resources(), Resources::default());
}
#[test]
fn racing_children_cannot_both_claim_the_one_active_slot() {
    let (_, one, two, ledger) = setup();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let handles = [one.agent_id().to_owned(), two.agent_id().to_owned()].map(|id| {
            let ledger = ledger.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                ledger.reserve_resources(
                    &id,
                    Resources {
                        active_children: 1,
                        processes: 1,
                        ..Resources::default()
                    },
                )
            })
        });
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(ledger.resources().active_children, 1);
    drop(results);
    assert_eq!(ledger.resources(), Resources::default());
}
#[test]
fn context_is_aggregate_and_child_work_cannot_allocate_without_an_active_slot() {
    let (root, one, _, ledger) = setup();
    assert!(
        ledger
            .reserve_resources(
                one.agent_id(),
                Resources {
                    processes: 1,
                    ..Resources::default()
                }
            )
            .is_err()
    );
    assert!(
        ledger
            .reserve_resources(
                root.agent_id(),
                Resources {
                    active_children: 1,
                    ..Resources::default()
                }
            )
            .is_err()
    );
    let root_context = ledger
        .reserve_resources(
            root.agent_id(),
            Resources {
                context_bytes: RunLimits::default().max_context_bytes - 1024,
                ..Resources::default()
            },
        )
        .unwrap();
    assert!(
        ledger
            .reserve_resources(
                one.agent_id(),
                Resources {
                    active_children: 1,
                    context_bytes: 1025,
                    ..Resources::default()
                }
            )
            .is_err()
    );
    let child_context = ledger
        .reserve_resources(
            one.agent_id(),
            Resources {
                active_children: 1,
                context_bytes: 1024,
                ..Resources::default()
            },
        )
        .unwrap();
    assert_eq!(
        ledger.resources().context_bytes,
        RunLimits::default().max_context_bytes
    );
    ledger.close_admission();
    assert!(matches!(
        ledger.reserve_resources(
            root.agent_id(),
            Resources {
                processes: 1,
                ..Resources::default()
            }
        ),
        Err(AdmissionError::Closed)
    ));
    drop(root_context);
    drop(child_context);
    assert_eq!(ledger.resources(), Resources::default());
}
#[tokio::test]
async fn mutation_waiters_cancel_timeout_or_close_without_entering_the_critical_section() {
    let (root, child, _, ledger) = setup();
    let root_cancel = crate::CancellationToken::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let held = ledger
        .lock_mutation(root.agent_id(), &root_cancel, deadline)
        .await
        .unwrap();
    let cancel = crate::CancellationToken::new();
    let waiting = ledger.lock_mutation(child.agent_id(), &cancel, deadline);
    tokio::pin!(waiting);
    assert!(matches!(
        futures_util::poll!(&mut waiting),
        std::task::Poll::Pending
    ));
    cancel.cancel();
    assert!(matches!(waiting.await, Err(AdmissionError::Cancelled)));
    assert!(matches!(
        ledger
            .lock_mutation(child.agent_id(), &root_cancel, tokio::time::Instant::now())
            .await,
        Err(AdmissionError::TimedOut)
    ));
    drop(held);
    let acquired = ledger
        .lock_mutation(child.agent_id(), &root_cancel, deadline)
        .await
        .unwrap();
    let waiting = ledger.lock_mutation(root.agent_id(), &root_cancel, deadline);
    tokio::pin!(waiting);
    assert!(matches!(
        futures_util::poll!(&mut waiting),
        std::task::Poll::Pending
    ));
    ledger.close_admission();
    assert!(matches!(waiting.await, Err(AdmissionError::Closed)));
    drop(acquired);
}
