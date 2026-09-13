use futures_util::{future::BoxFuture, stream};
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use pablo_core::{
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    *,
};
use std::sync::Mutex;

#[derive(Clone)]
struct Seen {
    reasoning: ReasoningConfig,
    messages: Vec<Message>,
    summary: bool,
    tools: serde_json::Value,
    instructions: String,
}
struct Fixture {
    mode: &'static str,
    seen: Mutex<Vec<Seen>>,
    reads: Mutex<usize>,
    overflow: Mutex<bool>,
}
impl Provider for Fixture {
    fn validate_reasoning(
        &self,
        reasoning: ReasoningConfig,
        max_output_tokens: u32,
    ) -> Result<(), &'static str> {
        reasoning.validate(max_output_tokens)
    }
    fn name(&self) -> &'static str {
        "compaction-fixture"
    }
    fn stream<'a>(
        &'a self,
        r: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let summary = matches!(r.messages.last(),Some(Message::User{text}) if text.starts_with("Create a concise handoff"));
            self.seen.lock().unwrap().push(Seen {
                reasoning: r.reasoning,
                messages: r.messages.to_vec(),
                summary,
                tools: serde_json::to_value(r.tools).unwrap(),
                instructions: r.instructions.into(),
            });
            let mut events = Vec::new();
            let reason = if summary {
                assert!(!r.allow_tool_calls);
                match self.mode {
                    "empty"=>{},
                    "oversized"=>events.push(ProviderEvent::TextDelta("x".repeat(20000))),
                    "tool"=>events.push(ProviderEvent::ToolCallStart{id:"forbidden-summary-effect".into(),name:"fs.read".into()}),
                    _=>events.push(ProviderEvent::TextDelta("Goal: inspect evidence once per requested step; keep workspace constraints. Artifact evidence.txt remains authoritative; continue unresolved work.".into())),
                }
                FinishReason::Stop
            } else if *self.reads.lock().unwrap() < 3 {
                let mut reads = self.reads.lock().unwrap();
                *reads += 1;
                let id = format!("read{reads}");
                events.push(ProviderEvent::ToolCallStart {
                    id: id.clone(),
                    name: "fs.read".into(),
                });
                events.push(ProviderEvent::ToolCallArgumentsDelta {
                    id,
                    delta: r#"{"path":"evidence.txt"}"#.into(),
                });
                FinishReason::ToolCalls
            } else {
                if self.mode == "overflow" && !*self.overflow.lock().unwrap() {
                    *self.overflow.lock().unwrap() = true;
                    return Err(ProviderError {
                        code: FailureCode::ContextOverflow,
                        delivery: DeliveryCertainty::ResponseReceived,
                        retry_class: None,
                    });
                }
                events.push(ProviderEvent::TextDelta("done".into()));
                FinishReason::Stop
            };
            events.push(ProviderEvent::Finished {
                reason,
                usage: Usage::default(),
            });
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}

