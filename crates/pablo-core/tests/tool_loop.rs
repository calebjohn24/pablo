#![cfg(any(target_os = "macos", target_os = "linux"))]

use futures_util::{future::BoxFuture, stream};
use opentelemetry::trace::Status;
use opentelemetry_sdk::trace::{InMemorySpanExporter, Sampler, SdkTracerProvider};
use pablo_core::{
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    telemetry,
    tool::{ToolResult, ToolStatus},
    *,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::{sleep, timeout};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pablo-c12-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn spec(&self) -> RunSpec {
        let mut spec = RunSpec::new("Read the evidence.", self.0.clone(), "scripted/tools-v1");
        spec.instructions = "Keep this instruction prefix stable.".into();
        spec
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn sdk() -> (SdkTracerProvider, InMemorySpanExporter) {
    let exporter = InMemorySpanExporter::default();
    (
        SdkTracerProvider::builder()
            .with_sampler(Sampler::AlwaysOn)
            .with_simple_exporter(exporter.clone())
            .build(),
        exporter,
    )
}
fn done() -> ProviderEvent {
    ProviderEvent::Finished {
        reason: FinishReason::Stop,
        usage: Usage::default(),
    }
}
fn call_events(id: &str, name: &str, arguments: &Value) -> Vec<ProviderEvent> {
    let mut events = vec![ProviderEvent::ToolCallStart {
        id: id.into(),
        name: name.into(),
    }];
    // Fragment at character boundaries, including escapes, instead of providing
    // a ready-made JSON value to the runtime's assembly path.
    let text = arguments.to_string();
    for part in text.as_bytes().chunks(7) {
        events.push(ProviderEvent::ToolCallArgumentsDelta {
            id: id.into(),
            delta: std::str::from_utf8(part).unwrap().into(),
        });
    }
    events
}
fn scripted(events: Vec<ProviderEvent>) -> ScriptedProvider {
    ScriptedProvider::new(
        events
            .into_iter()
            .map(|event| (Duration::ZERO, Ok(event)))
            .collect(),
    )
}
fn shell_provider(arguments: Value) -> ScriptedProvider {
    let mut events = call_events("call-1", "shell.run", &arguments);
    events.push(ProviderEvent::Finished {
        reason: FinishReason::ToolCalls,
        usage: Usage::default(),
    });
    scripted(events)
}
fn terminal(events: &[RunEvent], outcome: &RunOutcome) {
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
    assert_eq!(
        events.last().unwrap().kind,
        EventKind::RunFinished {
            outcome: outcome.clone()
        }
    );
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.seq, i as u64 + 1);
    }
}
fn tool_result(events: &[RunEvent]) -> &ToolResult {
    events
        .iter()
        .find_map(|event| {
            if let EventKind::ToolFinished { result, .. } = &event.kind {
                Some(result)
            } else {
                None
            }
        })
        .expect("tool finished")
}

struct RoundTrip {
    first: Vec<ProviderEvent>,
    requests: Mutex<Vec<(String, Value, Vec<Message>)>>,
    calls: AtomicUsize,
    expected: Vec<String>,
}
impl RoundTrip {
    fn new(commands: &[&str], expected: &[&str]) -> Self {
        let mut first = Vec::new();
        for (i, command) in commands.iter().enumerate() {
            first.extend(call_events(
                &format!("call-{i}"),
                "shell.run",
                &json!({"command":command,"cwd":"."}),
            ));
        }
        first.push(ProviderEvent::Finished {
            reason: FinishReason::ToolCalls,
            usage: Usage {
                input_tokens: Some(5),
                output_tokens: Some(2),
                ..Usage::default()
            },
        });
        Self {
            first,
            requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            expected: expected.iter().map(|s| s.to_string()).collect(),
        }
    }
}
impl Provider for RoundTrip {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            assert!(!request.cancellation.is_cancelled());
            self.requests.lock().unwrap().push((
                request.instructions.into(),
                serde_json::to_value(request.tools).unwrap(),
                request.messages.to_vec(),
            ));
            let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert!(
                        matches!(request.messages, [Message::User { text }] if text == request.input)
                    );
                    self.first.clone()
                }
                1 => {
                    let results: Vec<_> = request
                        .messages
                        .iter()
                        .filter_map(|message| {
                            if let Message::Tool { result, .. } = message {
                                Some(result)
                            } else {
                                None
                            }
                        })
                        .collect();
                    assert_eq!(results.len(), self.expected.len());
                    let mut answer = String::new();
                    for (result, expected) in results.iter().zip(&self.expected) {
                        assert_eq!(result.status, ToolStatus::Completed);
                        let actual = &result.shell.as_ref().unwrap().stdout;
                        assert_eq!(
                            actual, expected,
                            "the second request must contain actual shell output"
                        );
                        answer.push_str(actual);
                    }
                    vec![
                        ProviderEvent::TextDelta(answer),
                        ProviderEvent::Finished {
                            reason: FinishReason::Stop,
                            usage: Usage {
                                input_tokens: Some(9),
                                output_tokens: Some(3),
                                ..Usage::default()
                            },
                        },
                    ]
                }
                _ => panic!("unexpected retry or extra model call"),
            };
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}

