use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use pablo_core::{output::OutputSettings, *};
use serde_json::json;

#[tokio::test]
async fn validated_results_are_terminal_only_with_content_safe_spans_and_redacted_diagnostics() {
    for (text, expected) in [
        ("{\"answer\":7}", "valid"),
        ("prefix {\"answer\":7}", "invalid"),
        ("{\"answer\":0}", "invalid"),
    ] {
        let exporter = InMemorySpanExporter::default();
        let sdk = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let mut spec = RunSpec::new(
            "synthetic task",
            std::env::current_dir().unwrap(),
            "scripted/text-v1",
        );
        spec.output = Some(OutputSettings::new(
            json!({"type":"object","properties":{"answer":{"type":"integer","minimum":1}},"required":["answer"]}),
        ));
        spec.limits.max_model_calls = Some(1);
        let mut events = Vec::new();
        let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
        let result = Runtime::new(sdk.tracer("test"))
            .run(
                &spec,
                &ScriptedProvider::text([text]),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    trace.emit(e)
                },
            )
            .await
            .unwrap();
        assert_eq!(result.is_completed(), expected == "valid");
        let terminal = events.last().unwrap();
        assert_eq!(
            terminal.output_validation.as_ref().unwrap().status,
            expected
        );
        assert!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::TextDelta { .. }))
                .all(|e| e.output_validation.as_ref().unwrap().status == "unvalidated")
        );
        let task = TaskResult::from_terminal(terminal).unwrap();
        assert_eq!(task.schema_version, "c3.13");
        assert_eq!(task.accounting.unwrap().model_calls, 1);
        let bytes = trace.into_inner();
        let native: serde_json::Value = serde_json::from_slice(
            bytes
                .split(|b| *b == b'\n')
                .rfind(|v| !v.is_empty())
                .unwrap(),
        )
        .unwrap();
        assert!(
            native["output_validation"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["instance_path"] == "" && d["schema_path"] == "")
        );
        sdk.force_flush().unwrap();
        let spans = exporter.get_finished_spans().unwrap();
        let validate = spans.iter().find(|s| s.name == "validate_output").unwrap();
        let root = spans
            .iter()
            .find(|s| s.name == "invoke_agent pablo")
            .unwrap();
        assert_eq!(validate.parent_span_id, root.span_context.span_id());
        assert!(!format!("{spans:?}").contains("synthetic task"));
        assert!(!format!("{spans:?}").contains("minimum"));
    }
}

#[tokio::test]
async fn cancellation_and_output_bounds_never_publish_validated_success() {
    for mode in ["cancel", "deadline", "root_bytes", "json_bytes", "work"] {
        let sdk = SdkTracerProvider::builder().build();
        let mut spec = RunSpec::new("task", std::env::current_dir().unwrap(), "scripted/text-v1");
        let mut settings = OutputSettings::new(json!(true));
        if mode == "work" {
            settings.max_validation_work = 1;
        }
        spec.output = Some(settings);
        let text = if mode == "json_bytes" {
            format!("\"{}\"", "x".repeat(1048576))
        } else {
            "\"answer\"".into()
        };
        if mode == "root_bytes" {
            spec.limits.max_output_bytes = 2;
        }
        if mode == "deadline" {
            spec.limits.max_run_duration_ms = 10;
        }
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();
        let result = Runtime::new(sdk.tracer("test"))
            .run_with_tools(
                &spec,
                &ScriptedProvider::text([text]),
                &ToolRegistry::default(),
                &cancellation,
                &mut |e: &RunEvent| {
                    if mode == "cancel" && matches!(e.kind, EventKind::ModelFinished { .. }) {
                        cancellation.cancel();
                    }
                    if mode == "deadline" && matches!(e.kind, EventKind::ModelFinished { .. }) {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(!result.is_completed());
        let record = events.last().unwrap().output_validation.as_ref().unwrap();
        match mode {
            "cancel" => {
                assert_eq!(result, RunOutcome::Cancelled);
                assert_eq!(record.status, "unvalidated");
            }
            "deadline" => {
                assert_eq!(result, RunOutcome::TimedOut);
                assert_eq!(record.status, "unvalidated");
            }
            "root_bytes" => {
                assert_eq!(
                    result,
                    RunOutcome::LimitExceeded {
                        limit: LimitKind::OutputBytes
                    }
                );
                assert_eq!(record.status, "unvalidated");
            }
            _ => assert_eq!(record.status, "invalid"),
        }
    }
}
