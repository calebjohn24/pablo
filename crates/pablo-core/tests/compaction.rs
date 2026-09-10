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
async fn threshold_and_overflow_compact_once_and_preserve_task_prefix_and_complete_recent_turns() {
    for mode in ["threshold", "overflow"] {
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
        spec.limits.max_output_tokens = 128;
        spec.limits.max_model_calls = Some(8);
        spec.context.max_summary_tokens = 128;
        spec.context.window_tokens = if mode == "threshold" {
            Some(5000)
        } else {
            None
        };
        let exporter = InMemorySpanExporter::default();
        let telemetry = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
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
        assert!(result.is_completed(), "{mode}: {result:?}");
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
            1
        );
        assert_eq!(
            following
                .messages
                .iter()
                .filter(|m| matches!(m, Message::Tool { .. }))
                .count(),
            1
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