#[tokio::test]
async fn actual_shell_evidence_round_trip_has_stable_prefix_and_correlated_tool_spans() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("evidence.txt"), "synthetic-evidence-84b\n").unwrap();
    let provider = RoundTrip::new(&["cat evidence.txt"], &["synthetic-evidence-84b\n"]);
    let (sdk, exporter) = sdk();
    let tools = ToolRegistry::with_shell().unwrap();
    let mut spec = fixture.spec();
    spec.trace.capture_content = true;
    let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
    let mut events = Vec::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            &spec,
            &provider,
            &tools,
            &CancellationToken::new(),
            &mut |e: &RunEvent| {
                trace.emit(e)?;
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    terminal(&events, &outcome);
    assert!(
        matches!(outcome, RunOutcome::Completed { output, usage, .. } if output == "synthetic-evidence-84b\n" && usage.input_tokens == Some(14) && usage.output_tokens == Some(5) && usage.cache_read_input_tokens.is_none())
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, requests[1].0);
    assert_eq!(requests[0].1, requests[1].1);
    assert_eq!(requests[0].1[0]["name"], "shell.run");
    assert_eq!(requests[1].2.len(), 3);
    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 4);
    let root = spans
        .iter()
        .find(|s| s.name == "invoke_agent pablo")
        .unwrap();
    let tool = spans
        .iter()
        .find(|s| s.name == "execute_tool shell.run")
        .unwrap();
    assert_eq!(tool.parent_span_id, root.span_context.span_id());
    assert_eq!(tool.status, Status::Unset);
    for span in &spans {
        assert_eq!(span.span_context.trace_id(), root.span_context.trace_id());
        assert!(!format!("{span:?}").contains("synthetic-evidence-84b"));
        assert!(!format!("{span:?}").contains("cat evidence.txt"));
    }
    let tool_events: Vec<_> = events
        .iter()
        .filter(|e| e.span_id == tool.span_context.span_id().to_string())
        .collect();
    assert_eq!(tool_events.len(), 3);
    assert!(matches!(tool_events[0].kind, EventKind::ToolStarted { .. }));
    assert!(matches!(
        tool_events[1].kind,
        EventKind::ShellStarted { .. }
    ));
    assert!(matches!(
        tool_events[2].kind,
        EventKind::ToolFinished { .. }
    ));
    assert_eq!(
        tool.start_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros(),
        u128::from(tool_events[0].timestamp_unix_micros)
    );
    assert_eq!(
        tool.end_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros(),
        u128::from(tool_events[2].timestamp_unix_micros)
    );
    let decoded: Vec<RunEvent> = String::from_utf8(trace.into_inner())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(decoded, events);
}

