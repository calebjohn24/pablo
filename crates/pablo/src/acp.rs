//! Stable ACP v1 adapter. The official SDK owns dispatch and JSON-RPC IDs.
use agent_client_protocol::{
    Agent, Client, ConnectionTo, Error, Lines, Responder, schema::v1 as wire,
};
use futures::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{
    CancellationToken, EventKind, EventSink, JsonlSink, LimitKind, RunError, RunEvent, RunOutcome,
    RunSpec, Runtime, SinkError, ToolRegistry, gateway::GatewayProvider, telemetry,
    tool::ToolStatus,
};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufWriter},
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

use crate::config::{self, Options};

const EXTENSION: &str = "pablo/v1";
const FRAME_BYTES: usize = 32 * 1024 * 1024;
const INPUT_BYTES: usize = 64 * 1024 * 1024;
const INPUT_MESSAGES: usize = 128;
const QUEUE_EVENTS: usize = 8;
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

type Worker = std::thread::JoinHandle<Result<RunOutcome, String>>;
#[derive(Default)]
struct State {
    initialized: bool,
    extended: bool,
    session: Option<(wire::SessionId, std::path::PathBuf)>,
    prompted: bool,
    prompt_active: bool,
    worker: Option<Worker>,
    events: Option<async_channel::Sender<RunEvent>>,
}
#[derive(Default)]
struct Written {
    count: AtomicU64,
    changed: Notify,
}

fn invalid(message: &'static str) -> Error {
    Error::invalid_params().data(message)
}
fn meta(value: Value) -> wire::Meta {
    [(EXTENSION.to_owned(), value)].into_iter().collect()
}
fn correlation(event: &RunEvent, first: u64) -> Value {
    json!({"schema_version":event.schema_version,"run_id":event.run_id,
        "session_id":event.session_id,"seq_start":first,"seq_end":event.seq,
        "timestamp_unix_micros":event.timestamp_unix_micros,
        "trace_id":event.trace_id,"span_id":event.span_id,
        "parent_span_id":event.parent_span_id,"trace_flags":event.trace_flags})
}

#[cfg(unix)]
pub async fn serve(options: Options) -> Result<ExitCode, String> {
    use std::os::fd::AsFd;
    // Nonblocking descriptors keep transport failure and signal shutdown from
    // leaving an uncancellable stdio read/write on Tokio's blocking pool.
    let input = async_io::Async::new(File::from(
        io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|_| "cannot duplicate ACP stdin")?,
    ))
    .map_err(|_| "ACP stdin must be a pipe or socket")?;
    let output = async_io::Async::new(File::from(
        io::stdout()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|_| "cannot duplicate ACP stdout")?,
    ))
    .map_err(|_| "ACP stdout must be a pipe or socket")?;
    serve_streams(options, input, output).await
}
#[cfg(not(unix))]
pub async fn serve(_options: Options) -> Result<ExitCode, String> {
    Err("ACP stdio currently requires Unix pipes".into())
}

