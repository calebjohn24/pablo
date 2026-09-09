//! Stable ACP v1 adapter. The official SDK owns dispatch and JSON-RPC IDs.
use agent_client_protocol::{
    Agent, Client, ConnectionTo, Error, Lines, Responder, schema::v1 as wire,
};
use futures::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use pablo_core::{CancellationToken, EventKind, LimitKind, RunEvent, RunOutcome, tool::ToolStatus};
use serde_json::{Value, json};
use std::{
    fs::File,
    io,
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

use crate::config::Options;

mod worker;
use worker::{Task, Worker};

const EXTENSION: &str = "pablo/v1";
const FRAME_BYTES: usize = 32 * 1024 * 1024;
const INPUT_BYTES: usize = 64 * 1024 * 1024;
const INPUT_MESSAGES: usize = 128;
const QUEUE_EVENTS: usize = 8;
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
struct State {
    initialized: bool,
    extended: bool,
    task_extended: bool,
    session: Option<(wire::SessionId, std::path::PathBuf)>,
    prompted: bool,
    prompt_active: bool,
    worker: Option<Worker>,
    cancellation: Option<CancellationToken>,
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
    // In addition to individual frames, bound the entire process connection.
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
            // Borrow only the method; skip the payload without materializing a
            // second JSON tree just to acknowledge a session/update write.
            #[derive(serde::Deserialize)]
            struct Message<'a> {
                #[serde(borrow)]
                method: Option<&'a str>,
            }
            let notification = serde_json::from_str::<Message<'_>>(&line)
                .ok()
                .is_some_and(|message| message.method == Some("session/update"));
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
    let builder = Agent
        .builder()
        .name("pablo")
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: wire::InitializeRequest,
                            responder: Responder<wire::InitializeResponse>,
                            _cx| {
                    let mut state = state.lock().unwrap();
                    if state.initialized {
                        return responder.respond_with_error(invalid("already initialized"));
                    }
                    // ACP negotiation returns our supported version. A client that
                    // cannot speak v1 must disconnect; draft v2 is never selected.
                    state.initialized = true;
                    state.extended = request
                        .client_capabilities
                        .meta
                        .as_ref()
                        .and_then(|m| m.get(EXTENSION))
                        == Some(&json!(true));
                    state.task_extended = state.extended
                        && request
                            .client_capabilities
                            .meta
                            .as_ref()
                            .and_then(|m| m.get("pablo/task-v1"))
                            == Some(&json!(true));
                    let mut capabilities = meta(json!(true));
                    capabilities.insert("pablo/task-v1".into(), json!(true));
                    let caps = wire::AgentCapabilities::new().meta(capabilities);
                    responder.respond(
                        wire::InitializeResponse::new(
                            agent_client_protocol::schema::ProtocolVersion::V1,
                        )
                        .agent_capabilities(caps)
                        .agent_info(wire::Implementation::new(
                            "pablo",
                            env!("CARGO_PKG_VERSION"),
                        )),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |request: wire::NewSessionRequest,
                            responder: Responder<wire::NewSessionResponse>,
                            _cx| {
                    let mut state = state.lock().unwrap();
                    if !state.initialized {
                        return responder.respond_with_error(invalid("initialize first"));
                    }
                    if state.prompt_active || (state.session.is_some() && !state.prompted) {
                        return responder.respond_with_error(invalid(
                            "finish the current session before creating another",
                        ));
                    }
                    if !request.mcp_servers.is_empty() {
                        return responder
                            .respond_with_error(invalid("MCP servers are unsupported"));
                    }
                    if !request.cwd.is_absolute() {
                        return responder.respond_with_error(invalid("cwd must be absolute"));
                    }
                    let Ok(cwd) = request.cwd.canonicalize() else {
                        return responder
                            .respond_with_error(invalid("cwd must be an existing directory"));
                    };
                    if !cwd.is_dir() {
                        return responder.respond_with_error(invalid("cwd must be a directory"));
                    }
                    let id = wire::SessionId::new(uuid::Uuid::new_v4().to_string());
                    state.session = Some((id.clone(), cwd));
                    state.prompted = false;
                    responder.respond(wire::NewSessionResponse::new(id))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = state.clone();
                async move |request: wire::CancelNotification, _cx| {
                    let s = state.lock().unwrap();
                    if s.prompt_active
                        && s.session
                            .as_ref()
                            .is_some_and(|(id, _)| *id == request.session_id)
                        && let Some(cancel) = &s.cancellation
                    {
                        if !cancel.is_cancelled() {
                            eprintln!("pablo: ACP cancellation requested");
                        }
                        cancel.cancel();
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let options = options.clone();
                let cancellation = cancellation.clone();
                let written = written.clone();
                async move |request: wire::PromptRequest,
                            responder: Responder<wire::PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    let input = match prompt_text(&request.prompt) {
                        Ok(input) => input,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    let (cwd, extended, task_extended) = {
                        let mut s = state.lock().unwrap();
                        let Some((id, cwd)) = &s.session else {
                            return responder.respond_with_error(invalid("create a session first"));
                        };
                        if *id != request.session_id {
                            return responder.respond_with_error(invalid("unknown session"));
                        }
                        if s.prompted {
                            return responder.respond_with_error(invalid(
                                "one prompt per session; create a new session",
                            ));
                        }
                        let cwd = cwd.clone();
                        s.prompted = true;
                        (cwd, s.extended, s.task_extended)
                    };
                    let mut spec = match options.spec() {
                        Ok(spec) => spec,
                        Err(_) => {
                            return responder.respond_with_error(
                                Error::internal_error().data("invalid host configuration"),
                            );
                        }
                    };
                    spec.workspace = cwd;
                    spec.input = input;
                    spec.session_id = Some(request.session_id.to_string());
                    let incoming = request.meta.as_ref().and_then(|m| m.get(EXTENSION));
                    let parent = if extended {
                        crate::otel::parent(
                            incoming
                                .and_then(|m| m.get("traceparent"))
                                .and_then(Value::as_str),
                            incoming
                                .and_then(|m| m.get("tracestate"))
                                .and_then(Value::as_str),
                        )
                    } else {
                        opentelemetry::Context::new()
                    };
                    let (tx, rx) = async_channel::bounded(QUEUE_EVENTS);
                    let cancel = cancellation.child_token();
                    let (completed, outcome) = tokio::sync::oneshot::channel();
                    {
                        let mut s = state.lock().unwrap();
                        if s.worker.is_none() {
                            match Worker::start(options.clone()) {
                                Ok(worker) => s.worker = Some(worker),
                                Err(_) => {
                                    return responder.respond_with_error(
                                        Error::internal_error().data("cannot start runtime worker"),
                                    );
                                }
                            }
                        }
                        let task = Task {
                            spec,
                            parent,
                            cancel: cancel.clone(),
                            events: tx.clone().into(),
                            completed,
                        };
                        if s.worker.as_ref().unwrap().submit(task).is_err() {
                            return responder.respond_with_error(
                                Error::internal_error().data("runtime worker unavailable"),
                            );
                        }
                        s.events = Some(tx);
                        s.cancellation = Some(cancel.clone());
                        s.prompt_active = true;
                    }
                    let state = state.clone();
                    let written = written.clone();
                    let sender = cx.clone();
                    cx.spawn(async move {
                        let request_cancel = responder.cancellation();
                        let forwarding = forward_events(rx, &sender, &written, extended);
                        tokio::pin!(forwarding);
                        let terminal = tokio::select! {
                            result = &mut forwarding => result,
                            _ = request_cancel.cancelled() => { cancel.cancel(); forwarding.await }
                        }?;
                        let outcome = outcome
                            .await
                            .unwrap_or_else(|_| Err("runtime worker failed".into()));
                        {
                            let mut s = state.lock().unwrap();
                            s.prompt_active = false;
                            s.events = None;
                            s.cancellation = None;
                        }
                        responder.respond_with_result(prompt_response(
                            outcome,
                            terminal,
                            extended,
                            task_extended,
                            cancel.is_cancelled(),
                        ))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
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
        worker
            .shutdown()
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

struct EventStream {
    rx: async_channel::Receiver<RunEvent>,
    pending: Option<RunEvent>,
    first_text: bool,
}
impl EventStream {
    fn new(rx: async_channel::Receiver<RunEvent>) -> Self {
        Self {
            rx,
            pending: None,
            first_text: true,
        }
    }
    async fn next(&mut self) -> Option<(RunEvent, u64)> {
        let mut event = match self.pending.take() {
            Some(event) => event,
            None => match self.rx.recv().await {
                Ok(event) => event,
                Err(_) => return None,
            },
        };
        let first = event.seq;
        if matches!(event.kind, EventKind::ModelStarted { .. }) {
            self.first_text = true;
        }
        let text = matches!(event.kind, EventKind::TextDelta { .. });
        if text && !self.first_text {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(16);
            loop {
                let EventKind::TextDelta { text } = &event.kind else {
                    unreachable!()
                };
                if text.len() >= 4096 {
                    break;
                }
                let Ok(Ok(next)) = tokio::time::timeout_at(deadline, self.rx.recv()).await else {
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
                    self.pending = Some(next);
                    break;
                }
            }
        }
        if text {
            self.first_text = false;
        }
        Some((event, first))
    }
}

async fn forward_events(
    rx: async_channel::Receiver<RunEvent>,
    cx: &ConnectionTo<Client>,
    written: &Written,
    extended: bool,
) -> Result<Option<RunEvent>, Error> {
    let mut events = EventStream::new(rx);
    let mut terminal = None;
    let mut sent = written.count.load(Ordering::Acquire);
    while let Some((event, first)) = events.next().await {
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
            wire::ToolCall::new(call.id.clone(), call.name.clone())
                .kind(if matches!(call.name.as_str(), "fs.write" | "fs.edit") {
                    wire::ToolKind::Edit
                } else if call.name.starts_with("fs.") {
                    wire::ToolKind::Read
                } else {
                    wire::ToolKind::Execute
                })
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
    task_extended: bool,
    cancelled: bool,
) -> Result<wire::PromptResponse, Error> {
    let outcome =
        outcome.map_err(|_| Error::internal_error().data("runtime setup or execution failed"))?;
    let terminal =
        terminal.ok_or_else(|| Error::internal_error().data("terminal event unavailable"))?;
    let mut details = correlation(&terminal, terminal.seq);
    if task_extended {
        details["task"] = serde_json::to_value(
            pablo_core::TaskResult::from_terminal(&terminal).ok_or_else(Error::internal_error)?,
        )
        .map_err(|_| Error::internal_error())?;
    } else {
        details["outcome"] = serde_json::to_value(&outcome).map_err(|_| Error::internal_error())?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Poll;

    fn event(seq: u64, kind: EventKind) -> RunEvent {
        RunEvent {
            accounting: None,
            schema_version: "c1.2".into(),
            seq,
            timestamp_unix_micros: seq,
            run_id: "run".into(),
            session_id: "session".into(),
            trace_id: "trace".into(),
            span_id: "model".into(),
            parent_span_id: None,
            trace_flags: "01".into(),
            kind,
        }
    }

    #[tokio::test]
    async fn first_text_of_each_model_is_ready_without_waiting_for_more_events_or_a_timer() {
        let (tx, rx) = async_channel::bounded(8);
        let mut stream = EventStream::new(rx);
        for seq in [1, 3] {
            tx.send(event(
                seq,
                EventKind::ModelStarted {
                    provider: "fixture".into(),
                    model: "test".into(),
                },
            ))
            .await
            .unwrap();
            stream.next().await.unwrap();
            tx.send(event(
                seq + 1,
                EventKind::TextDelta {
                    text: "first".into(),
                },
            ))
            .await
            .unwrap();
            assert!(
                matches!(futures::poll!(Box::pin(stream.next())), Poll::Ready(Some((_, first))) if first == seq + 1)
            );
        }
    }

    #[tokio::test]
    async fn later_text_is_batched_with_its_native_range_and_a_finite_deadline() {
        let (tx, rx) = async_channel::bounded(8);
        let mut stream = EventStream::new(rx);
        for seq in 1..=3 {
            tx.send(event(
                seq,
                EventKind::TextDelta {
                    text: seq.to_string(),
                },
            ))
            .await
            .unwrap();
        }
        stream.next().await.unwrap();
        let next = stream.next();
        tokio::pin!(next);
        assert!(futures::poll!(&mut next).is_pending());
        let (event, first) = tokio::time::timeout(Duration::from_secs(1), next)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((first, event.seq, event.timestamp_unix_micros), (2, 3, 3));
        assert_eq!(event.kind, EventKind::TextDelta { text: "23".into() });
    }
}
