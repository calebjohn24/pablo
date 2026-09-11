//! Transport-independent handlers accepting the official stable ACP types.
//! Stdio owns JSON-RPC framing; these handlers own session admission and state.
use super::*;

pub(super) fn initialize(
    state: &Mutex<State>,
    request: wire::InitializeRequest,
) -> Result<wire::InitializeResponse, Error> {
    let mut state = state.lock().unwrap();
    if state.closed {
        return Err(invalid("connection closed"));
    }
    if state.initialized {
        return Err(invalid("already initialized"));
    }
    // ACP negotiation returns our supported version. A client that
    // cannot speak v1 must disconnect; draft v2 is never selected.
    state.initialized = true;
    state.extensions.base = request
        .client_capabilities
        .meta
        .as_ref()
        .and_then(|m| m.get(EXTENSION))
        == Some(&json!(true));
    state.extensions.task = state.extensions.base
        && request
            .client_capabilities
            .meta
            .as_ref()
            .and_then(|m| m.get("pablo/task-v1"))
            == Some(&json!(true));
    state.extensions.route = state.extensions.base
        && request
            .client_capabilities
            .meta
            .as_ref()
            .and_then(|m| m.get("pablo/model-route-v1"))
            == Some(&json!(true));
    state.extensions.compaction = state.extensions.base
        && request
            .client_capabilities
            .meta
            .as_ref()
            .and_then(|m| m.get("pablo/compaction-v1"))
            == Some(&json!(true));
    state.extensions.output = state.extensions.base
        && state.extensions.task
        && request
            .client_capabilities
            .meta
            .as_ref()
            .and_then(|m| m.get("pablo/output-v1"))
            == Some(&json!(true));
    state.extensions.skills = state.extensions.base
        && request
            .client_capabilities
            .meta
            .as_ref()
            .and_then(|m| m.get("pablo/skills-v1"))
            == Some(&json!(true));
    state.extensions.repair = state.extensions.output
        && request
            .client_capabilities
            .meta
            .as_ref()
            .and_then(|m| m.get("pablo/output-repair-v1"))
            == Some(&json!(true));
    let mut capabilities = meta(json!(true));
    capabilities.insert("pablo/task-v1".into(), json!(true));
    capabilities.insert("pablo/model-route-v1".into(), json!(true));
    capabilities.insert("pablo/compaction-v1".into(), json!(true));
    capabilities.insert("pablo/skills-v1".into(), json!(true));
    capabilities.insert("pablo/output-v1".into(), json!(true));
    capabilities.insert("pablo/output-repair-v1".into(), json!(true));
    let caps = wire::AgentCapabilities::new()
        .meta(capabilities)
        .mcp_capabilities(wire::McpCapabilities::new().http(true));
    Ok(
        wire::InitializeResponse::new(agent_client_protocol::schema::ProtocolVersion::V1)
            .agent_capabilities(caps)
            .agent_info(wire::Implementation::new(
                "pablo",
                env!("CARGO_PKG_VERSION"),
            )),
    )
}

