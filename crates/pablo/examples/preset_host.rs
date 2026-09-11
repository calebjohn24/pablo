//! Reference embedding host for the versioned presets. Runs Runtime directly.
//! The executable's configuration/ownership wiring is reused as source modules;
//! these modules are not a published Rust API. See presets/v1/README.md.
#[allow(dead_code)]
#[path = "../src/acp.rs"]
mod acp;
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/deployment.rs"]
mod deployment;
#[allow(dead_code)]
#[path = "../src/otel.rs"]
mod otel;

use pablo_core::{
    CancellationToken, EventKind, EventSink, JsonlSink, RunEvent, Runtime, SinkError, TaskResult,
    telemetry,
};
use std::io::BufWriter;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = config::Options::parse("run".into(), std::env::args_os().skip(1))?;
    let prepared = options
        .prepare_run(None, None, Some(uuid::Uuid::new_v4().to_string()))?
        .ok_or("preset_host requires a configured deployment")?;
    let bootstrap = options.deployment.as_ref().unwrap();
    let factory = acp::supervisor::factory::RootFactory::configured(&options, Some(&prepared))?
        .ok_or("preset_host requires children.enabled")?;
    let root_id = factory.root.root_run_id().to_owned();
    let secrets = deployment::Secrets::read(&prepared, bootstrap)?;
    let provider = secrets.provider(bootstrap)?;
    prepared.preflight(provider.as_ref())?;
    let mut trace = prepared
        .create_trace_file()?
        .map(|file| JsonlSink::for_tree(BufWriter::new(file), prepared.spec(), root_id.clone()))
        .transpose()?;
    let sdk = otel::Telemetry::configured(&prepared, secrets.headers.as_ref())?;
    let runtime = Runtime::new(telemetry::tracer(&sdk.sdk)).with_deployment(prepared.deployment());
    let cancellation = CancellationToken::new();
    let mut terminal = None;
    let mut sink = |event: &RunEvent| -> Result<(), SinkError> {
        if event.run_id == root_id && matches!(event.kind, EventKind::RunFinished { .. }) {
            terminal = TaskResult::from_terminal(event);
        }
        if let Some(trace) = &mut trace {
            trace.emit(event)?;
        }
        Ok(())
    };
    // Poll interruption beside the owned run; cancellation never drops cleanup.
    let result = {
        let run = factory.run(runtime, provider.as_ref(), &cancellation, &mut sink);
        tokio::pin!(run);
        tokio::select! {
          result = &mut run => result,
          signal = tokio::signal::ctrl_c() => {
              cancellation.cancel();
              let result = run.await;
              match signal {
                  Ok(()) => result,
                  Err(_) => Err("cannot observe host interruption".into()),
              }
          }
        }
    };
    sdk.shutdown().await;
    result??;
    println!(
        "{}",
        serde_json::to_string(&terminal.ok_or("missing root terminal")?)?
    );
    Ok(())
}