async fn serve_streams(
    options: Options,
    input: impl futures::AsyncRead + Unpin + Send + 'static,
    output: impl futures::AsyncWrite + Unpin + Send + 'static,
) -> Result<ExitCode, String> {
    let state = Arc::new(Mutex::new(State::default()));
    let options = Arc::new(options);
    let cancellation = CancellationToken::new();
    let written = Arc::new(Written::default());
    // In addition to individual frames, bound the entire single-task connection.
    // This bounds the SDK's internal incoming queues even under request floods.
    let incoming = futures::stream::try_unfold(
        (futures::io::BufReader::new(input), 0usize, 0usize),
        |(mut input, bytes, count)| async move {
            let mut line = Vec::new();
            (&mut input)
                .take((FRAME_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .await?;
            if line.is_empty() {
                return Ok(None);
            }
            if line.len() > FRAME_BYTES
                || bytes + line.len() > INPUT_BYTES
                || count >= INPUT_MESSAGES
            {
                return Err(io::Error::other("ACP input limit exceeded"));
            }
            if line.last() != Some(&b'\n')
                || line.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'[')
            {
                return Err(io::Error::other(
                    "ACP requires individual newline-terminated messages",
                ));
            }
            let size = line.len();
            let line =
                String::from_utf8(line).map_err(|_| io::Error::other("ACP requires UTF-8"))?;
            Ok(Some((line, (input, bytes + size, count + 1))))
        },
    );
    let outgoing = futures::sink::unfold(
        (output, written.clone()),
        |(mut output, written), line: String| async move {
            if line.len() + 1 > FRAME_BYTES {
                return Err(io::Error::other("ACP output frame limit exceeded"));
            }
            let notification = serde_json::from_str::<Value>(&line)
                .ok()
                .is_some_and(|v| v["method"] == "session/update");
            tokio::time::timeout(WRITE_TIMEOUT, async {
                output.write_all(line.as_bytes()).await?;
                output.write_all(b"\n").await?;
                output.flush().await
            })
            .await
            .map_err(|_| {
                eprintln!("pablo: ACP consumer stalled; cancelling owned work");
                io::Error::other("ACP write timeout")
            })??;
            if notification {
                written.count.fetch_add(1, Ordering::Release);
                written.changed.notify_one();
            }
            Ok((output, written))
        },
    );
    let builder = Agent.builder().name("pablo")
        .on_receive_request({
            let state = state.clone();
            async move |request: wire::InitializeRequest, responder: Responder<wire::InitializeResponse>, _cx| {
                let mut state = state.lock().unwrap();
                if state.initialized { return responder.respond_with_error(invalid("already initialized")); }
                // ACP negotiation returns our supported version. A client that
                // cannot speak v1 must disconnect; draft v2 is never selected.
                state.initialized = true;
                state.extended = request.client_capabilities.meta.as_ref()
                    .and_then(|m| m.get(EXTENSION)) == Some(&json!(true));
                let caps = wire::AgentCapabilities::new().meta(meta(json!(true)));
                responder.respond(wire::InitializeResponse::new(agent_client_protocol::schema::ProtocolVersion::V1)
                    .agent_capabilities(caps)
                    .agent_info(wire::Implementation::new("pablo", env!("CARGO_PKG_VERSION"))))
            }
        }, agent_client_protocol::on_receive_request!())
        .on_receive_request({
            let state = state.clone();
            async move |request: wire::NewSessionRequest, responder: Responder<wire::NewSessionResponse>, _cx| {
                let mut state = state.lock().unwrap();
                if !state.initialized { return responder.respond_with_error(invalid("initialize first")); }
                if state.session.is_some() { return responder.respond_with_error(invalid("one session per process")); }
                if !request.mcp_servers.is_empty() { return responder.respond_with_error(invalid("MCP servers are unsupported")); }
                if !request.cwd.is_absolute() { return responder.respond_with_error(invalid("cwd must be absolute")); }
                let Ok(cwd) = request.cwd.canonicalize() else { return responder.respond_with_error(invalid("cwd must be an existing directory")); };
                if !cwd.is_dir() { return responder.respond_with_error(invalid("cwd must be a directory")); }
                let id = wire::SessionId::new(uuid::Uuid::new_v4().to_string());
                state.session = Some((id.clone(), cwd));
                responder.respond(wire::NewSessionResponse::new(id))
            }
        }, agent_client_protocol::on_receive_request!())
        .on_receive_notification({
            let state = state.clone(); let cancellation = cancellation.clone();
            async move |request: wire::CancelNotification, _cx| {
                let s = state.lock().unwrap();
                if s.prompt_active && s.session.as_ref().is_some_and(|(id,_)| *id == request.session_id) {
                    if !cancellation.is_cancelled() { eprintln!("pablo: ACP cancellation requested"); }
                    cancellation.cancel();
                }
                Ok(())
            }
        }, agent_client_protocol::on_receive_notification!())
        .on_receive_request({
            let state = state.clone(); let options = options.clone();
            let cancellation = cancellation.clone(); let written = written.clone();
            async move |request: wire::PromptRequest, responder: Responder<wire::PromptResponse>, cx: ConnectionTo<Client>| {
                let input = match prompt_text(&request.prompt) {
                    Ok(input) => input,
                    Err(error) => return responder.respond_with_error(error),
                };
                let (cwd, extended) = {
                    let mut s = state.lock().unwrap();
                    let Some((id, cwd)) = &s.session else { return responder.respond_with_error(invalid("create a session first")); };
                    if *id != request.session_id { return responder.respond_with_error(invalid("unknown session")); }
                    if s.prompted { return responder.respond_with_error(invalid("one prompt per session; start a new process")); }
                    let cwd = cwd.clone(); s.prompted = true; (cwd, s.extended)
                };
                let mut spec = match options.spec() {
                    Ok(spec) => spec,
                    Err(_) => return responder.respond_with_error(Error::internal_error().data("invalid host configuration")),
                };
                spec.workspace = cwd; spec.input = input; spec.session_id = Some(request.session_id.to_string());
                let (tx, rx) = async_channel::bounded(QUEUE_EVENTS);
                state.lock().unwrap().events = Some(tx.clone());
                let worker = {
                    let options = options.clone(); let cancel = cancellation.clone();
                    std::thread::Builder::new().name("pablo-run".into()).spawn(move || run_worker(options, spec, cancel, tx))
                };
                let Ok(worker) = worker else { return responder.respond_with_error(Error::internal_error().data("cannot start runtime worker")); };
                {
                    let mut s = state.lock().unwrap();
                    s.worker = Some(worker);
                    s.prompt_active = true;
                }
                let state = state.clone(); let written = written.clone(); let cancellation = cancellation.clone();
                let sender = cx.clone();
                cx.spawn(async move {
                    let request_cancel = responder.cancellation();
                    let forwarding = forward_events(rx, &sender, &written, extended);
                    tokio::pin!(forwarding);
                    let terminal = tokio::select! {
                        result = &mut forwarding => result,
                        _ = request_cancel.cancelled() => { cancellation.cancel(); forwarding.await }
                    }?;
                    let worker = state.lock().unwrap().worker.take();
                    let outcome = join_worker(worker).await;
                    state.lock().unwrap().prompt_active = false;
                    responder.respond_with_result(prompt_response(outcome, terminal, extended, cancellation.is_cancelled()))
                })
            }
        }, agent_client_protocol::on_receive_request!())
        .on_close({
            let cancellation = cancellation.clone();
            async move |_cx| {
                cancellation.cancel();
                // End the SDK connection; its receiver drops, waking the worker.
                Err(Error::internal_error().data("client disconnected"))
            }
        });
    #[cfg(unix)]
    let mut interrupts = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| "cannot register ACP signal handler")?;
    #[cfg(unix)]
    let mut terminates = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| "cannot register ACP signal handler")?;
    // Keep the connection future in an inner scope so dropping it releases all
    // queue receivers before we join owned runtime work.
    let result = {
        let connection = builder.connect_to(Lines::new(Box::pin(outgoing), Box::pin(incoming)));
        tokio::pin!(connection);
        #[cfg(unix)]
        {
            tokio::select! {
                result = &mut connection => result,
                _ = interrupts.recv() => Err(Error::internal_error()),
                _ = terminates.recv() => Err(Error::internal_error()),
            }
        }
        #[cfg(not(unix))]
        {
            connection.await
        }
    };
    cancellation.cancel();
    if let Some(events) = state.lock().unwrap().events.take() {
        events.close();
    }
    let worker = state.lock().unwrap().worker.take();
    if let Some(worker) = worker {
        join_worker(Some(worker))
            .await
            .map_err(|_| "ACP runtime cleanup failed")?;
    }
    if result.is_err() {
        eprintln!("pablo: ACP connection closed; owned work settled");
    }
    Ok(ExitCode::SUCCESS)
}

