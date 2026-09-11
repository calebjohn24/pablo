//! Local synthetic MCP measurements through the public Rust embedding path.
use futures::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use pablo_core::{
    deployment::{self, ConfigInput, CredentialInputs, CredentialReadError, ResolveRequest},
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    *,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
struct Credentials;
impl CredentialInputs for Credentials {
    fn environment(&self, _: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        Err(CredentialReadError)
    }
    fn host(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        if name != "synthetic" {
            return Err(CredentialReadError);
        }
        Ok(Some(b"synthetic-mcp-token".to_vec()))
    }
}
struct Model(AtomicUsize);
impl Provider for Model {
    fn name(&self) -> &'static str {
        "synthetic"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            assert_eq!(request.tools.len(), 1);
            let events = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![
                    ProviderEvent::ToolCallStart {
                        id: "read".into(),
                        name: request.tools[0].name.clone(),
                    },
                    ProviderEvent::ToolCallArgumentsDelta {
                        id: "read".into(),
                        delta: "{}".into(),
                    },
                    ProviderEvent::Finished {
                        reason: FinishReason::ToolCalls,
                        usage: Usage::default(),
                    },
                ]
            } else {
                let Message::Tool { result, .. } = request.messages.last().unwrap() else {
                    panic!("missing result")
                };
                assert_eq!(
                    result
                        .mcp
                        .as_ref()
                        .unwrap()
                        .content
                        .as_ref()
                        .unwrap()
                        .structured
                        .as_ref()
                        .unwrap()["text"],
                    "measurement evidence"
                );
                vec![
                    ProviderEvent::TextDelta("Consumed evidence.".into()),
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
fn stats(values: &[f64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({"n":sorted.len(),"min":sorted[0],"p50":sorted[(sorted.len() as f64*0.5).ceil() as usize-1],"p95":sorted[(sorted.len() as f64*0.95).ceil() as usize-1],"max":sorted.last()})
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        3,
        "mcp_measure ABSOLUTE_PYTHON ABSOLUTE_SERVER SAMPLES"
    );
    let python = PathBuf::from(&args[0]);
    let server = PathBuf::from(&args[1]);
    assert!(python.is_absolute() && server.is_absolute());
    let count: usize = args[2].parse().unwrap();
    assert!((10..=1000).contains(&count));
    let cwd = std::env::temp_dir().join(format!("pablo-mcp-measure-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    std::fs::write(cwd.join("evidence.txt"), "measurement evidence").unwrap();
    let mut request = ResolveRequest::new(cwd.clone(), "unused");
    request
        .path_bindings
        .insert("workspace".into(), cwd.clone());
    request.entry = ConfigInput::Document(
        json!({"schema_version":1,"options":{"shell":{"enabled":false},"filesystem":{"enabled":false},"mcp":{"servers":{"local":{"transport":"stdio","command":python,"args":[server],"env":{"FIXTURE_TOKEN":"token"}}}}},"credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"unused"}]},"token":{"consumer":"mcp.env","sources":[{"kind":"host","name":"synthetic"}]}}}),
    );
    let prepared = deployment::resolve(request)
        .unwrap()
        .prepare_run(deployment::RunInput {
            input: "Read the synthetic evidence.".into(),
            workspace: None,
            session_id: None,
        })
        .unwrap();
    let sdk = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let mut startup = Vec::new();
    let mut run = Vec::new();
    let mut tool = Vec::new();
    for i in 0..count + 5 {
        let cancellation = CancellationToken::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let began = Instant::now();
        let tools = prepared
            .tools_with_mcp(&Credentials, deadline, &cancellation)
            .await
            .unwrap();
        let startup_ms = began.elapsed().as_secs_f64() * 1000.;
        let model = Model(AtomicUsize::new(0));
        let mut tool_began = None;
        let mut tool_ms = 0.;
        let began = Instant::now();
        let outcome = runtime
            .run_with_tools(
                prepared.spec(),
                &model,
                &tools,
                &cancellation,
                &mut |event: &RunEvent| {
                    match &event.kind {
                        EventKind::ToolStarted { .. } => tool_began = Some(Instant::now()),
                        EventKind::ToolFinished { result, .. } => {
                            assert_eq!(result.status, tool::ToolStatus::Completed);
                            tool_ms = tool_began.unwrap().elapsed().as_secs_f64() * 1000.;
                        }
                        _ => {}
                    }
                    Ok(())
                },
            )
            .await
            .unwrap();
        let run_ms = began.elapsed().as_secs_f64() * 1000.;
        assert!(outcome.is_completed());
        assert_eq!(model.0.load(Ordering::SeqCst), 2);
        if i >= 5 {
            startup.push(startup_ms);
            run.push(run_ms);
            tool.push(tool_ms);
        }
    }
    let report = json!({"samples":count,"warmup":5,"method":"fresh Python SDK 2.2.0 process and admitted catalog per run; two synthetic model calls consume one actual read; tool timing includes fixture context metadata write; run timing includes joined close; config resolution and fixture setup excluded","stats":{"startup_ms":stats(&startup),"run_ms":stats(&run),"tool_ms":stats(&tool)},"raw":{"startup_ms":startup,"run_ms":run,"tool_ms":tool}});
    println!("{report}");
    std::fs::remove_dir_all(cwd).unwrap();
}
