#![cfg(unix)]
use futures_util::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::{InMemorySpanExporter, Sampler, SdkTracerProvider};
use pablo_core::{
    deployment::{self, ConfigInput, CredentialInputs, CredentialReadError, ResolveRequest},
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    *,
};
use serde_json::json;
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
struct Credentials(AtomicUsize);
impl CredentialInputs for Credentials {
    fn environment(&self, _: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        panic!("undeclared environment lookup")
    }
    fn host(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        assert_eq!(name, "synthetic");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Some(b"synthetic-mcp-token".to_vec()))
    }
}
struct ReadsResult(AtomicUsize);
impl Provider for ReadsResult {
    fn name(&self) -> &'static str {
        "synthetic"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            assert_eq!(request.tools.len(), 1);
            assert_eq!(request.tools[0].name, "mcp/local/read_evidence");
            let events = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![
                    ProviderEvent::ToolCallStart {
                        id: "read-1".into(),
                        name: request.tools[0].name.clone(),
                    },
                    ProviderEvent::ToolCallArgumentsDelta {
                        id: "read-1".into(),
                        delta: "{}".into(),
                    },
                    ProviderEvent::Finished {
                        reason: FinishReason::ToolCalls,
                        usage: Usage::default(),
                    },
                ]
            } else {
                let Message::Tool { result, .. } = request.messages.last().unwrap() else {
                    panic!("missing actual tool result")
                };
                let content = result.mcp.as_ref().unwrap().content.as_ref().unwrap();
                assert_eq!(
                    content.structured.as_ref().unwrap()["text"],
                    "new synthetic evidence"
                );
                vec![
                    ProviderEvent::TextDelta("Consumed the actual MCP result.".into()),
                    ProviderEvent::Finished {
                        reason: FinishReason::Stop,
                        usage: Usage::default(),
                    },
                ]
            };
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}
#[tokio::test]
#[ignore = "requires tests/fixtures/mcp/requirements.txt in .pablo/mcp-fixture-venv"]
async fn independent_stdio_tool_uses_ordinary_runtime_and_private_host_credentials() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let cwd = std::env::temp_dir().join(format!("pablo-mcp-runtime-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    std::fs::write(cwd.join("evidence.txt"), "new synthetic evidence").unwrap();
    let mut request = ResolveRequest::new(cwd.clone(), "unused");
    request
        .path_bindings
        .insert("workspace".into(), cwd.clone());
    request.entry = ConfigInput::Document(
        json!({"schema_version":1,"options":{"shell":{"enabled":false},"filesystem":{"enabled":false},"mcp":{"servers":{"local":{"transport":"stdio","command":root.join(".pablo/mcp-fixture-venv/bin/python"),"args":[root.join("tests/fixtures/mcp/server.py")],"env":{"FIXTURE_TOKEN":"local-token"}}}}},"credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"must-not-resolve"}]},"local-token":{"consumer":"mcp.env","sources":[{"kind":"host","name":"synthetic"}]}}}),
    );
    let prepared = deployment::resolve(request)
        .unwrap()
        .prepare_run(deployment::RunInput {
            input: "Read synthetic evidence".into(),
            workspace: None,
            session_id: None,
        })
        .unwrap();
    let credentials = Credentials(AtomicUsize::new(0));
    let cancellation = CancellationToken::new();
    let tools = prepared
        .tools_with_mcp(
            &credentials,
            tokio::time::Instant::now() + Duration::from_secs(10),
            &cancellation,
        )
        .await
        .unwrap();
    assert_eq!(credentials.0.load(Ordering::SeqCst), 1);
    let exporter = InMemorySpanExporter::default();
    let sdk = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_simple_exporter(exporter.clone())
        .build();
    let provider = ReadsResult(AtomicUsize::new(0));
    let mut events = Vec::new();
    let mut trace = JsonlSink::new(Vec::new(), prepared.spec()).unwrap();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            prepared.spec(),
            &provider,
            &tools,
            &cancellation,
            &mut |e: &RunEvent| {
                trace.emit(e)?;
                if matches!(e.kind, EventKind::RunFinished { .. }) {
                    let pid = std::fs::read_to_string(cwd.join("pid"))
                        .unwrap()
                        .parse::<i32>()
                        .unwrap();
                    assert_eq!(
                        rustix::process::test_kill_process_group(
                            rustix::process::Pid::from_raw(pid).unwrap()
                        ),
                        Err(rustix::io::Errno::SRCH)
                    );
                }
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert!(outcome.is_completed());
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::ShellStarted { .. }))
    );
    let native = String::from_utf8(trace.into_inner()).unwrap();
    for private in ["new synthetic evidence", "synthetic-mcp-token"] {
        assert!(!native.contains(private));
    }
    assert!(native.contains("text_blocks"));
    assert_eq!(provider.0.load(Ordering::SeqCst), 2);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::ToolFinished { .. }))
            .count(),
        1
    );
    let spans = exporter.get_finished_spans().unwrap();
    let logical: Vec<_> = spans
        .iter()
        .filter(|s| s.name.starts_with("execute_tool"))
        .collect();
    assert_eq!(logical.len(), 1);
    assert!(
        logical[0]
            .attributes
            .iter()
            .any(|a| a.key.as_str() == "mcp.method.name" && a.value.as_str() == "tools/call")
    );
    assert_eq!(logical[0].span_kind, opentelemetry::trace::SpanKind::Client);
    assert!(
        Runtime::new(telemetry::tracer(&sdk))
            .run_with_tools(
                prepared.spec(),
                &provider,
                &tools,
                &cancellation,
                &mut |_: &RunEvent| Ok(())
            )
            .await
            .is_err()
    );
    assert_eq!(provider.0.load(Ordering::SeqCst), 2);
    tools.close().await.unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