fn prompt_text(blocks: &[wire::ContentBlock]) -> Result<String, Error> {
    let mut input = String::new();
    for block in blocks {
        let text = match block {
            wire::ContentBlock::Text(text) => text.text.clone(),
            // ACP v1 requires resource links as a baseline. Pass the reference
            // as task data; never fetch an arbitrary URI or grant filesystem tools.
            wire::ContentBlock::ResourceLink(link) => {
                format!("\nResource reference: {} ({})\n", link.uri, link.name)
            }
            _ => return Err(invalid("this agent supports text and resource links only")),
        };
        if input.len() + text.len() > pablo_core::RunLimits::default().max_input_bytes - 4096 {
            return Err(invalid("prompt exceeds 1020 KiB"));
        }
        input.push_str(&text);
    }
    if input.trim().is_empty() {
        return Err(invalid("prompt must not be empty"));
    }
    Ok(input)
}

fn run_worker(
    options: Arc<Options>,
    spec: RunSpec,
    cancel: CancellationToken,
    tx: async_channel::Sender<RunEvent>,
) -> Result<RunOutcome, String> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let Ok(executor) = executor else {
        tx.close();
        return Err("cannot create runtime".into());
    };
    let result = executor.block_on(async {
        let provider = if let Some(endpoint) = std::env::var_os("PABLO_FIXTURE_ENDPOINT") {
            GatewayProvider::local_fixture(endpoint.to_str().ok_or("invalid fixture endpoint")?)?
        } else {
            GatewayProvider::vercel(&config::gateway_key(options.env_file.as_deref())?)?
        };
        let tools = if options.no_shell {
            ToolRegistry::default()
        } else {
            ToolRegistry::with_shell().map_err(|_| "cannot initialize shell tool")?
        };
        let mut trace = if let Some(path) = &options.trace_path {
            let mut open = OpenOptions::new();
            open.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.mode(0o600);
            }
            let file = open.open(path).map_err(|_| "cannot create trace file")?;
            Some(
                JsonlSink::new(BufWriter::new(file), &spec)
                    .map_err(|_| "invalid trace settings")?,
            )
        } else {
            None
        };
        let sdk = SdkTracerProvider::builder().build();
        let runtime = Runtime::new(telemetry::tracer(&sdk));
        let mut slow_reported = false;
        let mut sink = |event: &RunEvent| -> Result<(), SinkError> {
            if let Some(trace) = trace.as_mut() {
                trace.emit(event)?;
            }
            if serde_json::to_vec(event).map_err(io::Error::other)?.len() > FRAME_BYTES {
                return Err(SinkError::Capacity);
            }
            if tx.is_full() && !slow_reported {
                eprintln!("pablo: ACP consumer slow; runtime waiting for output capacity");
                slow_reported = true;
            }
            // Only this dedicated runtime thread blocks. The transport continues
            // reading cancellation and enforces its write deadline independently.
            tx.send_blocking(event.clone())
                .map_err(|_| io::Error::other("ACP consumer closed"))?;
            Ok(())
        };
        let outcome = runtime
            .run_with_tools(&spec, &provider, &tools, &cancel, &mut sink)
            .await;
        if sdk.shutdown_with_timeout(Duration::from_secs(2)).is_err() {
            eprintln!("pablo: telemetry shutdown failed");
        }
        match outcome {
            Ok(outcome) => Ok(outcome),
            Err(RunError::EventDelivery { outcome, .. }) => Ok(*outcome),
            Err(_) => Err("runtime rejected the run".into()),
        }
    });
    tx.close();
    result
}