#[tokio::test]
async fn two_tool_calls_execute_sequentially_and_nonzero_exit_is_returned_to_model() {
    let fixture = Fixture::new();
    let provider = RoundTrip::new(
        &[
            "printf first > order; printf out; printf err >&2; exit 7",
            "cat order",
        ],
        &["out", "first"],
    );
    let (sdk, exporter) = sdk();
    let mut events = Vec::new();
    let result = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            &fixture.spec(),
            &provider,
            &ToolRegistry::with_shell().unwrap(),
            &CancellationToken::new(),
            &mut |e: &RunEvent| {
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert!(result.is_completed());
    terminal(&events, &result);
    let shell = tool_result(&events).shell.as_ref().unwrap();
    assert_eq!(shell.exit_code, Some(7));
    assert_eq!(shell.stderr, "err");
    let lifecycle: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolStarted { call } => Some(format!("start:{}", call.id)),
            EventKind::ToolFinished { call_id, .. } => Some(format!("end:{call_id}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle,
        ["start:call-0", "end:call-0", "start:call-1", "end:call-1"]
    );
    assert_eq!(
        exporter
            .get_finished_spans()
            .unwrap()
            .iter()
            .filter(|s| matches!(s.status, Status::Error { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn schema_and_policy_rejections_never_spawn_a_process() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("inside")).unwrap();
    std::os::unix::fs::symlink(std::env::temp_dir(), fixture.0.join("escape")).unwrap();
    let (sdk, _) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let tools = ToolRegistry::with_shell().unwrap();
    for (args, expected) in [
        (json!({"command":"touch bad"}), ToolStatus::InvalidArguments),
        (
            json!({"command":"touch bad","cwd":".","tty":true}),
            ToolStatus::InvalidArguments,
        ),
        (
            json!({"command":"touch bad","cwd":".","timeout_ms":0}),
            ToolStatus::InvalidArguments,
        ),
        (
            json!({"command":12,"cwd":"."}),
            ToolStatus::InvalidArguments,
        ),
        (
            json!({"command":"touch bad","cwd":".."}),
            ToolStatus::PolicyDenied,
        ),
        (
            json!({"command":"touch bad","cwd":"escape"}),
            ToolStatus::PolicyDenied,
        ),
        (
            json!({"command":"touch bad","cwd":".","env":{"AI_GATEWAY_API_KEY":"no"}}),
            ToolStatus::PolicyDenied,
        ),
        (
            json!({"command":"touch bad","cwd":".","env":{"ENV":"no"}}),
            ToolStatus::PolicyDenied,
        ),
        (
            json!({"command":"touch bad","cwd":".","env":{"PABLO_TASK_VALUE":"x\u{0000}"}}),
            ToolStatus::InvalidArguments,
        ),
    ] {
        let mut events = Vec::new();
        let outcome = runtime
            .run_with_tools(
                &fixture.spec(),
                &shell_provider(args),
                &tools,
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(!outcome.is_completed());
        terminal(&events, &outcome);
        assert_eq!(tool_result(&events).status, expected);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, EventKind::ShellStarted { .. }))
        );
        assert!(!fixture.0.join("bad").exists());
    }
}

#[tokio::test]
async fn empty_catalog_denies_tools_and_budgets_prevent_unnecessary_effects() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    for mode in 0..5 {
        let mut spec = fixture.spec();
        let tools = if mode == 0 {
            ToolRegistry::default()
        } else {
            ToolRegistry::with_shell().unwrap()
        };
        match mode {
            1 => spec.limits.max_tool_calls = 0,
            2 => spec.limits.max_model_calls = 1,
            3 => spec.limits.max_tool_input_bytes = 2,
            4 => spec.limits.max_context_bytes = 1,
            _ => {}
        }
        let mut events = Vec::new();
        let outcome = runtime
            .run_with_tools(
                &spec,
                &shell_provider(json!({"command":"touch bad","cwd":"."})),
                &tools,
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        let expected = match mode {
            0 => RunOutcome::PolicyDenied {
                rule: PolicyRule::ToolUnavailable,
            },
            1 => RunOutcome::LimitExceeded {
                limit: LimitKind::ToolCalls,
            },
            2 => RunOutcome::LimitExceeded {
                limit: LimitKind::ModelCalls,
            },
            3 => RunOutcome::LimitExceeded {
                limit: LimitKind::ToolInputBytes,
            },
            _ => RunOutcome::LimitExceeded {
                limit: LimitKind::ContextBytes,
            },
        };
        assert_eq!(outcome, expected);
        terminal(&events, &outcome);
        assert!(!fixture.0.join("bad").exists());
    }
}

#[tokio::test]
async fn malformed_fragmented_calls_fail_before_dispatch() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    let start = ProviderEvent::ToolCallStart {
        id: "id".into(),
        name: "shell.run".into(),
    };
    let finish = ProviderEvent::Finished {
        reason: FinishReason::ToolCalls,
        usage: Usage::default(),
    };
    for events in [
        vec![start.clone(), start.clone(), finish.clone()],
        vec![
            ProviderEvent::ToolCallArgumentsDelta {
                id: "unknown".into(),
                delta: "{}".into(),
            },
            finish.clone(),
        ],
        vec![
            start.clone(),
            ProviderEvent::ToolCallArgumentsDelta {
                id: "id".into(),
                delta: "{".into(),
            },
            finish.clone(),
        ],
        vec![finish],
        vec![start, done()],
    ] {
        let mut received = Vec::new();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                &fixture.spec(),
                &scripted(events),
                &ToolRegistry::with_shell().unwrap(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    received.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, RunOutcome::Failed { .. }));
        terminal(&received, &outcome);
        assert!(
            !received
                .iter()
                .any(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
        );
    }
}

#[tokio::test]
async fn output_and_serialization_bounds_terminate_commands_and_keep_metadata() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    for command in [
        "while :; do printf abcdefghijklmnopqrstuvwxyz; done",
        "dd if=/dev/zero bs=4096 count=1 2>/dev/null",
    ] {
        let mut spec = fixture.spec();
        spec.limits.max_tool_output_bytes = 1024;
        let mut events = Vec::new();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                &spec,
                &shell_provider(json!({"command":command,"cwd":".","max_output_bytes":1024})),
                &ToolRegistry::with_shell().unwrap(),
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
                limit: LimitKind::ToolOutputBytes
            }
        );
        terminal(&events, &outcome);
        let result = tool_result(&events);
        assert!(serde_json::to_vec(result).unwrap().len() <= 1024);
        assert!(result.shell.as_ref().unwrap().stdout_truncated);
    }
}

