mod acp;
mod config;
mod deployment;
mod otel;

use std::{
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    process::ExitCode,
};

use pablo_core::{
    CancellationToken, EventKind, EventSink, JsonlSink, Provider, RunError, RunEvent, RunOutcome,
    Runtime, ScriptedProvider, SinkError, TaskErrorCode, TaskResult, gateway::GatewayProvider,
    telemetry,
};

const HELP: &str = "pablo — headless Rust runtime\n\nUsage:\n  pablo \"TASK\" [RUN OPTIONS...]\n  pablo run \"TASK\" [--json] [--workspace PATH] [--model ID] [--no-shell]\n                 [--env-file PATH] [--timeout SECONDS] [--tool-timeout SECONDS]\n                 [--max-tool-calls N] [--max-model-calls N]\n                 [--max-total-tokens N] [--max-cost-microusd N]\n                 [--trace PATH] [--capture-content]\n  pablo acp --stdio [--model ID] [--no-shell] [--env-file PATH]\n                  [--max-tool-calls N] [--max-model-calls N]\n                 [--max-total-tokens N] [--max-cost-microusd N]\n                  [--timeout SECONDS] [--tool-timeout SECONDS] [--trace PATH] [--capture-content]\n  pablo demo [--trace PATH] [--capture-content]\n  CLI trace context: --traceparent VALUE [--tracestate VALUE] (run/demo)\n  pablo --version\n  pablo --help\n\n--json writes one task envelope plus LF; output remains a string.\nRun sends one task to Vercel AI Gateway using direct HTTP.\nDefault model: google/gemini-3.8-flash. Workspace: current directory.\nReads AI_GATEWAY_API_KEY (alias VERCEL_AI_GATEWAY) from the environment\nor .env in the invoking directory.\nShell and filesystem reads are enabled for run/ACP; use --no-shell and --no-filesystem for text only. Use --allow-write to enable revision-checked write/edit; --policy PATH sets tool, launcher and filesystem-root rules. Ctrl-C cancels and cleans up.\nDefaults: 3600 seconds per run, 900 seconds per shell call. Timeouts accept 1–86400.\nModel and tool call counts are unlimited by default; use --max-*-calls to cap them.\nHard aggregate token/cost ceilings require an attested adapter; the live gateway currently rejects them before delivery.\nEach run/session is a fresh task (no saved chat history).\nACP reuses its process across successive sessions; use {session_id} in --trace paths for separate files.\nDemo is offline. Trace files must be new; native content is off by default.\nNetwork telemetry is off by default; set OTEL_TRACES_EXPORTER=otlp for OTLP/HTTP Protobuf.\n";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let json = args.first().is_none_or(|c| {
        c != "config" && c != "acp" && c != "demo" && c != "--help" && c != "--version"
    }) && args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    let mut stage = TaskErrorCode::InvalidArguments;
    match execute(args, &mut stage).await {
        Ok(code) => code,
        Err(message) => {
            if json
                && TaskResult::rejected(stage)
                    .write_json(io::stdout().lock(), 0)
                    .is_err()
            {
                return ExitCode::FAILURE;
            }
            eprintln!("pablo: {message}");
            ExitCode::from(2)
        }
    }
}

