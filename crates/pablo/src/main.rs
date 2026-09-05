mod config;

use std::{
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    process::ExitCode,
    time::Duration,
};

use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};
use pablo_core::{
    CancellationToken, EventKind, EventSink, JsonlSink, Provider, RunEvent, RunOutcome, Runtime,
    ScriptedProvider, SinkError, ToolRegistry, gateway::GatewayProvider, telemetry,
};

const HELP: &str = "pablo — headless Rust runtime (C1.2a preview)\n\nUsage:\n  pablo run \"TASK\" [--workspace PATH] [--model ID] [--no-shell]\n                 [--env-file PATH] [--timeout SECONDS]\n                 [--trace PATH] [--capture-content]\n  pablo demo [--trace PATH] [--capture-content]\n  pablo --version\n  pablo --help\n\nRun sends one task to Vercel AI Gateway using direct HTTP.\nDefault model: openai/gpt-4.1-mini. Workspace: current directory.\nReads AI_GATEWAY_API_KEY (alias VERCEL_AI_GATEWAY) from the environment\nor .env in the invoking directory.\nShell is enabled for run; use --no-shell for text only. Ctrl-C cancels and cleans up.\nEach invocation is a fresh task (no saved chat history).\nDemo is offline. Trace files must be new; native content is off by default.\nNo network telemetry exporter is enabled.\n";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match execute().await {
        Ok(code) => code,
        Err(message) => {
            eprintln!("pablo: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn execute() -> Result<ExitCode, String> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().unwrap_or_else(|| "--help".into());
    if command == "--help" && args.len() == 0 {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    if command == "--version" && args.len() == 0 {
        println!(
            "pablo {} (OTel mapping {}; GenAI {})",
            env!("CARGO_PKG_VERSION"),
            telemetry::MAPPING_VERSION,
            telemetry::SEMCONV_REVISION
        );
        return Ok(ExitCode::SUCCESS);
    }
    let options = config::Options::parse(command, args)?;
    let spec = options.spec()?;
    let provider: Box<dyn Provider> = if options.live {
        if let Some(endpoint) = std::env::var_os("PABLO_FIXTURE_ENDPOINT") {
            Box::new(GatewayProvider::local_fixture(
                endpoint.to_str().ok_or("invalid fixture endpoint")?,
            )?)
        } else {
            Box::new(GatewayProvider::vercel(&config::gateway_key(
                options.env_file.as_deref(),
            )?)?)
        }
    } else {
        Box::new(ScriptedProvider::text(["Hello ", "from ", "pablo.\n"]))
    };
    let tools = if options.live && !options.no_shell {
        ToolRegistry::with_shell().map_err(|_| "cannot initialize shell tool")?
    } else {
        ToolRegistry::default()
    };
    let mut native_trace = match &options.trace_path {
        Some(path) => {
            let mut open = OpenOptions::new();
            open.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.mode(0o600);
            }
            let file = open.open(path).map_err(
                |_| "cannot create trace file; check its parent directory and use a new path",
            )?;
            Some(JsonlSink::new(BufWriter::new(file), &spec).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let sdk = SdkTracerProvider::builder()
        .with_resource(
            Resource::builder_empty()
                .with_service_name("pablo")
                .with_attribute(opentelemetry::KeyValue::new(
                    "service.version",
                    env!("CARGO_PKG_VERSION"),
                ))
                .build(),
        )
        .build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let cancellation = CancellationToken::new();
    // Register before starting work. Poll signals alongside the same run future;
    // never drop a running shell future on Ctrl-C.
    #[cfg(unix)]
    let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "cannot register Ctrl-C handler")?;
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    let mut needs_newline = false;
    if options.live {
        writeln!(
            stderr,
            "pablo: running with {} (Ctrl-C to cancel)",
            spec.model
        )
        .map_err(|_| "cannot write terminal output")?;
    }
    let mut sink = |event: &RunEvent| -> Result<(), SinkError> {
        if let Some(trace) = native_trace.as_mut() {
            trace.emit(event)?;
        }
        match &event.kind {
            EventKind::TextDelta { text } => {
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
                if !text.is_empty() {
                    needs_newline = !text.ends_with('\n');
                }
            }
            EventKind::ToolStarted { call } if options.live => {
                // JSON quoting makes control characters in commands visible.
                writeln!(
                    stderr,
                    "\npablo: shell {} (cwd {})",
                    call.arguments["command"], call.arguments["cwd"]
                )?;
            }
            EventKind::ToolFinished { result, .. } if options.live => {
                if let Some(shell) = &result.shell {
                    writeln!(
                        stderr,
                        "pablo: shell {:?}, exit {:?}; {} stdout / {} stderr bytes",
                        result.status,
                        shell.exit_code,
                        shell.stdout.len(),
                        shell.stderr.len()
                    )?;
                } else {
                    writeln!(stderr, "pablo: shell {:?}", result.status)?;
                }
            }
            _ => {}
        }
        Ok(())
    };
    let result = {
        let run =
            runtime.run_with_tools(&spec, provider.as_ref(), &tools, &cancellation, &mut sink);
        tokio::pin!(run);
        loop {
            #[cfg(unix)]
            let interrupted = signals.recv();
            #[cfg(not(unix))]
            let interrupted = tokio::signal::ctrl_c();
            tokio::select! {
                biased;
                _ = interrupted => { cancellation.cancel(); }
                result = &mut run => break result,
            }
        }
    };
    if sdk.shutdown_with_timeout(Duration::from_secs(2)).is_err() {
        eprintln!("pablo: telemetry shutdown failed");
    }
    if options.live && needs_newline {
        writeln!(stdout).map_err(|_| "cannot write terminal output")?;
    }
    let outcome = result.map_err(|e| e.to_string())?;
    match outcome {
        RunOutcome::Completed { .. } => Ok(ExitCode::SUCCESS),
        RunOutcome::Cancelled => {
            eprintln!("pablo: cancelled; owned work cleaned up");
            Ok(ExitCode::from(130))
        }
        RunOutcome::Failed { code, delivery } => {
            Err(format!("run failed: {code:?} ({delivery:?})"))
        }
        RunOutcome::LimitExceeded { limit } => Err(format!("run limit exceeded: {limit:?}")),
        RunOutcome::PolicyDenied { rule } => Err(format!("run denied: {rule:?}")),
        RunOutcome::TimedOut => Err("run timed out; owned work cleaned up".into()),
    }
}
