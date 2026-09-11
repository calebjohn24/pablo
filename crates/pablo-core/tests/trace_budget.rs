use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{
    children::{
        AgentRef,
        ledger::{
            AdmissionError, RootLedger,
            events::{EventCounts, TraceCounts},
        },
    },
    events::{EventSink, JsonlSink, SinkError, jsonl_size},
    *,
};

fn spec() -> RunSpec {
    let mut spec = RunSpec::new(
        "trace fixture",
        std::env::temp_dir().canonicalize().unwrap(),
        "fixture",
    );
    spec.limits.max_output_bytes = 1024;
    spec.trace.max_bytes = 64 * 1024;
    spec
}
async fn records(spec: &RunSpec) -> Vec<RunEvent> {
    let sdk = SdkTracerProvider::builder().build();
    let mut records = Vec::new();
    Runtime::new(telemetry::tracer(&sdk))
        .run(
            spec,
            &ScriptedProvider::text(["\0\n\"\\☃"]),
            &mut |e: &RunEvent| {
                records.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    sdk.shutdown().unwrap();
    records
}
#[tokio::test]
async fn counted_projection_matches_full_and_redacted_writer_with_exact_capacity() {
    for capture in [false, true] {
        let mut spec = spec();
        spec.trace.capture_content = capture;
        let records = records(&spec).await;
        let mut writer = JsonlSink::new(Vec::new(), &spec).unwrap();
        let mut total = 0;
        for record in &records {
            let size = jsonl_size(record, capture, spec.trace.max_bytes).unwrap();
            assert_eq!(jsonl_size(record, capture, size).unwrap(), size);
            assert!(matches!(
                jsonl_size(record, capture, size - 1),
                Err(SinkError::Capacity)
            ));
            writer.emit(record).unwrap();
            total += size;
        }
        assert_eq!(total, writer.bytes_written());
        assert_eq!(total, writer.into_inner().len());
        let mut huge = records[0].clone();
        huge.kind = EventKind::TextDelta {
            text: "\0".repeat(1024 * 1024),
        };
        assert!(matches!(
            jsonl_size(&huge, true, 4096),
            Err(SinkError::Capacity)
        ));
        assert!(jsonl_size(&huge, false, 4096).unwrap() < 4096);
    }
}

#[tokio::test]
async fn atomic_trace_race_keeps_counts_and_both_terminal_reservations() {
    let mut spec = spec();
    spec.trace.capture_content = true;
    let records = records(&spec).await;
    let event = &records[0];
    let terminal = records.last().unwrap();
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    // Leave exactly one ordinary record after protecting both terminals.
    let bound = 12 * 1024 + spec.limits.max_output_bytes * 6;
    let size = jsonl_size(event, true, usize::MAX).unwrap();
    spec.trace.max_bytes = bound * 2 + size;
    ledger.configure_trace(&spec.trace).unwrap();
    ledger.configure_trace(&spec.trace).unwrap();
    ledger.register_child(&child, spec.limits.clone()).unwrap();
    let mut root_terminal = ledger.claim_run_event(root.agent_id()).unwrap();
    let mut child_terminal = ledger.claim_run_event(child.agent_id()).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = [root.agent_id(), child.agent_id()]
            .into_iter()
            .map(|agent| {
                let ledger = &ledger;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    ledger.admit_event_record(agent, event)
                })
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(results.iter().any(|r| matches!(
        r,
        Err(AdmissionError::Outcome(RunOutcome::LimitExceeded {
            limit: LimitKind::TraceBytes
        }))
    )));
    assert_eq!(
        ledger.event_counts(),
        EventCounts {
            used: 1,
            reserved: 2
        }
    );
    assert_eq!(
        ledger.trace_counts(),
        Some(TraceCounts {
            used: size,
            reserved: bound * 2
        })
    );
    // Legacy count-only calls cannot bypass a traced ledger.
    assert!(ledger.admit_event(root.agent_id()).is_err());
    assert!(root_terminal.consume().is_err());
    assert!(root_terminal.consume_record(event).is_err());
    assert_eq!(
        ledger.event_counts(),
        EventCounts {
            used: 1,
            reserved: 2
        }
    );
    ledger.close_admission();
    child_terminal.consume_record(terminal).unwrap();
    root_terminal.consume_record(terminal).unwrap();
    assert!(root_terminal.consume_record(terminal).is_err());
    assert_eq!(
        ledger.event_counts(),
        EventCounts {
            used: 3,
            reserved: 0
        }
    );
    assert_eq!(
        ledger.trace_counts(),
        Some(TraceCounts {
            used: size + 2 * jsonl_size(terminal, true, usize::MAX).unwrap(),
            reserved: 0,
        })
    );
}

#[tokio::test]
async fn trace_policy_and_terminal_claim_fail_without_partial_mutation() {
    let spec = spec();
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    let settings = TraceSettings {
        capture_content: false,
        max_bytes: 12 * 1024,
    };
    let mut too_small = settings.clone();
    too_small.max_bytes -= 1;
    assert!(ledger.configure_trace(&too_small).is_err());
    assert_eq!(ledger.trace_counts(), None);
    ledger.configure_trace(&settings).unwrap();
    assert!(ledger.configure_trace(&spec.trace).is_err());
    ledger.register_child(&child, spec.limits.clone()).unwrap();
    assert!(ledger.claim_run_event(child.agent_id()).is_err());
    assert!(ledger.claim_run_event(child.agent_id()).is_err());
    assert_eq!(
        ledger.event_counts(),
        EventCounts {
            used: 0,
            reserved: 1
        }
    );
    assert_eq!(
        ledger.trace_counts(),
        Some(TraceCounts {
            used: 0,
            reserved: 12 * 1024
        })
    );
    let terminal = ledger.claim_run_event(root.agent_id()).unwrap();
    drop(terminal);
    assert_eq!(ledger.trace_counts(), Some(TraceCounts::default()));
    let untraced = RootLedger::new(&root, spec.limits.clone()).unwrap();
    let mut huge = records(&spec).await.remove(0);
    huge.kind = EventKind::TextDelta {
        text: "\0".repeat(1024 * 1024),
    };
    untraced.admit_event_record(root.agent_id(), &huge).unwrap();
    assert_eq!(untraced.trace_counts(), None);
    assert!(untraced.configure_trace(&settings).is_err());
}

#[tokio::test]
async fn scoped_tree_exhaustion_preserves_child_then_root_terminals_and_writer_lifetime() {
    for capture in [false, true] {
        let sdk = SdkTracerProvider::builder().build();
        let mut spec = spec();
        spec.trace.capture_content = capture;
        let root = AgentRef::root("root".into(), "session".into());
        let child = root.temporary_child().unwrap();
        let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
        ledger.configure_trace(&spec.trace).unwrap();
        ledger.register_child(&child, spec.limits.clone()).unwrap();
        let mut writer = JsonlSink::for_tree(Vec::new(), &spec, "root".into()).unwrap();
        // A host may start its child first: the root terminal is already protected.
        let child_outcome = Runtime::new(telemetry::tracer(&sdk))
            .with_root_ledger(ledger.clone(), child.agent_id().into())
            .unwrap()
            .run(
                &spec,
                &ScriptedProvider::text(std::iter::repeat_n("x", 1000)),
                &mut writer,
            )
            .await
            .unwrap();
        assert_eq!(
            child_outcome,
            RunOutcome::LimitExceeded {
                limit: LimitKind::TraceBytes
            }
        );
        let root_outcome = Runtime::new(telemetry::tracer(&sdk))
            .with_root_ledger(ledger.clone(), root.agent_id().into())
            .unwrap()
            .run(
                &spec,
                &ScriptedProvider::text(["root finished"]),
                &mut writer,
            )
            .await
            .unwrap();
        assert!(root_outcome.is_completed());
        let counts = ledger.trace_counts().unwrap();
        assert_eq!(counts.reserved, 0);
        assert_eq!(counts.used, writer.bytes_written());
        assert!(counts.used <= spec.trace.max_bytes);
        let rows: Vec<serde_json::Value> = String::from_utf8(writer.into_inner())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            ledger.event_counts(),
            EventCounts {
                used: rows.len() as u64,
                reserved: 0
            }
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row["type"] == "run.finished")
                .count(),
            2
        );
        assert_eq!(rows.last().unwrap()["run_id"], "root");
        sdk.shutdown().unwrap();
    }
}