pub(super) fn new_session(
    state: &Mutex<State>,
    options: &Options,
    request: wire::NewSessionRequest,
) -> Result<wire::NewSessionResponse, Error> {
    let mut state = state.lock().unwrap();
    if state.closed {
        return Err(invalid("connection closed"));
    }
    if !state.initialized {
        return Err(invalid("initialize first"));
    }
    if state.admitted_child.is_some() && state.session.is_some() {
        return Err(invalid("one session per temporary child"));
    }
    if state.prompt_active || (state.session.is_some() && !state.prompted) {
        return Err(invalid(
            "finish the current session before creating another",
        ));
    }
    let mcp = match normalize_mcp(&request.mcp_servers) {
        Ok(mcp) => mcp,
        Err(message) => return Err(invalid(message)),
    };
    if state.admitted_child.is_some() && !mcp.is_empty() {
        return Err(invalid("child MCP selection is already admitted"));
    }
    if !mcp.is_empty() {
        let admitted = options
            .configured()
            .map_err(|_| "MCP host configuration invalid")
            .and_then(|host| host.ok_or("MCP server is not host-configured; unsupported"))
            .and_then(|host| {
                host.admit_mcp_client(&mcp)
                    .map_err(|_| "MCP server configuration denied by host")
            });
        if let Err(message) = admitted {
            return Err(invalid(message));
        }
    }
    if !request.cwd.is_absolute() {
        return Err(invalid("cwd must be absolute"));
    }
    let Ok(cwd) = request.cwd.canonicalize() else {
        return Err(invalid("cwd must be an existing directory"));
    };
    if !cwd.is_dir() {
        return Err(invalid("cwd must be a directory"));
    }
    let id = wire::SessionId::new(uuid::Uuid::new_v4().to_string());
    if let Some(child) = &state.admitted_child {
        if cwd != child.prepared.spec().workspace {
            return Err(invalid("child workspace is already admitted"));
        }
    } else {
        if let Err(message) =
            options.prepare_run(Some(String::new()), Some(cwd.clone()), Some(id.to_string()))
        {
            return Err(Error::invalid_params().data(message));
        }
    }
    state.mcp = mcp;
    state.session = Some((id.clone(), cwd));
    state.prompted = false;
    Ok(wire::NewSessionResponse::new(id))
}