async fn join_worker(worker: Option<Worker>) -> Result<RunOutcome, String> {
    let worker = worker.ok_or("missing runtime worker")?;
    tokio::task::spawn_blocking(move || worker.join())
        .await
        .map_err(|_| "runtime join failed")?
        .map_err(|_| "runtime worker failed")?
}

async fn forward_events(
    rx: async_channel::Receiver<RunEvent>,
    cx: &ConnectionTo<Client>,
    written: &Written,
    extended: bool,
) -> Result<Option<RunEvent>, Error> {
    let mut pending = None;
    let mut terminal = None;
    let mut sent = 0;
    loop {
        let mut event = match pending.take() {
            Some(event) => event,
            None => match rx.recv().await {
                Ok(event) => event,
                Err(_) => break,
            },
        };
        let first = event.seq;
        if matches!(event.kind, EventKind::TextDelta { .. }) {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(16);
            loop {
                let EventKind::TextDelta { text } = &event.kind else {
                    unreachable!()
                };
                if text.len() >= 4096 {
                    break;
                }
                let Ok(Ok(next)) = tokio::time::timeout_at(deadline, rx.recv()).await else {
                    break;
                };
                if let (EventKind::TextDelta { text }, EventKind::TextDelta { text: more }) =
                    (&mut event.kind, &next.kind)
                    && event.span_id == next.span_id
                    && text.len() + more.len() <= 4096
                {
                    text.push_str(more);
                    event.seq = next.seq;
                    event.timestamp_unix_micros = next.timestamp_unix_micros;
                } else {
                    pending = Some(next);
                    break;
                }
            }
        }
        if matches!(event.kind, EventKind::RunFinished { .. }) {
            terminal = Some(event.clone());
        }
        if let Some(update) = project(&event)? {
            let mut notification = wire::SessionNotification::new(event.session_id.clone(), update);
            if extended {
                notification.meta = Some(meta(correlation(&event, first)));
            }
            cx.send_notification(notification)?;
            sent += 1;
            while written.count.load(Ordering::Acquire) < sent {
                written.changed.notified().await;
            }
        }
    }
    Ok(terminal)
}