#[tokio::test]
async fn event_denial_does_not_charge_trace_and_failed_sink_does_not_refund_admission() {
    let mut spec = spec();
    let records = records(&spec).await;
    spec.limits.max_events = 2;
    let root = AgentRef::root("root".into(), "session".into());
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    ledger.configure_trace(&spec.trace).unwrap();
    let mut terminal = ledger.claim_run_event(root.agent_id()).unwrap();
    ledger
        .admit_event_record(root.agent_id(), &records[0])
        .unwrap();
    let counts = ledger.trace_counts();
    assert!(matches!(
        ledger.admit_event_record(root.agent_id(), &records[0]),
        Err(AdmissionError::Outcome(RunOutcome::LimitExceeded {
            limit: LimitKind::Events
        }))
    ));
    assert_eq!(ledger.trace_counts(), counts);
    terminal.consume_record(records.last().unwrap()).unwrap();
    drop(terminal);
    assert_eq!(
        ledger.event_counts(),
        EventCounts {
            used: 2,
            reserved: 0
        }
    );

    // A sink can fail after a partial write: all admitted attempts stay spent.
    spec.limits.max_events = 4;
    let sdk = SdkTracerProvider::builder().build();
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    ledger.configure_trace(&spec.trace).unwrap();
    let mut attempted_bytes = 0;
    let result = Runtime::new(telemetry::tracer(&sdk))
        .with_root_ledger(ledger.clone(), root.agent_id().into())
        .unwrap()
        .run(
            &spec,
            &ScriptedProvider::text(["unused"]),
            &mut |event: &RunEvent| {
                attempted_bytes += jsonl_size(event, false, spec.trace.max_bytes).unwrap();
                Err(SinkError::Io(std::io::Error::other("fixture failure")))
            },
        )
        .await;
    assert!(matches!(result, Err(RunError::EventDelivery { .. })));
    assert!(attempted_bytes > 0);
    assert_eq!(
        ledger.trace_counts(),
        Some(TraceCounts {
            used: attempted_bytes,
            reserved: 0
        })
    );
    sdk.shutdown().unwrap();
}