pub(super) fn cancel(state: &Mutex<State>, request: wire::CancelNotification) -> Result<(), Error> {
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

pub(super) struct PendingPrompt {
    terminal: Option<Arc<Mutex<Option<RunEvent>>>>,
    state: Arc<Mutex<State>>,
    rx: async_channel::Receiver<RunEvent>,
    outcome: tokio::sync::oneshot::Receiver<Result<RunOutcome, String>>,
    cancel: CancellationToken,
    extensions: Extensions,
    capture_content: bool,
}

pub(super) fn start_prompt(
    state: Arc<Mutex<State>>,
    options: Arc<Options>,
    cancellation: &CancellationToken,
    request: wire::PromptRequest,
) -> Result<PendingPrompt, Error> {
    let max_input = state.lock().unwrap().admitted_child.as_ref().map_or(
        pablo_core::RunLimits::default().max_input_bytes - 4096,
        |child| child.prepared.spec().limits.max_input_bytes,
    );
    let input = prompt_text(&request.prompt, max_input)?;
    let (cwd, extensions, mcp, admitted) = {
        let mut s = state.lock().unwrap();
        if s.closed || cancellation.is_cancelled() {
            return Err(invalid("connection closed"));
        }
        let Some((id, cwd)) = &s.session else {
            return Err(invalid("create a session first"));
        };
        if *id != request.session_id {
            return Err(invalid("unknown session"));
        }
        if s.prompted {
            return Err(invalid("one prompt per session; create a new session"));
        }
        let admitted = if let Some(child) = &s.admitted_child {
            if input != child.prepared.spec().input {
                return Err(invalid("child input is already admitted"));
            }
            Some((
                child.prepared.clone(),
                child.accounting.clone(),
                child.root_cancellation.clone(),
                child.parent.clone(),
            ))
        } else {
            None
        };
        let cwd = cwd.clone();
        s.prompted = true;
        (cwd, s.extensions, s.mcp.clone(), admitted)
    };
    let mut prepared = if let Some((prepared, ..)) = &admitted {
        Some(prepared.clone())
    } else {
        match options.prepare_run(
            Some(input.clone()),
            Some(cwd.clone()),
            Some(request.session_id.to_string()),
        ) {
            Ok(prepared) => prepared,
            Err(message) => {
                return Err(Error::invalid_params().data(message));
            }
        }
    };
    if let Some(prepared) = &mut prepared
        && let Err(error) = prepared.select_mcp_client(&mcp)
    {
        return Err(Error::invalid_params().data(error.to_string()));
    }
    let mut spec = match prepared
        .as_ref()
        .map(|p| Ok(p.spec().clone()))
        .unwrap_or_else(|| options.spec())
    {
        Ok(spec) => spec,
        Err(_) => {
            return Err(Error::internal_error().data("invalid host configuration"));
        }
    };
    spec.workspace = cwd;
    spec.input = input;
    spec.session_id = Some(request.session_id.to_string());
    let incoming = request.meta.as_ref().and_then(|m| m.get(EXTENSION));
    let parent = if let Some((_, _, _, parent)) = &admitted {
        parent.clone()
    } else if let Some(prepared) = prepared.as_ref().filter(|_| extensions.base) {
        crate::otel::parent_explicit(
            incoming
                .and_then(|m| m.get("traceparent"))
                .and_then(Value::as_str),
            incoming
                .and_then(|m| m.get("tracestate"))
                .and_then(Value::as_str),
            prepared.deployment().options()["otel"]["propagators"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "tracecontext"),
        )
    } else if extensions.base {
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
    let capture_content = spec.trace.capture_content;
    let (tx, rx) = async_channel::bounded(QUEUE_EVENTS);
    let cancel = admitted.as_ref().map_or_else(
        || cancellation.child_token(),
        |(_, _, root, _)| root.child_token(),
    );
    let (completed, outcome) = tokio::sync::oneshot::channel();
    let terminal = admitted.as_ref().map(|_| Arc::new(Mutex::new(None)));
    {
        let mut s = state.lock().unwrap();
        if s.closed || cancellation.is_cancelled() {
            return Err(invalid("connection closed"));
        }
        if s.worker.is_none() {
            let lease = s
                .admitted_child
                .as_ref()
                .and_then(|child| child.lease.clone());
            match Worker::start(options.clone(), lease) {
                Ok(worker) => s.worker = Some(worker),
                Err(_) => {
                    return Err(Error::internal_error().data("cannot start runtime worker"));
                }
            }
        }
        let task = Task {
            terminal: terminal.clone(),
            accounting: admitted.map(|(_, scope, _, _)| scope),
            prepared,
            spec,
            parent,
            cancel: cancel.clone(),
            events: tx.clone().into(),
            completed,
        };
        if s.worker.as_ref().unwrap().submit(task).is_err() {
            return Err(Error::internal_error().data("runtime worker unavailable"));
        }
        s.events = Some(tx);
        s.cancellation = Some(cancel.clone());
        s.prompt_active = true;
    }

    Ok(PendingPrompt {
        terminal,
        state,
        rx,
        outcome,
        cancel,
        extensions,
        capture_content,
    })
}

pub(super) struct Completion {
    pub outcome: Result<RunOutcome, String>,
    pub terminal: Option<RunEvent>,
}
impl PendingPrompt {
    pub(super) async fn finish(
        self,
        delivery: &delivery::Delivery,
        request_cancel: impl std::future::Future<Output = ()>,
    ) -> Result<wire::PromptResponse, Error> {
        self.finish_observed(delivery, request_cancel, None).await
    }
    pub(super) async fn finish_observed(
        self,
        delivery: &delivery::Delivery,
        request_cancel: impl std::future::Future<Output = ()>,
        receipt: Option<tokio::sync::oneshot::Sender<Completion>>,
    ) -> Result<wire::PromptResponse, Error> {
        let forwarding = forward_events(self.rx, delivery, self.extensions, self.capture_content);
        tokio::pin!(forwarding);
        let terminal = tokio::select! {
            result = &mut forwarding => result,
            _ = request_cancel => { self.cancel.cancel(); forwarding.await }
        };
        if terminal.is_err() {
            self.cancel.cancel();
            if let Some(events) = self.state.lock().unwrap().events.as_ref() {
                events.close();
            }
        }
        // Always await task settlement, including a closed or failed consumer.
        let outcome = self
            .outcome
            .await
            .unwrap_or_else(|_| Err("runtime worker failed".into()));
        {
            let mut s = self.state.lock().unwrap();
            s.prompt_active = false;
            s.events = None;
            s.cancellation = None;
        }
        if let Some(receipt) = receipt {
            let _ = receipt.send(Completion {
                outcome: outcome.clone(),
                terminal: self
                    .terminal
                    .as_ref()
                    .and_then(|event| event.lock().unwrap().take())
                    .or_else(|| terminal.as_ref().ok().and_then(Clone::clone)),
            });
        }
        prompt_response(
            outcome,
            terminal?,
            self.extensions,
            self.cancel.is_cancelled(),
        )
    }
}