async fn execute(
    args: Vec<std::ffi::OsString>,
    stage: &mut TaskErrorCode,
) -> Result<ExitCode, String> {
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "--help".into());
    if command == "config" {
        deployment::inspect(args.collect())?;
        return Ok(ExitCode::SUCCESS);
    }
    if command == "--help" && args.len() == 0 {
        print!("{HELP}{}", deployment::HELP);
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
    if options.acp {
        return acp::serve(options).await;
    }
    *stage = TaskErrorCode::InvalidConfiguration;
    let spec = options.spec()?;
    let tools = options.tools()?;
    let provider: Box<dyn Provider> = if options.live {
        if let Some(endpoint) = std::env::var_os("PABLO_FIXTURE_ENDPOINT") {
            Box::new(GatewayProvider::local_fixture(
                endpoint.to_str().ok_or("invalid fixture endpoint")?,
            )?)
        } else {
            *stage = TaskErrorCode::CredentialUnavailable;
            Box::new(GatewayProvider::vercel(&config::gateway_key(
                options.env_file.as_deref(),
            )?)?)
        }
    } else {
        Box::new(ScriptedProvider::text(["Hello ", "from ", "pablo.\n"]))
    };
    *stage = TaskErrorCode::TraceSetupFailed;
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
    *stage = TaskErrorCode::InvalidConfiguration;
    let sdk = otel::Telemetry::new();
    let runtime = Runtime::new(telemetry::tracer(&sdk.sdk)).with_parent_context(otel::parent(
        options.traceparent.as_deref(),
        options.tracestate.as_deref(),
    ));
    let cancellation = CancellationToken::new();
    // Register before starting work. Poll signals alongside the same run future;
    // never drop a running shell future on Ctrl-C.
    #[cfg(unix)]
    let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "cannot register Ctrl-C handler")?;
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    let mut needs_newline = false;
    if options.live && !options.json {
        writeln!(
            stderr,
            "pablo: running with {} (Ctrl-C to cancel)",
            spec.model
        )
        .map_err(|_| "cannot write terminal output")?;
    }
    let mut task_result = None;
    let mut sink = |event: &RunEvent| -> Result<(), SinkError> {
        if options.json && matches!(event.kind, EventKind::RunFinished { .. }) {
            task_result = TaskResult::from_terminal(event);
        }
        if let Some(trace) = native_trace.as_mut() {
            trace.emit(event)?;
        }
        if options.json {
            return Ok(());
        }
        match &event.kind {
            EventKind::TextDelta { text } => {
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
                if !text.is_empty() {
                    needs_newline = !text.ends_with('\n');
                }
            }
            EventKind::ToolStarted { call } if options.live && call.name == "shell.run" => {
                // JSON quoting makes control characters in commands visible.
                writeln!(
                    stderr,
                    "\npablo: shell {} (cwd {})",
                    call.arguments["command"], call.arguments["cwd"]
                )?;
            }
            EventKind::ToolStarted { call } if options.live => {
                writeln!(stderr, "\npablo: {}", call.name)?;
            }
            EventKind::ToolFinished { name, result, .. } if options.live => {
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
                    writeln!(stderr, "pablo: {} {:?}", name, result.status)?;
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
    sdk.shutdown().await;
    let (outcome, delivery_failed) = match result {
        Ok(outcome) => (outcome, false),
        Err(RunError::EventDelivery { outcome, .. }) => (*outcome, true),
        Err(error) => return Err(error.to_string()),
    };
    if options.json {
        let Some(task) = task_result else {
            eprintln!("pablo: terminal result unavailable");
            return Ok(ExitCode::FAILURE);
        };
        if task
            .write_json(stdout.lock(), spec.limits.max_output_bytes)
            .is_err()
        {
            eprintln!("pablo: cannot deliver task result");
            return Ok(ExitCode::FAILURE);
        }
    } else if options.live && needs_newline && writeln!(stdout).is_err() {
        return Ok(ExitCode::FAILURE);
    }
    if delivery_failed {
        eprintln!("pablo: terminal event delivery failed");
        return Ok(ExitCode::FAILURE);
    }
    match outcome {
        RunOutcome::Completed { .. } => Ok(ExitCode::SUCCESS),
        RunOutcome::Cancelled => {
            if !options.json {
                eprintln!("pablo: cancelled; owned work cleaned up");
            }
            Ok(ExitCode::from(130))
        }
        outcome => {
            if !options.json {
                match outcome {
                    RunOutcome::Failed { code, delivery } => {
                        eprintln!("pablo: run failed: {code:?} ({delivery:?})")
                    }
                    RunOutcome::LimitExceeded { limit } => {
                        eprintln!("pablo: run limit exceeded: {limit:?}")
                    }
                    RunOutcome::PolicyDenied { rule } => eprintln!("pablo: run denied: {rule:?}"),
                    _ => eprintln!("pablo: run timed out; owned work cleaned up"),
                }
            }
            Ok(ExitCode::FAILURE)
        }
    }
}
