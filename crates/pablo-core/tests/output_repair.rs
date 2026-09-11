use futures_util::{future::BoxFuture, stream};
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use pablo_core::{
    output::OutputSettings,
    provider::{AccountingBounds, ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    *,
};
use serde_json::json;
use std::sync::Mutex;

struct Fixture {
    mode: &'static str,
    seen: Mutex<Vec<(Vec<Message>, String, usize)>>,
}
impl Provider for Fixture {
    fn name(&self) -> &'static str {
        "repair-fixture"
    }
    fn accounting_bounds(&self, _: &str, _: u32) -> AccountingBounds {
        AccountingBounds {
            tokens: Some(10),
            cost_microusd: Some(8),
        }
    }
    fn stream<'a>(
        &'a self,
        r: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let n = {
                let mut seen = self.seen.lock().unwrap();
                seen.push((
                    r.messages.to_vec(),
                    r.instructions.into(),
                    r.max_output_bytes,
                ));
                seen.len()
            };
            assert!(n <= 2, "unexpected third request");
            if n == 2 {
                assert!(!r.allow_tool_calls);
                if self.mode == "cancel" {
                    r.cancellation.cancel();
                }
                if self.mode == "deadline" {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                if self.mode == "provider" {
                    return Err(ProviderError {
                        code: FailureCode::ProviderRejected,
                        delivery: DeliveryCertainty::ResponseReceived,
                        retry_class: Some(provider::RetryClass::ServiceUnavailable),
                    });
                }
            }
            let text = if (n == 2 && self.mode != "invalid") || self.mode == "first_valid" {
                r#"{"answer":7}"#
            } else {
                r#"{"answer":0}"#
            };
            let mut events = vec![ProviderEvent::TextDelta(text.into())];
            if n == 2 && self.mode == "tool" {
                events = vec![ProviderEvent::ToolCallStart {
                    id: "forbidden".into(),
                    name: "fs.read".into(),
                }];
            }
            if n == 2 && self.mode == "tool" {
                events.push(ProviderEvent::ToolCallArgumentsDelta {
                    id: "forbidden".into(),
                    delta: r#"{"path":"file"}"#.into(),
                });
            }
            events.push(ProviderEvent::Cost { microusd: 4 });
            events.push(ProviderEvent::Finished {
                reason: if n == 2 && self.mode == "tool" {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                },
                usage: Usage {
                    input_tokens: Some(2),
                    output_tokens: Some(2),
                    cache_read_input_tokens: Some(0),
                    cache_write_input_tokens: Some(0),
                },
            });
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}
#[tokio::test]
async fn repair_reuses_history_and_accounting_and_never_dispatches_a_third_call() {
    for mode in [
        "valid",
        "invalid",
        "first_valid",
        "disabled",
        "models",
        "tokens",
        "cost",
        "bytes",
        "remaining_bytes",
        "shared_work",
        "work",
        "context",
        "events",
        "sink",
        "cancel",
        "deadline",
        "provider",
        "tool",
    ] {
        let exporter = InMemorySpanExporter::default();
        let sdk = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let p = Fixture {
            mode,
            seen: Mutex::new(Vec::new()),
        };
        let mut spec = RunSpec::new(
            "private original task",
            std::env::current_dir().unwrap(),
            "repair-fixture",
        );
        spec.instructions = "stable private prefix".into();
        let mut output = OutputSettings::new(
            json!({"type":"object","properties":{"answer":{"type":"integer","minimum":1}},"required":["answer"]}),
        );
        output.repair.enabled = mode != "disabled";
        output.repair.max_feedback_bytes = 512;
        if mode == "shared_work" {
            output.max_validation_work = output.compile().unwrap().work(12) + 1;
        }
        if mode == "work" {
            output.max_validation_work = 1;
        }
        spec.output = Some(output);
        spec.limits.max_model_calls = Some(if mode == "models" { 1 } else { 3 });
        spec.limits.max_total_tokens = Some(if mode == "tokens" { 10 } else { 30 });
        spec.limits.max_cost_microusd = Some(if mode == "cost" { 8 } else { 24 });
        spec.limits.max_output_bytes = match mode {
            "bytes" => 12,
            "remaining_bytes" => 20,
            _ => 4096,
        };
        if mode == "context" {
            spec.limits.max_context_bytes = 600;
        }
        if mode == "events" {
            spec.limits.max_events = 6;
        }
        if mode == "deadline" {
            spec.limits.max_run_duration_ms = 50;
        }
        let mut events = Vec::new();
        let mut starts = 0;
        let outcome = Runtime::new(sdk.tracer("test"))
            .run_with_tools(
                &spec,
                &p,
                &ToolRegistry::default(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    if matches!(e.kind, EventKind::ModelStarted { .. }) {
                        starts += 1;
                        if mode == "sink" && starts == 2 {
                            return Err(SinkError::Capacity);
                        }
                    }
                    Ok(())
                },
            )
            .await
            .unwrap();
        let task = TaskResult::from_terminal(events.last().unwrap()).unwrap();
        let seen = p.seen.lock().unwrap();
        let expected_calls = if [
            "first_valid",
            "disabled",
            "models",
            "tokens",
            "cost",
            "bytes",
            "work",
            "context",
            "events",
            "sink",
        ]
        .contains(&mode)
        {
            1
        } else {
            2
        };
        assert_eq!(seen.len(), expected_calls, "{mode}: {outcome:?}");
        assert_eq!(
            task.accounting.as_ref().unwrap().model_calls,
            expected_calls as u64
        );
        assert_eq!(
            task.accounting.as_ref().unwrap().charged_tokens,
            Some(
                if ["remaining_bytes", "cancel", "deadline", "provider", "tool"].contains(&mode) {
                    14
                } else {
                    expected_calls as u64 * 4
                }
            )
        );
        assert_eq!(
            outcome.is_completed(),
            ["valid", "first_valid"].contains(&mode),
            "{mode}: {outcome:?}"
        );
        let expected_limit = match mode {
            "models" => Some(LimitKind::ModelCalls),
            "tokens" => Some(LimitKind::TotalTokens),
            "cost" => Some(LimitKind::Cost),
            "bytes" | "remaining_bytes" => Some(LimitKind::OutputBytes),
            "context" => Some(LimitKind::ContextBytes),
            "events" => Some(LimitKind::Events),
            "sink" => Some(LimitKind::TraceBytes),
            _ => None,
        };
        if let Some(limit) = expected_limit {
            assert_eq!(outcome, RunOutcome::LimitExceeded { limit }, "{mode}");
        }
        if mode == "cancel" {
            assert_eq!(outcome, RunOutcome::Cancelled);
        }
        if mode == "deadline" {
            assert_eq!(outcome, RunOutcome::TimedOut);
        }
        if mode == "tool" {
            assert!(matches!(
                outcome,
                RunOutcome::Failed {
                    code: FailureCode::MalformedStream,
                    ..
                }
            ));
            assert_eq!(task.accounting.as_ref().unwrap().tool_calls, 0);
        }
        if seen.len() == 2 {
            assert_eq!(seen[0].1, seen[1].1);
            assert_eq!(seen[0].0[0], seen[1].0[0]);
            assert_eq!(seen[1].0.len(), 3);
            assert!(
                matches!(&seen[1].0[1],Message::Assistant{text,tool_calls} if text==r#"{"answer":0}"#&&tool_calls.is_empty())
            );
            assert!(
                matches!(&seen[1].0[2],Message::User{text} if text.contains("/answer")&&text.len()<=512&&!text.contains("private original"))
            );
            assert_eq!(seen[1].2, spec.limits.max_output_bytes - 12);
        }
        if mode == "disabled" {
            assert!(task.output_repair.is_none());
        } else {
            let record = task.output_repair.unwrap();
            assert_eq!(record.attempts, if expected_calls == 2 { 1 } else { 0 });
            assert_eq!(
                record.status,
                if mode == "first_valid" {
                    "not_needed"
                } else if mode == "valid" {
                    "succeeded"
                } else if expected_calls == 2 {
                    "failed"
                } else {
                    "blocked"
                }
            );
        }
        if [
            "models",
            "tokens",
            "cost",
            "bytes",
            "remaining_bytes",
            "context",
            "events",
            "sink",
            "cancel",
            "deadline",
            "provider",
            "tool",
        ]
        .contains(&mode)
        {
            assert_eq!(
                task.output_validation.as_ref().unwrap().status,
                "unvalidated",
                "{mode}"
            );
        }
        sdk.force_flush().unwrap();
        let spans = exporter.get_finished_spans().unwrap();
        let printed = format!("{spans:?}");
        assert!(
            !printed.contains("stable private")
                && !printed.contains("/answer")
                && !printed.contains("Correct the previous")
        );
        if expected_calls == 2 {
            assert!(printed.contains("output_repair"));
        }
    }
}

#[tokio::test]
async fn repair_uses_the_original_bounded_jsonl_sink() {
    let sdk = SdkTracerProvider::builder().build();
    let p = Fixture {
        mode: "valid",
        seen: Mutex::new(Vec::new()),
    };
    let mut spec = RunSpec::new("task", std::env::current_dir().unwrap(), "fixture");
    let mut output = OutputSettings::new(json!({"properties":{"answer":{"minimum":1}}}));
    output.repair.enabled = true;
    spec.output = Some(output);
    spec.trace.max_bytes = 12288;
    let mut sink = JsonlSink::new(Vec::new(), &spec).unwrap();
    let outcome = Runtime::new(sdk.tracer("test"))
        .run(&spec, &p, &mut sink)
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::TraceBytes
        }
    );
    let bytes = sink.into_inner();
    assert!(bytes.len() <= spec.trace.max_bytes);
    let events: Vec<serde_json::Value> = bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    let terminal = events.last().unwrap();
    assert_eq!(terminal["type"], "run.finished");
    assert_ne!(terminal["output_validation"]["status"], "valid");
    assert!(p.seen.lock().unwrap().len() <= 2);
    assert!(terminal["output_repair"]["attempts"].as_u64().unwrap() <= 1);
}
