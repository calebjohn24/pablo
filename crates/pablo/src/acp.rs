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

#[path = "acp/delivery.rs"]
mod delivery;
#[path = "acp/handlers.rs"]
mod handlers;
// C3.21 prepares this internal path; child execution is enabled at C3.22.
#[allow(dead_code)]
#[path = "acp/in_memory.rs"]
mod in_memory;
#[allow(dead_code)] // Some direct host operations are exercised only by internal acceptance tests.
#[path = "acp/supervisor.rs"]
pub(crate) mod supervisor;
#[path = "acp/worker.rs"]
mod worker;
use worker::{Task, Worker};

const EXTENSION: &str = "pablo/v1";
const FRAME_BYTES: usize = 32 * 1024 * 1024;
const INPUT_BYTES: usize = 64 * 1024 * 1024;
const INPUT_MESSAGES: usize = 128;
const QUEUE_EVENTS: usize = 8;
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Default)]
struct Extensions {
    base: bool,
    task: bool,
    route: bool,
    compaction: bool,
    skills: bool,
    output: bool,
    repair: bool,
}

#[derive(Default)]
struct State {
    closed: bool,
    admitted_child: Option<in_memory::AdmittedChild>,
    initialized: bool,
    extensions: Extensions,
    session: Option<(wire::SessionId, std::path::PathBuf)>,
    mcp: Vec<pablo_core::mcp::ClientServer>,
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
    let mut value = json!({"schema_version":event.schema_version,"run_id":event.run_id,
        "session_id":event.session_id,"seq_start":first,"seq_end":event.seq,
        "timestamp_unix_micros":event.timestamp_unix_micros,
        "trace_id":event.trace_id,"span_id":event.span_id,
        "parent_span_id":event.parent_span_id,"trace_flags":event.trace_flags});
    if let Some(seq) = event.root_seq {
        value["root_seq"] = json!(seq);
    }
    if let Some(agent) = &event.agent {
        value["agent"] = serde_json::to_value(agent).expect("bounded agent identity");
    }
    if let Some(identity) = &event.deployment {
        value["deployment"] = serde_json::to_value(identity).expect("bounded identity");
    }
    if let Some(profile) = &event.model_profile {
        value["model_profile"] = serde_json::to_value(profile).expect("bounded provider identity");
    }
    value
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
    // MCP diagnostics/session selection stay offline; resolve its credentials per task.
    if let Some(configured) = options.configured()?.filter(|deployment| {
        deployment.options()["mcp"]["servers"]
            .as_object()
            .is_some_and(|servers| servers.is_empty())
    }) {
        let prepared = configured
            .prepare_run(pablo_core::deployment::RunInput {
                input: String::new(),
                workspace: None,
                session_id: Some(uuid::Uuid::new_v4().to_string()),
            })
            .map_err(|error| error.to_string())?;
        let secrets =
            crate::deployment::Secrets::read(&prepared, options.deployment.as_ref().unwrap())?;
        let provider = secrets.provider(options.deployment.as_ref().unwrap())?;
        prepared
            .preflight(provider.as_ref())
            .map_err(|error| error.to_string())?;
        crate::otel::Telemetry::check_configured(&prepared, secrets.headers.as_ref())?;
    }
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
            // second JSON tree just to acknowledge an event notification write.
            #[derive(serde::Deserialize)]
            struct Message<'a> {
                #[serde(borrow)]
                method: Option<&'a str>,
            }
            let notification =
                serde_json::from_str::<Message<'_>>(&line)
                    .ok()
                    .is_some_and(|message| {
                        matches!(
                            message.method,
                            Some(
                                "session/update"
                                    | "_pablo/model_attempt"
                                    | "_pablo/compaction"
                                    | "_pablo/skill"
                            )
                        )
                    });
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
                    responder.respond_with_result(handlers::initialize(&state, request))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                let options = options.clone();
                async move |request: wire::NewSessionRequest,
                            responder: Responder<wire::NewSessionResponse>,
                            _cx| {
                    responder.respond_with_result(handlers::new_session(&state, &options, request))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = state.clone();
                async move |request: wire::CancelNotification, _cx| {
                    handlers::cancel(&state, request)
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
                    let pending = match handlers::start_prompt(
                        state.clone(),
                        options.clone(),
                        &cancellation,
                        request,
                    ) {
                        Ok(pending) => pending,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    let delivery = delivery::Delivery::Stdio {
                        cx: cx.clone(),
                        written: written.clone(),
                    };
                    cx.spawn(async move {
                        let request_cancel = responder.cancellation();
                        let response = pending
                            .finish(&delivery, async {
                                request_cancel.cancelled().await;
                            })
                            .await;
                        responder.respond_with_result(response)
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

fn prompt_text(blocks: &[wire::ContentBlock], max_bytes: usize) -> Result<String, Error> {
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
        if input.len().saturating_add(text.len()) > max_bytes {
            return Err(invalid("prompt exceeds admitted byte bound"));
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
                    event.root_seq = next.root_seq;
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
    delivery: &delivery::Delivery,
    extensions: Extensions,
    capture_content: bool,
) -> Result<Option<RunEvent>, Error> {
    if matches!(delivery, delivery::Delivery::Native { .. }) {
        return delivery.forward_native(rx).await;
    }
    let mut events = EventStream::new(rx);
    let mut terminal = None;
    while let Some((event, first)) = events.next().await {
        if matches!(event.kind, EventKind::RunFinished { .. })
            && (event.root_seq.is_none() || event.agent.as_ref().is_none_or(|a| a.depth() == 0))
        {
            terminal = Some(event.clone());
        }
        #[cfg(unix)]
        if extensions.skills
            && let EventKind::SkillActivated {
                skill,
                instructions,
            } = &event.kind
        {
            let params=serde_json::value::to_raw_value(&json!({"sessionId":delivery_session(&event),"type":"skill.activated","pablo/v1":correlation(&event,first),"skill":skill,"instructions":if capture_content {instructions.as_deref()}else{None},"content_redacted":!capture_content})).map_err(|_|Error::internal_error())?;
            delivery
                .send(wire::AgentNotification::ExtNotification(
                    wire::ExtNotification::new("_pablo/skill", Arc::from(params)),
                ))
                .await?;
        }
        if extensions.compaction
            && matches!(
                event.kind,
                EventKind::CompactionStarted | EventKind::CompactionFinished { .. }
            )
        {
            let mut details = correlation(&event, first);
            details["compaction"] =
                serde_json::to_value(&event.compaction).map_err(|_| Error::internal_error())?;
            let (kind, summary, summary_bytes) = match &event.kind {
                EventKind::CompactionFinished {
                    summary,
                    summary_bytes,
                } => (
                    "context.compaction.finished",
                    if capture_content {
                        summary.as_deref()
                    } else {
                        None
                    },
                    *summary_bytes,
                ),
                _ => ("context.compaction.started", None, 0),
            };
            let params=serde_json::value::to_raw_value(&json!({"sessionId":delivery_session(&event),"type":kind,"pablo/v1":details,"summary":summary,"summary_bytes":summary_bytes,"content_redacted":!capture_content})).map_err(|_|Error::internal_error())?;
            delivery
                .send(wire::AgentNotification::ExtNotification(
                    wire::ExtNotification::new("_pablo/compaction", Arc::from(params)),
                ))
                .await?;
        }
        if extensions.route
            && event.model_route.is_some()
            && matches!(
                event.kind,
                EventKind::ModelStarted { .. } | EventKind::ModelFinished { .. }
            )
        {
            let mut details = correlation(&event, first);
            details["model_route"] =
                serde_json::to_value(&event.model_route).map_err(|_| Error::internal_error())?;
            let params = serde_json::value::to_raw_value(&json!({
                "sessionId": delivery_session(&event),
                "type": if matches!(event.kind, EventKind::ModelStarted { .. }) { "model.started" } else { "model.finished" },
                "pablo/v1": details
            })).map_err(|_| Error::internal_error())?;
            delivery
                .send(wire::AgentNotification::ExtNotification(
                    wire::ExtNotification::new("_pablo/model_attempt", Arc::from(params)),
                ))
                .await?;
        }
        if let Some(update) = project(&event)? {
            let mut notification =
                wire::SessionNotification::new(delivery_session(&event).to_owned(), update);
            if extensions.base {
                let mut details = correlation(&event, first);
                if extensions.output && event.output_validation.is_some() {
                    details["output_validation"] = serde_json::to_value(&event.output_validation)
                        .map_err(|_| Error::internal_error())?;
                }
                if extensions.repair && event.output_repair.is_some() {
                    details["output_repair"] = serde_json::to_value(&event.output_repair)
                        .map_err(|_| Error::internal_error())?;
                }
                notification.meta = Some(meta(details));
            }
            delivery
                .send(wire::AgentNotification::SessionNotification(notification))
                .await?;
        }
    }
    Ok(terminal)
}

fn delivery_session(event: &RunEvent) -> &str {
    if event.root_seq.is_some()
        && let Some(agent) = &event.agent
    {
        agent.root_session_id()
    } else {
        &event.session_id
    }
}

fn delivery_call_id(event: &RunEvent, call_id: &str) -> String {
    if event.root_seq.is_some()
        && let Some(agent) = &event.agent
    {
        format!("{}/{}", agent.agent_id(), call_id)
    } else {
        call_id.to_owned()
    }
}

fn project(event: &RunEvent) -> Result<Option<wire::SessionUpdate>, Error> {
    let remote = event
        .agent
        .as_ref()
        .is_some_and(|agent| agent.kind() == pablo_core::children::AgentKind::RemoteA2a);
    Ok(Some(match &event.kind {
        EventKind::RunStarted if remote => wire::SessionUpdate::ToolCall(
            wire::ToolCall::new(delivery_call_id(event, "remote"), "Remote A2A task")
                .kind(wire::ToolKind::Execute)
                .status(wire::ToolCallStatus::Pending),
        ),
        EventKind::A2aUpdate { remote } => {
            wire::SessionUpdate::ToolCallUpdate(wire::ToolCallUpdate::new(
                delivery_call_id(event, "remote"),
                wire::ToolCallUpdateFields::new()
                    .status(wire::ToolCallStatus::InProgress)
                    .raw_output(json!({"remote_a2a":remote})),
            ))
        }
        EventKind::RunFinished { outcome } if remote => {
            wire::SessionUpdate::ToolCallUpdate(wire::ToolCallUpdate::new(
                delivery_call_id(event, "remote"),
                wire::ToolCallUpdateFields::new()
                    .status(if outcome.is_completed() {
                        wire::ToolCallStatus::Completed
                    } else {
                        wire::ToolCallStatus::Failed
                    })
                    .raw_output(json!({"outcome":outcome})),
            ))
        }
        EventKind::TextDelta { text } => wire::SessionUpdate::AgentMessageChunk(
            wire::ContentChunk::new(wire::ContentBlock::Text(wire::TextContent::new(text))),
        ),
        EventKind::ToolStarted { call } => wire::SessionUpdate::ToolCall(
            wire::ToolCall::new(delivery_call_id(event, &call.id), call.name.clone())
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
                delivery_call_id(event, call_id),
                wire::ToolCallUpdateFields::new().status(wire::ToolCallStatus::InProgress),
            ))
        }
        EventKind::ToolFinished {
            call_id, result, ..
        } => {
            let success = result.status == ToolStatus::Completed
                && result.shell.as_ref().is_none_or(|s| s.exit_code == Some(0));
            wire::SessionUpdate::ToolCallUpdate(wire::ToolCallUpdate::new(
                delivery_call_id(event, call_id),
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
    extensions: Extensions,
    cancelled: bool,
) -> Result<wire::PromptResponse, Error> {
    let outcome = outcome.map_err(|message| {
        if message.starts_with("config_") {
            Error::invalid_params().data(message)
        } else {
            Error::internal_error().data("runtime setup or execution failed")
        }
    })?;
    let terminal =
        terminal.ok_or_else(|| Error::internal_error().data("terminal event unavailable"))?;
    let mut details = correlation(&terminal, terminal.seq);
    if extensions.route && terminal.model_route.is_some() {
        details["model_route"] =
            serde_json::to_value(&terminal.model_route).map_err(|_| Error::internal_error())?;
    }
    if extensions.compaction && terminal.compaction.is_some() {
        details["compaction"] =
            serde_json::to_value(&terminal.compaction).map_err(|_| Error::internal_error())?;
    }
    if extensions.task {
        let mut task =
            pablo_core::TaskResult::from_terminal(&terminal).ok_or_else(Error::internal_error)?;
        if !extensions.repair {
            task.output_repair = None;
            if task.output_validation.is_some() {
                task.schema_version = "c3.13".into();
            }
        }
        if !extensions.output {
            task.output_validation = None;
            task.schema_version = pablo_core::task::TASK_SCHEMA_VERSION.into();
        }
        details["task"] = serde_json::to_value(task).map_err(|_| Error::internal_error())?;
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
                return Err(Error::internal_error().data(if extensions.base {
                    json!({"pablo/v1":details})
                } else {
                    json!({"status":outcome.label()})
                }));
            }
        }
    };
    let mut response = wire::PromptResponse::new(reason);
    if extensions.base {
        response.meta = Some(meta(details));
    }
    Ok(response)
}

/// Normalize the official ACP wire types without retaining client credentials.
fn normalize_mcp(
    servers: &[wire::McpServer],
) -> Result<Vec<pablo_core::mcp::ClientServer>, &'static str> {
    use pablo_core::mcp::{ClientServer, ClientTransport, MAX_SERVERS};
    if servers.len() > MAX_SERVERS {
        return Err("MCP server configuration exceeds host bound");
    }
    servers
        .iter()
        .map(|server| match server {
            wire::McpServer::Stdio(s) if s.env.is_empty() => Ok(ClientServer {
                name: s.name.clone(),
                transport: ClientTransport::Stdio {
                    command: s.command.to_str().ok_or("MCP launcher invalid")?.into(),
                    args: s.args.clone(),
                    env: Default::default(),
                },
            }),
            wire::McpServer::Http(s) if s.headers.is_empty() => Ok(ClientServer {
                name: s.name.clone(),
                transport: ClientTransport::Http {
                    url: s.url.clone(),
                    headers: Default::default(),
                },
            }),
            _ => Err("MCP client credentials or transport denied by host"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Poll;

    fn event(seq: u64, kind: EventKind) -> RunEvent {
        RunEvent {
            root_seq: None,
            agent: None,
            model_route: None,
            compaction: None,
            output_validation: None,
            output_repair: None,
            model_profile: None,
            deployment: None,
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
    async fn native_delivery_preserves_every_original_record_and_waits_for_consumption() {
        let originals = vec![
            event(1, EventKind::RunStarted),
            event(2, EventKind::TextDelta { text: "a".into() }),
            event(3, EventKind::TextDelta { text: "b".into() }),
            event(4, EventKind::TextDelta { text: "c".into() }),
            event(
                5,
                EventKind::RunFinished {
                    outcome: RunOutcome::Cancelled,
                },
            ),
        ];
        let (tx, rx) = async_channel::bounded(8);
        for event in &originals {
            tx.send(event.clone()).await.unwrap();
        }
        tx.close();
        let (sender, receiver) = async_channel::bounded(1);
        let delivery = delivery::Delivery::Native {
            sender,
            execution_cancel: CancellationToken::new(),
            closed: CancellationToken::new(),
        };
        let forwarding = forward_events(rx, &delivery, Extensions::default(), false);
        tokio::pin!(forwarding);
        for original in &originals {
            let update = tokio::select! {
                result = &mut forwarding => panic!("forwarding finished before consumption: {result:?}"),
                update = receiver.recv() => update.unwrap(),
            };
            assert_eq!(update.event, *original);
            assert!(matches!(futures::poll!(&mut forwarding), Poll::Pending));
            assert!(receiver.is_empty());
            update.consumed.send(()).unwrap();
        }
        receiver.close(); // Closing after acknowledging the terminal is successful.
        assert_eq!(forwarding.await.unwrap().as_ref(), originals.last());
    }

    #[tokio::test]
    async fn native_receiver_loss_wakes_idle_or_unacknowledged_forwarding() {
        for in_flight in [false, true] {
            let (tx, rx) = async_channel::bounded(8);
            let (sender, receiver) = async_channel::bounded(1);
            let delivery = delivery::Delivery::Native {
                sender,
                execution_cancel: CancellationToken::new(),
                closed: CancellationToken::new(),
            };
            let forwarding = delivery.forward_native(rx);
            tokio::pin!(forwarding);
            let held = if in_flight {
                tx.send(event(1, EventKind::RunStarted)).await.unwrap();
                Some(tokio::select! {
                    result = &mut forwarding => panic!("unexpected result: {result:?}"),
                    event = receiver.recv() => event.unwrap(),
                })
            } else {
                None
            };
            receiver.close();
            assert!(
                tokio::time::timeout(Duration::from_secs(1), forwarding)
                    .await
                    .unwrap()
                    .is_err()
            );
            drop(held);
            drop(tx);
        }
    }

    #[tokio::test]
    async fn cancellation_bounds_receipts_but_waits_for_owned_cleanup_events() {
        let (tx, rx) = async_channel::bounded(8);
        let (sender, receiver) = async_channel::bounded(1);
        let cancelled = CancellationToken::new();
        let delivery = delivery::Delivery::Native {
            sender,
            closed: CancellationToken::new(),
            execution_cancel: cancelled.clone(),
        };
        tx.send(event(1, EventKind::RunStarted)).await.unwrap();
        let forwarding = delivery.forward_native(rx);
        tokio::pin!(forwarding);
        let held = tokio::select! {
            result = &mut forwarding => panic!("unexpected result: {result:?}"),
            update = receiver.recv() => update.unwrap(),
        };
        let started = tokio::time::Instant::now();
        cancelled.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), forwarding)
                .await
                .unwrap()
                .is_err()
        );
        assert!(started.elapsed() >= delivery::CANCEL_ACK_TIMEOUT);
        drop(held);
        drop(tx);

        let (tx, rx) = async_channel::bounded(8);
        let (sender, receiver) = async_channel::bounded(1);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let delivery = delivery::Delivery::Native {
            sender,
            closed: CancellationToken::new(),
            execution_cancel: cancelled,
        };
        let cleanup = async {
            tokio::time::sleep(delivery::CANCEL_ACK_TIMEOUT * 2).await;
            tx.send(event(
                1,
                EventKind::RunFinished {
                    outcome: RunOutcome::Cancelled,
                },
            ))
            .await
            .unwrap();
            tx.close();
        };
        let consume = async {
            let update = receiver.recv().await.unwrap();
            update.consumed.send(()).unwrap();
        };
        let (result, (), ()) = tokio::join!(delivery.forward_native(rx), cleanup, consume);
        assert!(matches!(
            result.unwrap().unwrap().kind,
            EventKind::RunFinished {
                outcome: RunOutcome::Cancelled
            }
        ));
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
            let mut record = event(
                seq,
                EventKind::TextDelta {
                    text: seq.to_string(),
                },
            );
            record.root_seq = Some(seq + 10);
            tx.send(record).await.unwrap();
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
        assert_eq!(event.root_seq, Some(13));
        let correlation = correlation(&event, first);
        assert_eq!(correlation["root_seq"], 13);
        assert_eq!(correlation["seq_start"], 2);
        assert_eq!(correlation["seq_end"], 3);
    }
}
