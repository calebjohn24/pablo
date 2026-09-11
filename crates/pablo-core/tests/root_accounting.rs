//! Shared accounting must gate real runtime delivery, not merely ledger helpers.
use futures_util::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use pablo_core::{
    children::{AgentRef, ledger::RootLedger},
    provider::{AccountingBounds, ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    *,
};
use std::sync::atomic::{AtomicUsize, Ordering};
struct HeldProvider {
    root_deadline: tokio::time::Instant,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: AtomicUsize,
}
impl Provider for HeldProvider {
    fn name(&self) -> &'static str {
        "shared-ledger-fixture"
    }
    fn accounting_bounds(&self, _: &str, _: u32) -> AccountingBounds {
        AccountingBounds {
            tokens: Some(10),
            cost_microusd: Some(8),
        }
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            assert!(request.deadline <= self.root_deadline);
            assert_eq!(
                self.calls.fetch_add(1, Ordering::SeqCst),
                0,
                "second runtime crossed the root allowance"
            );
            self.entered.notify_one();
            self.release.notified().await;
            Ok(Box::pin(stream::iter(vec![
                Ok(ProviderEvent::TextDelta("actual provider result".into())),
                Ok(ProviderEvent::Cost { microusd: 7 }),
                Ok(ProviderEvent::Finished {
                    reason: FinishReason::Stop,
                    usage: Usage {
                        input_tokens: Some(3),
                        output_tokens: Some(2),
                        cache_read_input_tokens: Some(0),
                        cache_write_input_tokens: Some(0),
                    },
                }),
            ])) as ProviderStream<'a>)
        })
    }
}
#[tokio::test]
async fn root_reservation_prevents_another_runtime_from_delivering_while_first_is_active() {
    let exporter = InMemorySpanExporter::default();
    let sdk = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let mut spec = RunSpec::new(
        "fixture",
        std::env::temp_dir().canonicalize().unwrap(),
        "fixture",
    );
    spec.limits.max_model_calls = Some(1);
    spec.limits.max_total_tokens = Some(10);
    spec.limits.max_cost_microusd = Some(8);
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    ledger.register_child(&child, spec.limits.clone()).unwrap();
    let runtime = Runtime::new(telemetry::tracer(&sdk))
        .with_root_ledger(ledger.clone(), root.agent_id().into())
        .unwrap();
    let other = Runtime::new(telemetry::tracer(&sdk))
        .with_root_ledger(ledger.clone(), child.agent_id().into())
        .unwrap();
    let provider = HeldProvider {
        root_deadline: ledger.deadline(),
        entered: Default::default(),
        release: Default::default(),
        calls: AtomicUsize::new(0),
    };
    let mut own_events = Vec::new();
    let mut other_events = Vec::new();
    let first = async {
        runtime
            .run(&spec, &provider, &mut |e: &RunEvent| {
                own_events.push(e.clone());
                Ok(())
            })
            .await
            .unwrap()
    };
    let second = async {
        provider.entered.notified().await;
        let outcome = other
            .run(&spec, &provider, &mut |e: &RunEvent| {
                other_events.push(e.clone());
                Ok(())
            })
            .await
            .unwrap();
        provider.release.notify_one();
        outcome
    };
    let (completed, denied) = tokio::join!(first, second);
    assert!(completed.is_completed());
    assert_eq!(
        denied,
        RunOutcome::LimitExceeded {
            limit: LimitKind::ModelCalls
        }
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.total().model_calls, 1);
    assert_eq!(ledger.total().charged_tokens, Some(5));
    assert_eq!(ledger.total().charged_cost_microusd, Some(7));
    assert_eq!(ledger.agent(child.agent_id()).unwrap().model_calls, 0);
    assert_eq!(
        other_events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::ModelFinished { .. }))
            .count(),
        1
    );
    assert_eq!(
        other_events
            .last()
            .unwrap()
            .accounting
            .as_ref()
            .unwrap()
            .charged_tokens,
        Some(0),
        "denied global admission cannot leave a phantom local charge"
    );
    assert_eq!(
        own_events.last().unwrap().accounting.as_deref(),
        Some(&ledger.agent(root.agent_id()).unwrap())
    );
    for (events, expected) in [(&own_events, &root), (&other_events, &child)] {
        let identity = events[0].agent.as_ref().unwrap();
        assert_eq!(identity.agent_id(), expected.agent_id());
        assert_eq!(identity.root_run_id(), root.root_run_id());
        assert_eq!(identity.root_session_id(), root.root_session_id());
        assert_eq!(identity.parent_agent_id(), expected.parent_agent_id());
        for event in events {
            assert_eq!(event.agent.as_ref(), Some(identity));
            assert_eq!(event.session_id, identity.session_id());
        }
    }
    let mut jsonl = Vec::new();
    {
        let mut trace_spec = spec.clone();
        trace_spec.trace.capture_content = false;
        let mut sink = events::JsonlSink::new(&mut jsonl, &trace_spec).unwrap();
        for event in &own_events {
            sink.emit(event).unwrap();
        }
    }
    let lines = String::from_utf8(jsonl).unwrap();
    assert!(!lines.contains("actual provider result"));
    for line in lines.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(event["agent"]["agent_id"], root.agent_id());
    }
    sdk.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 4);
    for span in spans {
        let native = own_events
            .iter()
            .chain(&other_events)
            .find(|event| event.span_id == span.span_context.span_id().to_string())
            .unwrap();
        let identity = native.agent.as_ref().unwrap();
        for (key, expected) in [
            ("pablo.agent.id", identity.agent_id()),
            ("pablo.root.run.id", identity.root_run_id()),
            ("pablo.root.session.id", identity.root_session_id()),
            ("pablo.agent.session.id", identity.session_id()),
        ] {
            assert!(
                span.attributes
                    .iter()
                    .any(|attribute| attribute.key.as_str() == key
                        && attribute.value.as_str() == expected)
            );
        }
    }
    sdk.shutdown().unwrap();
}
#[tokio::test]
async fn shared_tool_exhaustion_finishes_the_span_without_mutating_a_file() {
    let cwd = std::env::temp_dir().join(format!("pablo-root-tools-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let sdk = SdkTracerProvider::builder().build();
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let mut spec = RunSpec::new("write", cwd.clone(), "fixture");
    spec.limits.max_tool_calls = Some(1);
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    ledger.register_child(&child, spec.limits.clone()).unwrap();
    ledger.admit_tool(root.agent_id()).unwrap();
    let provider=ScriptedProvider::new(vec![(std::time::Duration::ZERO,Ok(ProviderEvent::ToolCallStart{id:"write".into(),name:"fs.write".into()})),(std::time::Duration::ZERO,Ok(ProviderEvent::ToolCallArgumentsDelta{id:"write".into(),delta:serde_json::json!({"path":"denied.txt","text":"forbidden","expected_revision":null}).to_string()})),(std::time::Duration::ZERO,Ok(ProviderEvent::Finished{reason:FinishReason::ToolCalls,usage:Usage::default()}))]);
    let tools =
        ToolRegistry::configured_with_writes(false, true, true, Default::default()).unwrap();
    let mut events = Vec::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .with_root_ledger(ledger.clone(), child.agent_id().into())
        .unwrap()
        .run_with_tools(
            &spec,
            &provider,
            &tools,
            &CancellationToken::new(),
            &mut |e: &RunEvent| {
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::ToolCalls
        }
    );
    assert!(!cwd.join("denied.txt").exists());
    assert_eq!(ledger.total().tool_calls, 1);
    assert_eq!(ledger.agent(child.agent_id()).unwrap().tool_calls, 0);
    assert_eq!(events.iter().filter(|e|matches!(&e.kind,EventKind::ToolFinished{result,..}if result.status==pablo_core::tool::ToolStatus::AdmissionFailed)).count(),1);
    sdk.shutdown().unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn cancellation_while_waiting_for_root_mutation_gate_does_not_write_or_spend_tool_allowance()
{
    let cwd = std::env::temp_dir().join(format!("pablo-root-mutation-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let spec = RunSpec::new("write", cwd.clone(), "fixture");
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    ledger.register_child(&child, spec.limits.clone()).unwrap();
    let owner_cancel = CancellationToken::new();
    let held = ledger
        .lock_mutation(root.agent_id(), &owner_cancel, ledger.deadline())
        .await
        .unwrap();
    let sdk = SdkTracerProvider::builder().build();
    let runtime = Runtime::new(telemetry::tracer(&sdk))
        .with_root_ledger(ledger.clone(), child.agent_id().into())
        .unwrap();
    let cancellation = CancellationToken::new();
    let model_done = tokio::sync::Notify::new();
    let provider=ScriptedProvider::new(vec![(std::time::Duration::ZERO,Ok(ProviderEvent::ToolCallStart{id:"write".into(),name:"fs.write".into()})),(std::time::Duration::ZERO,Ok(ProviderEvent::ToolCallArgumentsDelta{id:"write".into(),delta:serde_json::json!({"path":"unwritten.txt","text":"forbidden","expected_revision":null}).to_string()})),(std::time::Duration::ZERO,Ok(ProviderEvent::Finished{reason:FinishReason::ToolCalls,usage:Usage::default()}))]);
    let tools =
        ToolRegistry::configured_with_writes(false, true, true, Default::default()).unwrap();
    let mut events = Vec::new();
    let running = async {
        runtime
            .run_with_tools(
                &spec,
                &provider,
                &tools,
                &cancellation,
                &mut |event: &RunEvent| {
                    if matches!(event.kind, EventKind::ModelFinished { .. }) {
                        model_done.notify_one();
                    }
                    events.push(event.clone());
                    Ok(())
                },
            )
            .await
            .unwrap()
    };
    let stopping = async {
        model_done.notified().await;
        cancellation.cancel();
    };
    let (outcome, ()) = tokio::join!(running, stopping);
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(!cwd.join("unwritten.txt").exists());
    assert_eq!(ledger.total().tool_calls, 0);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
    );
    drop(held);
    sdk.shutdown().unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn shared_deadline_cancellation_keeps_native_terminal_outcome_timed_out() {
    let sdk = SdkTracerProvider::builder().build();
    let root = AgentRef::root("root".into(), "session".into());
    let child = root.temporary_child().unwrap();
    let mut spec = RunSpec::new(
        "deadline",
        std::env::temp_dir().canonicalize().unwrap(),
        "fixture",
    );
    spec.limits.max_run_duration_ms = 20;
    let ledger = RootLedger::new(&root, spec.limits.clone()).unwrap();
    ledger.register_child(&child, spec.limits.clone()).unwrap();
    let runtime = Runtime::new(telemetry::tracer(&sdk))
        .with_root_ledger(ledger.clone(), child.agent_id().into())
        .unwrap();
    let provider = ScriptedProvider::new(vec![(
        std::time::Duration::from_secs(30),
        Ok(ProviderEvent::TextDelta("too late".into())),
    )]);
    let cancel = CancellationToken::new();
    let mut events = Vec::new();
    let mut sink = |event: &RunEvent| {
        events.push(event.clone());
        Ok(())
    };
    let tools = ToolRegistry::default();
    let run = runtime.run_with_tools(&spec, &provider, &tools, &cancel, &mut sink);
    let shutdown = async {
        tokio::time::sleep_until(ledger.deadline()).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(run, shutdown);
    let outcome = result.unwrap();
    assert_eq!(outcome, RunOutcome::TimedOut);
    assert!(
        matches!(&events.last().unwrap().kind,EventKind::RunFinished{outcome:native,..} if *native==outcome)
    );
    assert_eq!(
        events.last().unwrap().accounting.as_deref(),
        Some(&ledger.agent(child.agent_id()).unwrap())
    );
    sdk.shutdown().unwrap();
}