async fn wait_file(path: &std::path::Path) -> String {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(text) = fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                return text;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("process readiness file")
}
async fn assert_gone(pid: u32) {
    let pid = rustix::process::Pid::from_raw(pid as i32).unwrap();
    timeout(Duration::from_secs(2), async {
        while rustix::process::test_kill_process(pid).is_ok() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned process must be gone");
}

#[tokio::test]
async fn cancellation_kills_term_ignoring_children_before_terminal_delivery() {
    let fixture = Fixture::new();
    let (sdk, exporter) = sdk();
    let cancellation = CancellationToken::new();
    let child_file = fixture.0.join("child.pid");
    let provider = shell_provider(
        json!({"command":"trap '' TERM; /bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' & echo $! > child.pid; wait", "cwd":"."}),
    );
    let child_pid = Arc::new(Mutex::new(None));
    let captured = child_pid.clone();
    let cancel_task = async {
        let pid: u32 = wait_file(&child_file).await.trim().parse().unwrap();
        *captured.lock().unwrap() = Some(pid);
        cancellation.cancel();
    };
    let mut events = Vec::new();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let tools = ToolRegistry::with_shell().unwrap();
    let mut sink = |e: &RunEvent| {
        events.push(e.clone());
        Ok(())
    };
    let spec = fixture.spec();
    let (outcome, ()) = tokio::join!(
        runtime.run_with_tools(&spec, &provider, &tools, &cancellation, &mut sink),
        cancel_task
    );
    let outcome = outcome.unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);
    terminal(&events, &outcome);
    assert_eq!(tool_result(&events).status, ToolStatus::Cancelled);
    let pid = child_pid.lock().unwrap().unwrap();
    assert_gone(pid).await;
    for event in &events {
        if let EventKind::ShellStarted { process_id, .. } = event.kind {
            assert_gone(process_id).await;
        }
    }
    assert!(
        exporter
            .get_finished_spans()
            .unwrap()
            .iter()
            .all(|span| span.status == Status::Unset)
    );
}

#[tokio::test]
async fn timeout_and_success_both_cleanup_background_children_with_closed_pipes() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    for (command, timed_out) in [
        (
            "sleep 60 </dev/null >/dev/null 2>&1 & echo $! > child.pid; wait",
            true,
        ),
        (
            "sleep 60 </dev/null >/dev/null 2>&1 & echo $! > child.pid; exit 0",
            false,
        ),
    ] {
        let _ = fs::remove_file(fixture.0.join("child.pid"));
        let mut events = Vec::new();
        let cancel = CancellationToken::new();
        let mut sink = |e: &RunEvent| {
            if !timed_out && matches!(e.kind, EventKind::ToolFinished { .. }) {
                cancel.cancel();
            }
            events.push(e.clone());
            Ok(())
        };
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                &fixture.spec(),
                &shell_provider(json!({"command":command,"cwd":".","timeout_ms":100})),
                &ToolRegistry::with_shell().unwrap(),
                &cancel,
                &mut sink,
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            if timed_out {
                RunOutcome::TimedOut
            } else {
                RunOutcome::Cancelled
            }
        );
        terminal(&events, &outcome);
        assert_eq!(
            tool_result(&events).status,
            if timed_out {
                ToolStatus::TimedOut
            } else {
                ToolStatus::Completed
            }
        );
        let pid: u32 = wait_file(&fixture.0.join("child.pid"))
            .await
            .trim()
            .parse()
            .unwrap();
        assert_gone(pid).await;
    }
}