fn project(event: &RunEvent) -> Result<Option<wire::SessionUpdate>, Error> {
    Ok(Some(match &event.kind {
        EventKind::TextDelta { text } => wire::SessionUpdate::AgentMessageChunk(
            wire::ContentChunk::new(wire::ContentBlock::Text(wire::TextContent::new(text))),
        ),
        EventKind::ToolStarted { call } => wire::SessionUpdate::ToolCall(
            wire::ToolCall::new(call.id.clone(), "shell.run")
                .kind(wire::ToolKind::Execute)
                .status(wire::ToolCallStatus::Pending)
                .raw_input(call.arguments.clone()),
        ),
        EventKind::ShellStarted { call_id, .. } => {
            wire::SessionUpdate::ToolCallUpdate(wire::ToolCallUpdate::new(
                call_id.clone(),
                wire::ToolCallUpdateFields::new().status(wire::ToolCallStatus::InProgress),
            ))
        }
        EventKind::ToolFinished {
            call_id, result, ..
        } => {
            let success = result.status == ToolStatus::Completed
                && result.shell.as_ref().is_none_or(|s| s.exit_code == Some(0));
            wire::SessionUpdate::ToolCallUpdate(wire::ToolCallUpdate::new(
                call_id.clone(),
                wire::ToolCallUpdateFields::new()
                    .status(if success {
                        wire::ToolCallStatus::Completed
                    } else {
                        wire::ToolCallStatus::Failed
                    })
                    .raw_output(serde_json::to_value(result).map_err(|_| Error::internal_error())?),
            ))
        }
        _ => return Ok(None),
    }))
}

fn prompt_response(
    outcome: Result<RunOutcome, String>,
    terminal: Option<RunEvent>,
    extended: bool,
    cancelled: bool,
) -> Result<wire::PromptResponse, Error> {
    let outcome =
        outcome.map_err(|_| Error::internal_error().data("runtime setup or execution failed"))?;
    let terminal =
        terminal.ok_or_else(|| Error::internal_error().data("terminal event unavailable"))?;
    let mut details = correlation(&terminal, terminal.seq);
    details["outcome"] = serde_json::to_value(&outcome).map_err(|_| Error::internal_error())?;
    // ACP cancellation remains meaningful while queued updates drain, even if
    // the core has already settled. Preserve that immutable native outcome in
    // metadata; the standard stop reason describes the cancelled prompt turn.
    let reason = if cancelled {
        wire::StopReason::Cancelled
    } else {
        match outcome {
            RunOutcome::Completed { .. } => wire::StopReason::EndTurn,
            RunOutcome::Cancelled => wire::StopReason::Cancelled,
            RunOutcome::PolicyDenied { .. } => wire::StopReason::Refusal,
            RunOutcome::LimitExceeded {
                limit: LimitKind::OutputTokens,
            } => wire::StopReason::MaxTokens,
            RunOutcome::LimitExceeded {
                limit: LimitKind::ModelCalls | LimitKind::ToolCalls,
            } => wire::StopReason::MaxTurnRequests,
            _ => {
                return Err(Error::internal_error().data(if extended {
                    json!({"pablo/v1":details})
                } else {
                    json!({"status":outcome.label()})
                }));
            }
        }
    };
    let mut response = wire::PromptResponse::new(reason);
    if extended {
        response.meta = Some(meta(details));
    }
    Ok(response)
}