#[tokio::test]
async fn reasoning_budget_survives_compaction_or_rejects_before_replacing_history() {
    for budget_tokens in [64, 128] {
        let cwd = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("evidence.txt"), "e".repeat(6000)).unwrap();
        let provider = Fixture {
            mode: "threshold",
            seen: Mutex::new(Vec::new()),
            reads: Mutex::new(0),
            overflow: Mutex::new(false),
        };
        let mut spec = RunSpec::new(
            "Inspect evidence in three steps. Preserve workspace constraints.",
            cwd.clone(),
            "fixture/model",
        );
        spec.instructions = "unchanged authority".into();
        spec.reasoning = ReasoningConfig::Budget { budget_tokens };
        spec.limits.max_output_tokens = 2048;
        spec.context.max_summary_tokens = 128;
        spec.context.window_tokens = Some(8000);
        let sdk = SdkTracerProvider::builder().build();
        let mut events = Vec::new();
        let result = Runtime::new(sdk.tracer("test"))
            .run_with_tools(
                &spec,
                &provider,
                &ToolRegistry::with_filesystem_reads().unwrap(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        let seen = provider.seen.lock().unwrap();
        assert!(seen.iter().all(|r| r.reasoning == spec.reasoning));
        let record = events.last().unwrap().compaction.as_ref().unwrap();
        if budget_tokens == 64 {
            assert!(result.is_completed());
            assert_eq!(seen.iter().filter(|r| r.summary).count(), 1);
            assert_eq!(record.status, "completed");
        } else {
            assert!(matches!(
                result,
                RunOutcome::Failed {
                    code: FailureCode::CompactionFailed,
                    ..
                }
            ));
            assert!(seen.iter().all(|r| !r.summary));
            assert_eq!(
                record.reason.as_deref(),
                Some("summary_reasoning_unsupported")
            );
            assert_eq!(
                record.after_bytes, None,
                "no replacement history was committed"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e.kind, EventKind::TextDelta { .. }))
            );
            assert_eq!(
                TaskResult::from_terminal(events.last().unwrap())
                    .unwrap()
                    .accounting
                    .unwrap()
                    .model_calls as usize,
                seen.len(),
                "no summary request was charged"
            );
        }
        assert_eq!(*provider.reads.lock().unwrap(), 3, "no tool replay");
        std::fs::remove_dir_all(cwd).unwrap();
    }
}
#[tokio::test]
async fn threshold_and_overflow_compact_once_and_preserve_task_prefix_and_complete_recent_turns() {
    for (mode, recent, window, output_tokens) in [
        ("threshold", 0, 8000, 2048),
        ("overflow", 0, 0, 2048),
        ("threshold", 1, 5000, 128),
        ("overflow", 1, 0, 128),
        ("threshold", 0, 5000, 128),
    ] {
        let expected_recent = if window == 5000 { 1 } else { recent };
        let cwd = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("evidence.txt"), "e".repeat(6000)).unwrap();
        let provider = Fixture {
            mode,
            seen: Mutex::new(Vec::new()),
            reads: Mutex::new(0),
            overflow: Mutex::new(false),
        };
        let mut spec = RunSpec::new(
            "Inspect evidence in three steps. Preserve workspace constraints.",
            cwd.clone(),
            "fixture/model",
        );
        spec.instructions = "unchanged authority".into();
        spec.limits.max_output_tokens = output_tokens;
        spec.limits.max_model_calls = Some(8);
        spec.context.max_summary_tokens = 128;
        spec.context.keep_recent_turns = recent;
        spec.context.window_tokens = if mode == "threshold" {
            Some(window)
        } else {
            None
        };
        let exporter = InMemorySpanExporter::default();
        let telemetry = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let root = children::AgentRef::root("root".into(), "root-session".into());
        let ledger = children::ledger::RootLedger::new(&root, spec.limits.clone()).unwrap();
        let mut after_compaction = None;
        let mut checked_reduction = false;
        let mut events = Vec::new();
        let result = Runtime::new(telemetry.tracer("test"))
            .with_root_ledger(ledger.clone(), root.agent_id().into())
            .unwrap()
            .run_with_tools(
                &spec,
                &provider,
                &ToolRegistry::with_filesystem_reads().unwrap(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    if matches!(e.kind, EventKind::ModelStarted { .. })
                        && let Some(after) = after_compaction.take()
                    {
                        assert_eq!(ledger.resources().context_bytes, after);
                        checked_reduction = true;
                    }
                    if matches!(e.kind, EventKind::CompactionFinished { .. }) {
                        after_compaction = e
                            .compaction
                            .as_ref()
                            .and_then(|c| c.after_bytes.as_ref())
                            .map(|b| b.parse::<usize>().unwrap());
                    }
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(result.is_completed(), "{mode}: {result:?}");
        assert!(checked_reduction);
        assert_eq!(ledger.resources().context_bytes, 0);
        let seen = provider.seen.lock().unwrap();
        let summary = seen.iter().position(|r| r.summary).expect("one summary");
        assert_eq!(seen.iter().filter(|r| r.summary).count(), 1);
        assert!(seen.iter().all(|r| r.instructions == spec.instructions
            && r.tools == seen[0].tools
            && r.messages[0] == seen[0].messages[0]));
        let following = &seen[summary + 1];
        assert!(
            matches!(&following.messages[1],Message::User{text} if text.starts_with("Derived history summary") && text.contains("evidence.txt"))
        );
        assert_eq!(
            following
                .messages
                .iter()
                .filter(|m| matches!(m, Message::Assistant { .. }))
                .count(),
            expected_recent
        );
        assert_eq!(
            following
                .messages
                .iter()
                .filter(|m| matches!(m, Message::Tool { .. }))
                .count(),
            expected_recent
        );
        let finish = events
            .iter()
            .find(|e| matches!(e.kind, EventKind::CompactionFinished { .. }))
            .unwrap();
        let record = finish.compaction.as_ref().unwrap();
        assert_eq!(record.trigger, mode);
        assert_eq!(record.status, "completed");
        telemetry.force_flush().unwrap();
        let spans = exporter.get_finished_spans().unwrap();
        let compact = spans.iter().find(|s| s.name == "compact_context").unwrap();
        assert_eq!(finish.span_id, compact.span_context.span_id().to_string());
        assert_eq!(
            spans
                .iter()
                .filter(|s| s.parent_span_id == compact.span_context.span_id())
                .count(),
            1
        );
        assert!(!format!("{spans:?}").contains("Goal: inspect evidence once"));

        assert!(
            record
                .after_bytes
                .as_ref()
                .unwrap()
                .parse::<usize>()
                .unwrap()
                < record.before_bytes.parse().unwrap()
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::CompactionFinished { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::TextDelta { .. }))
                .count(),
            1,
            "summary deltas must not become answer text"
        );
        let task = TaskResult::from_terminal(events.last().unwrap()).unwrap();
        assert_eq!(task.accounting.as_ref().unwrap().tool_calls, 3);
        assert_eq!(
            task.accounting.as_ref().unwrap().model_calls,
            if mode == "overflow" { 6 } else { 5 }
        );
        std::fs::remove_dir_all(cwd).unwrap();
    }
}
#[tokio::test]
async fn invalid_summary_stops_without_replaying_tools_or_publishing_summary_text() {
    for mode in ["empty", "oversized", "tool"] {
        let cwd = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("evidence.txt"), "e".repeat(6000)).unwrap();
        let provider = Fixture {
            mode,
            seen: Mutex::new(Vec::new()),
            reads: Mutex::new(0),
            overflow: Mutex::new(false),
        };
        let mut spec = RunSpec::new("inspect", cwd.clone(), "fixture/model");
        spec.limits.max_output_tokens = 128;
        spec.context.window_tokens = Some(5000);
        let telemetry = SdkTracerProvider::builder().build();
        let mut events = Vec::new();
        let result = Runtime::new(telemetry.tracer("test"))
            .run_with_tools(
                &spec,
                &provider,
                &ToolRegistry::with_filesystem_reads().unwrap(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(!result.is_completed());
        let seen = provider.seen.lock().unwrap();
        assert!(seen.last().unwrap().summary);
        assert_eq!(seen.iter().filter(|r| r.summary).count(), 1);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, EventKind::TextDelta { .. }))
        );
        let record = events.last().unwrap().compaction.as_ref().unwrap();
        assert_ne!(record.status, "completed");
        assert_eq!(record.after_bytes, None);
        std::fs::remove_dir_all(cwd).unwrap();
    }
}