#[tokio::test]
async fn cancellation_at_admission_spawn_and_tool_completion_has_one_outcome() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    for phase in ["before", "spawn", "finish"] {
        let cancel = CancellationToken::new();
        if phase == "before" {
            cancel.cancel();
        }
        let mut events = Vec::new();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                &fixture.spec(),
                &shell_provider(json!({"command":"true","cwd":"."})),
                &ToolRegistry::with_shell().unwrap(),
                &cancel,
                &mut |e: &RunEvent| {
                    if (phase == "spawn" && matches!(e.kind, EventKind::ShellStarted { .. }))
                        || (phase == "finish" && matches!(e.kind, EventKind::ToolFinished { .. }))
                    {
                        cancel.cancel();
                    }
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome, RunOutcome::Cancelled);
        terminal(&events, &outcome);
        if phase == "before" {
            assert_eq!(events.len(), 2);
        }
    }
}

#[tokio::test]
async fn event_failure_after_spawn_waits_for_cleanup_and_preserves_terminal_space() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    for failure in [false, true] {
        let mut spec = fixture.spec();
        if !failure {
            spec.limits.max_events = 6;
        }
        let mut events = Vec::new();
        let mut pid = None;
        let provider = scripted(vec![
            ProviderEvent::ToolCallStart {
                id: "call".into(),
                name: "shell.run".into(),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: "call".into(),
                delta: json!({"command":"sleep 60","cwd":"."}).to_string(),
            },
            ProviderEvent::Finished {
                reason: FinishReason::ToolCalls,
                usage: Usage::default(),
            },
        ]);
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                &spec,
                &provider,
                &ToolRegistry::with_shell().unwrap(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    if let EventKind::ShellStarted { process_id, .. } = e.kind {
                        pid = Some(process_id);
                        if failure {
                            return Err(SinkError::Capacity);
                        }
                    }
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RunOutcome::LimitExceeded {
                limit: if failure {
                    LimitKind::TraceBytes
                } else {
                    LimitKind::Events
                }
            }
        );
        terminal(&events, &outcome);
        assert_eq!(tool_result(&events).status, ToolStatus::EventSinkFailed);
        if let Some(pid) = pid {
            assert_gone(pid).await;
        }
    }
}

#[tokio::test]
async fn native_shell_content_is_opt_in_while_metadata_remains_available() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    let spec = fixture.spec();
    let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
    let provider = RoundTrip::new(&["printf secret-shell-value"], &["secret-shell-value"]);
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            &spec,
            &provider,
            &ToolRegistry::with_shell().unwrap(),
            &CancellationToken::new(),
            &mut trace,
        )
        .await
        .unwrap();
    assert!(outcome.is_completed());
    let text = String::from_utf8(trace.into_inner()).unwrap();
    assert!(!text.contains("secret-shell-value"));
    let records: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let start = records
        .iter()
        .find(|e| e["type"] == "tool.started")
        .unwrap();
    assert!(start["call"]["arguments"].is_null());
    let end = records
        .iter()
        .find(|e| e["type"] == "tool.finished")
        .unwrap();
    assert!(end["result"]["shell"]["stdout"].is_null());
    assert_eq!(end["result"]["shell"]["exit_code"], 0);
}

#[test]
fn provider_and_exporter_credentials_do_not_reach_shell_environment() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "environment_child", "--nocapture"])
        .env("PABLO_TEST_ENV_CHILD", "1")
        .env("AI_GATEWAY_API_KEY", "synthetic-gateway-secret")
        .env("OTEL_EXPORTER_OTLP_HEADERS", "synthetic-exporter-secret")
        .env("BASH_ENV", "/nonexistent/script")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test]