#[ignore = "requires the pinned independent Python fixture environment"]
async fn required_startup_failure_joins_prior_servers_and_optional_failure_is_recorded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    for required in [true, false] {
        let cwd = std::env::temp_dir().join(format!("pablo-mcp-startup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.canonicalize().unwrap();
        for path in ["a", "b"] {
            std::fs::create_dir(cwd.join(path)).unwrap();
        }
        let mut request = ResolveRequest::new(cwd.clone(), "unused");
        request
            .path_bindings
            .insert("workspace".into(), cwd.clone());
        request.entry = ConfigInput::Document(
            json!({"schema_version":1,"options":{"shell":{"enabled":false},"filesystem":{"enabled":false},"mcp":{"servers":{
            "a_good":{"transport":"stdio","command":root.join(".pablo/mcp-fixture-venv/bin/python"),"args":[root.join("tests/fixtures/mcp/server.py")],"cwd":{"base":"workspace","path":"a"},"env":{"FIXTURE_TOKEN":"token"}},
            "b_bad":{"transport":"stdio","command":root.join(".pablo/mcp-fixture-venv/bin/python"),"args":[root.join("tests/fixtures/mcp/adversarial.py"),"wrong_version"],"cwd":{"base":"workspace","path":"b"},"required":required}
        }}},"credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"unused"}]},"token":{"consumer":"mcp.env","sources":[{"kind":"host","name":"synthetic"}]}}}),
        );
        let prepared = deployment::resolve(request)
            .unwrap()
            .prepare_run(deployment::RunInput {
                input: "synthetic".into(),
                workspace: None,
                session_id: None,
            })
            .unwrap();
        let credentials = Credentials(AtomicUsize::new(0));
        let result = prepared
            .tools_with_mcp(
                &credentials,
                tokio::time::Instant::now() + Duration::from_secs(10),
                &CancellationToken::new(),
            )
            .await;
        match result {
            Ok(tools) => {
                assert!(!required);
                assert_eq!(tools.descriptors().len(), 1);
                assert_eq!(
                    tools.mcp_omissions().get("b_bad"),
                    Some(&"config_mcp_startup")
                );
                tools.close().await.unwrap();
            }
            Err(error) => {
                assert!(required);
                assert_eq!(error.code, "config_mcp_startup");
            }
        }
        for path in ["a", "b"] {
            let pid = std::fs::read_to_string(cwd.join(path).join("pid"))
                .unwrap()
                .parse::<i32>()
                .unwrap();
            assert_eq!(
                rustix::process::test_kill_process_group(
                    rustix::process::Pid::from_raw(pid).unwrap()
                ),
                Err(rustix::io::Errno::SRCH)
            );
        }
        std::fs::remove_dir_all(cwd).unwrap();
    }
}