async fn environment_child() {
    if std::env::var_os("PABLO_TEST_ENV_CHILD").is_none() {
        return;
    }
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    let cancel = CancellationToken::new();
    let mut events = Vec::new();
    let provider =
        shell_provider(json!({"command":"env","cwd":".","env":{"PABLO_TASK_VALUE":"allowed"}}));
    let _ = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            &fixture.spec(),
            &provider,
            &ToolRegistry::with_shell().unwrap(),
            &cancel,
            &mut |e: &RunEvent| {
                if matches!(e.kind, EventKind::ToolFinished { .. }) {
                    cancel.cancel();
                }
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    let output = &tool_result(&events).shell.as_ref().unwrap().stdout;
    assert!(output.contains("PABLO_TASK_VALUE=allowed"));
    assert!(!output.contains("synthetic-"));
    assert!(!output.contains("AI_GATEWAY_API_KEY"));
    assert!(!output.contains("OTEL_"));
    assert!(!output.contains("BASH_ENV"));
    assert!(!output.contains("PABLO_TEST_ENV_CHILD"));
}

struct PendingProvider {
    opening: bool,
    started: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}
struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl Provider for PendingProvider {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn stream<'a>(
        &'a self,
        _: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            if self.opening {
                let _guard = DropFlag(self.dropped.clone());
                self.started.notify_one();
                std::future::pending().await
            } else {
                let stream = stream::once(async {
                    let _guard = DropFlag(self.dropped.clone());
                    self.started.notify_one();
                    std::future::pending().await
                });
                Ok(Box::pin(stream) as ProviderStream<'a>)
            }
        })
    }
}

#[tokio::test]
async fn cancellation_drops_model_opening_and_streaming_work() {
    for opening in [true, false] {
        let fixture = Fixture::new();
        let (sdk, exporter) = sdk();
        let provider = PendingProvider {
            opening,
            started: Arc::new(tokio::sync::Notify::new()),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let cancellation = CancellationToken::new();
        let cancel = async {
            provider.started.notified().await;
            cancellation.cancel();
        };
        let runtime = Runtime::new(telemetry::tracer(&sdk));
        let spec = fixture.spec();
        let tools = ToolRegistry::default();
        let mut events = Vec::new();
        let mut sink = |e: &RunEvent| {
            events.push(e.clone());
            Ok(())
        };
        let (outcome, ()) = timeout(Duration::from_secs(1), async {
            tokio::join!(
                runtime.run_with_tools(&spec, &provider, &tools, &cancellation, &mut sink),
                cancel
            )
        })
        .await
        .unwrap();
        let outcome = outcome.unwrap();
        assert_eq!(outcome, RunOutcome::Cancelled);
        terminal(&events, &outcome);
        assert!(provider.dropped.load(Ordering::SeqCst));
        assert!(
            exporter
                .get_finished_spans()
                .unwrap()
                .iter()
                .all(|s| s.status == Status::Unset)
        );
    }
}

#[tokio::test]
async fn contained_cwd_and_non_utf8_output_are_reported_explicitly() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    fs::create_dir(fixture.0.join("inside")).unwrap();
    fs::write(fixture.0.join("inside/evidence"), "nested").unwrap();
    let cancel = CancellationToken::new();
    let mut events = Vec::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            &fixture.spec(),
            &shell_provider(json!({
                "command":"cat evidence; printf '\\377\\376' >&2", "cwd":fixture.0.join("inside")
            })),
            &ToolRegistry::with_shell().unwrap(),
            &cancel,
            &mut |e: &RunEvent| {
                if matches!(e.kind, EventKind::ToolFinished { .. }) {
                    cancel.cancel();
                }
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);
    let shell = tool_result(&events).shell.as_ref().unwrap();
    assert_eq!(shell.stdout, "nested");
    assert!(!shell.stdout_lossy);
    assert_eq!(shell.stderr, "\u{fffd}\u{fffd}");
    assert!(shell.stderr_lossy);
}

#[tokio::test]
async fn event_budget_stays_bounded_across_model_tool_model_transitions() {
    let fixture = Fixture::new();
    let (sdk, _) = sdk();
    for max_events in 4..=10 {
        let mut spec = fixture.spec();
        spec.limits.max_events = max_events;
        let mut provider = RoundTrip::new(&["printf ok"], &["ok"]);
        provider.first = vec![
            ProviderEvent::ToolCallStart {
                id: "call".into(),
                name: "shell.run".into(),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: "call".into(),
                delta: json!({"command":"printf ok","cwd":"."}).to_string(),
            },
            ProviderEvent::Finished {
                reason: FinishReason::ToolCalls,
                usage: Usage::default(),
            },
        ];
        let mut events = Vec::new();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                &spec,
                &provider,
                &ToolRegistry::with_shell().unwrap(),
                &CancellationToken::new(),
                &mut |e: &RunEvent| {
                    events.push(e.clone());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert!(
            events.len() as u64 <= max_events,
            "event budget {max_events}"
        );
        terminal(&events, &outcome);
        if max_events == 10 {
            assert!(outcome.is_completed());
        } else {
            assert_eq!(
                outcome,
                RunOutcome::LimitExceeded {
                    limit: LimitKind::Events
                }
            );
        }
    }
}
