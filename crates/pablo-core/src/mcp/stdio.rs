//! One owned server process. No restart, reconnect or implicit tool replay.
use super::{
    transport::{BoundedTransport, Bounds},
    *,
};
use opentelemetry::{KeyValue, trace::TraceContextExt};
use rmcp::{
    RoleClient,
    model::*,
    service::{RunningService, ServiceError},
};
use rustix::process::{Pid, Signal, kill_process_group};
use std::{path::Path, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::JoinHandle,
    time::{Instant, timeout_at},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    PolicyDenied,
    Spawn,
    Protocol,
    Catalog,
    InvalidArguments,
    InvalidResult,
    UnsupportedResult,
    Cancelled,
    TimedOut,
    Cleanup,
    Closed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP operation failed: {self:?}")
    }
}
impl std::error::Error for Error {}

pub struct CatalogTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    input: Arc<crate::output::CompiledOutput>,
    output: Option<Arc<crate::output::CompiledOutput>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResultContent {
    pub text: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured: Option<serde_json::Value>,
    pub is_error: bool,
}

pub struct StdioSession {
    service: Option<RunningService<RoleClient, ClientInfo>>,
    child: Child,
    pid: Pid,
    armed: bool,
    cleanup_failed: bool,
    stderr: Option<JoinHandle<Result<usize, ()>>>,
    transport_cancel: CancellationToken,
    bounds: Arc<Bounds>,
    tools: Vec<CatalogTool>,
    operation_timeout: Duration,
    catalog_bytes: usize,
}
impl Drop for StdioSession {
    fn drop(&mut self) {
        self.transport_cancel.cancel();
        if self.armed {
            let _ = kill_process_group(self.pid, Signal::KILL);
        }
        if let Some(task) = &self.stderr {
            task.abort();
        }
    }
}
impl StdioSession {
    /// Internal launch boundary: callers must derive cwd and private environment from admitted host configuration.
    pub(crate) async fn start(
        server: &Server,
        cwd: &Path,
        env: BTreeMap<String, String>,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<Self, Error> {
        server.validate().map_err(|_| Error::PolicyDenied)?;
        let Server::Stdio {
            command,
            args,
            startup_timeout_ms,
            operation_timeout_ms,
            ..
        } = server
        else {
            return Err(Error::Protocol);
        };
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::TimedOut);
        }
        let mut command = Command::new(command);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|_| Error::Spawn)?;
        let pid = Pid::from_raw(child.id().ok_or(Error::Spawn)? as i32).ok_or(Error::Spawn)?;
        let transport = BoundedTransport::new(
            child.stdout.take().ok_or(Error::Spawn)?,
            child.stdin.take().ok_or(Error::Spawn)?,
        );
        let bounds = transport.bounds.clone();
        let transport_cancel = transport.cancellation.clone();
        let mut stderr = child.stderr.take().ok_or(Error::Spawn)?;
        let stderr = tokio::spawn(async move {
            let mut bytes = [0u8; 8192];
            let mut captured = 0usize;
            loop {
                let n = stderr.read(&mut bytes).await.map_err(|_| ())?;
                if n == 0 {
                    return Ok(captured);
                }
                captured = captured.saturating_add(n).min(65536);
            }
        });
        let mut session = Self {
            service: None,
            child,
            pid,
            armed: true,
            cleanup_failed: false,
            stderr: Some(stderr),
            transport_cancel,
            bounds,
            tools: Vec::new(),
            operation_timeout: Duration::from_millis(*operation_timeout_ms),
            catalog_bytes: 0,
        };
        let startup_deadline =
            deadline.min(Instant::now() + Duration::from_millis(*startup_timeout_ms));
        let info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("pablo", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25);
        let started = bounded(startup_deadline, cancellation, async {
            let service = rmcp::serve_client(info, transport)
                .await
                .map_err(|_| Error::Protocol)?;
            let compatible = service.peer_info().is_some_and(|info| {
                info.protocol_version == ProtocolVersion::V_2025_11_25
                    && info.capabilities.tools.is_some()
            });
            session.service = Some(service);
            if !compatible {
                return Err(Error::Protocol);
            }
            session.discover().await
        })
        .await;
        if let Err(error) = started {
            session.close().await?;
            return Err(error);
        }
        Ok(session)
    }
    pub fn catalog_bytes(&self) -> usize {
        self.catalog_bytes
    }
    pub fn tools(&self) -> &[CatalogTool] {
        &self.tools
    }
    pub fn process_id(&self) -> u32 {
        self.pid.as_raw_nonzero().get() as u32
    }
    async fn discover(&mut self) -> Result<(), Error> {
        let mut cursor = None;
        let mut cursors = HashSet::new();
        let mut names = HashSet::new();
        let mut bytes = 0usize;
        for _ in 0..8 {
            let request = ClientRequest::ListToolsRequest(ListToolsRequest {
                method: Default::default(),
                params: Some(PaginatedRequestParams::default().with_cursor(cursor)),
                extensions: Default::default(),
            });
            let response = self
                .service
                .as_ref()
                .ok_or(Error::Closed)?
                .send_request(request)
                .await
                .map_err(protocol)?;
            let ServerResult::ListToolsResult(page) = response else {
                return Err(Error::Protocol);
            };
            bytes =
                bytes.saturating_add(serde_json::to_vec(&page).map_err(|_| Error::Catalog)?.len());
            self.catalog_bytes = bytes;
            if bytes > MAX_RESULT_BYTES || self.tools.len() + page.tools.len() > 64 {
                return Err(Error::Catalog);
            }
            for tool in page.tools {
                if !name(&tool.name, 128)
                    || !names.insert(tool.name.to_string())
                    || tool.description.as_ref().is_some_and(|s| s.len() > 4096)
                {
                    return Err(Error::Catalog);
                }
                let input_schema = serde_json::Value::Object((*tool.input_schema).clone());
                if input_schema["type"] != "object"
                    || !crate::filesystem::fits(&input_schema, MAX_SCHEMA_BYTES)
                {
                    return Err(Error::Catalog);
                }
                let input = crate::output::compile(&input_schema).map_err(|_| Error::Catalog)?;
                let output = tool
                    .output_schema
                    .map(|schema| {
                        let value = serde_json::Value::Object((*schema).clone());
                        if !crate::filesystem::fits(&value, MAX_SCHEMA_BYTES) {
                            return Err(Error::Catalog);
                        }
                        crate::output::compile(&value).map_err(|_| Error::Catalog)
                    })
                    .transpose()?;
                self.tools.push(CatalogTool {
                    name: tool.name.into_owned(),
                    description: tool.description.map(|s| s.into_owned()).unwrap_or_default(),
                    input_schema,
                    input,
                    output,
                });
            }
            cursor = page.next_cursor;
            match &cursor {
                None => return Ok(()),
                Some(c) if c.len() <= 1024 && cursors.insert(c.clone()) => {}
                _ => return Err(Error::Catalog),
            }
        }
        Err(Error::Catalog)
    }
    pub async fn call(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ResultContent, Error> {
        self.call_with_context(
            name,
            arguments,
            deadline,
            cancellation,
            &opentelemetry::Context::new(),
        )
        .await
    }
    pub(crate) async fn call_with_context(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        deadline: Instant,
        cancellation: &CancellationToken,
        context: &opentelemetry::Context,
    ) -> Result<ResultContent, Error> {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::TimedOut);
        }
        let tool = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or(Error::PolicyDenied)?;
        let stopped = || cancellation.is_cancelled() || Instant::now() >= deadline;
        if !crate::filesystem::fits(&arguments, MAX_RESULT_BYTES) {
            return Err(Error::InvalidArguments);
        }
        let encoded = serde_json::to_string(&arguments).map_err(|_| Error::InvalidArguments)?;
        if !tool
            .input
            .validate(&encoded, 16_777_216, stopped)
            .is_some_and(|v| v.status == "valid")
        {
            return Err(Error::InvalidArguments);
        }
        let output = tool.output.clone();
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or(Error::InvalidArguments)?;
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new(name.to_owned()).with_arguments(arguments),
        ));
        let service = self.service.as_ref().ok_or(Error::Closed)?;
        let operation_deadline = deadline.min(Instant::now() + self.operation_timeout);
        let mut meta = RequestMetaObject::new();
        let span = context.span();
        let identity = span.span_context();
        if identity.is_valid() {
            meta.0.insert(
                "traceparent".into(),
                serde_json::Value::String(format!(
                    "00-{}-{}-{:02x}",
                    identity.trace_id(),
                    identity.span_id(),
                    identity.trace_flags().to_u8()
                )),
            );
            if !identity.trace_state().header().is_empty() {
                meta.0.insert(
                    "tracestate".into(),
                    serde_json::Value::String(identity.trace_state().header()),
                );
            }
        }
        let handle = bounded(operation_deadline, cancellation, async {
            service
                .send_request_with_option(
                    request,
                    rmcp::service::PeerRequestOptions::no_options().with_meta(meta),
                )
                .await
                .map_err(protocol)
        })
        .await;
        let handle = match handle {
            Ok(value) => value,
            Err(error) => {
                self.close().await?;
                return Err(error);
            }
        };
        let id = handle.id.clone();
        context
            .span()
            .set_attribute(KeyValue::new("jsonrpc.request.id", id.to_string()));
        let response = bounded(operation_deadline, cancellation, async {
            handle.await_response().await.map_err(|error| {
                if let ServiceError::McpError(ref data) = error {
                    context.span().set_attribute(KeyValue::new(
                        "rpc.response.status_code",
                        i64::from(data.code.0),
                    ));
                }
                protocol(error)
            })
        })
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let cleanup_deadline = Instant::now() + Duration::from_secs(2);
                if matches!(error, Error::Cancelled | Error::TimedOut) {
                    let notification =
                        CancelledNotification::new(CancelledNotificationParam::new(Some(id), None));
                    let _ = tokio::time::timeout(
                        Duration::from_millis(50),
                        service.send_notification(notification.into()),
                    )
                    .await;
                }
                self.close_before(cleanup_deadline).await?;
                return Err(error);
            }
        };
        if self.bounds.failed() {
            self.close().await?;
            return Err(Error::Protocol);
        }
        let ServerResult::CallToolResult(result) = response else {
            return Err(Error::UnsupportedResult);
        };
        if !crate::filesystem::fits(&result, MAX_RESULT_BYTES) {
            return Err(Error::InvalidResult);
        }
        if result
            .structured_content
            .as_ref()
            .is_some_and(|v| !v.is_object())
        {
            return Err(Error::InvalidResult);
        }
        let mut text = Vec::new();
        for content in result.content {
            match content {
                ContentBlock::Text(value) => text.push(value.text),
                _ => return Err(Error::UnsupportedResult),
            }
        }
        if let Some(schema) = output
            && !(result.is_error == Some(true) && result.structured_content.is_none())
        {
            let structured = result
                .structured_content
                .as_ref()
                .ok_or(Error::InvalidResult)?;
            let text = serde_json::to_string(structured).map_err(|_| Error::InvalidResult)?;
            if !schema
                .validate(&text, 16_777_216, stopped)
                .is_some_and(|v| v.status == "valid")
            {
                return Err(Error::InvalidResult);
            }
        }
        Ok(ResultContent {
            text,
            structured: result.structured_content,
            is_error: result.is_error.unwrap_or(false),
        })
    }
    pub async fn close(&mut self) -> Result<(), Error> {
        self.close_before(Instant::now() + Duration::from_secs(2))
            .await
    }
    async fn close_before(&mut self, deadline: Instant) -> Result<(), Error> {
        if !self.armed {
            return if self.cleanup_failed {
                Err(Error::Cleanup)
            } else {
                Ok(())
            };
        }
        self.transport_cancel.cancel();
        let killed = matches!(
            kill_process_group(self.pid, Signal::KILL),
            Ok(()) | Err(rustix::io::Errno::SRCH) | Err(rustix::io::Errno::PERM)
        );
        self.armed = false;
        // The bounded adapter cancels pending reads/writes; no server handlers are admitted.
        let mut service_joined = true;
        if let Some(mut service) = self.service.take() {
            service_joined = service.close().await.is_ok();
        }
        let joined = timeout_at(deadline, async {
            self.child.wait().await.map_err(|_| Error::Cleanup)?;
            if let Some(task) = self.stderr.as_mut() {
                task.await
                    .map_err(|_| Error::Cleanup)?
                    .map_err(|_| Error::Cleanup)?;
            }
            loop {
                match rustix::process::test_kill_process_group(self.pid) {
                    Err(rustix::io::Errno::SRCH) => break,
                    Ok(()) | Err(rustix::io::Errno::PERM) => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    _ => return Err(Error::Cleanup),
                }
            }
            Ok(())
        })
        .await;
        if let Some(task) = self.stderr.take()
            && !task.is_finished()
        {
            task.abort();
            let _ = task.await;
        }
        self.cleanup_failed = !killed || !service_joined || !matches!(joined, Ok(Ok(())));
        if self.cleanup_failed {
            Err(Error::Cleanup)
        } else {
            Ok(())
        }
    }
}
fn protocol(_: ServiceError) -> Error {
    Error::Protocol
}
async fn bounded<T>(
    deadline: Instant,
    cancellation: &CancellationToken,
    future: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::select! {biased; _=cancellation.cancelled()=>Err(Error::Cancelled), result=timeout_at(deadline,future)=>result.map_err(|_| Error::TimedOut)?}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires pinned independent Python SDK fixture environment"]
    async fn independent_python_sdk_reads_real_evidence_and_closes_owned_process() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let python = root.join(".pablo/mcp-fixture-venv/bin/python");
        assert!(
            python.exists(),
            "install tests/fixtures/mcp/requirements.txt first"
        );
        let cwd = std::env::temp_dir().join(format!("pablo-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("evidence.txt"), "synthetic actual evidence").unwrap();
        let server: Server = serde_json::from_value(serde_json::json!({"transport":"stdio","command":python,"args":[root.join("tests/fixtures/mcp/server.py")]})).unwrap();
        let cancellation = CancellationToken::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut session = StdioSession::start(
            &server,
            &cwd,
            BTreeMap::from([("FIXTURE_TOKEN".into(), "synthetic-mcp-token".into())]),
            deadline,
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(session.tools().len(), 1);
        assert_eq!(session.tools()[0].name, "read_evidence");
        let pid = session.pid;
        let result = session
            .call(
                "read_evidence",
                serde_json::json!({}),
                deadline,
                &cancellation,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(
            result.structured.unwrap()["text"],
            "synthetic actual evidence"
        );
        assert!(!result.text.is_empty());
        session.close().await.unwrap();
        assert_eq!(
            rustix::process::test_kill_process_group(pid),
            Err(rustix::io::Errno::SRCH)
        );
        session.close().await.unwrap();
        std::fs::remove_dir_all(cwd).unwrap();
    }
    #[tokio::test]
    #[ignore = "requires the isolated Python fixture interpreter"]
    async fn hostile_stdio_peers_are_bounded_and_joined_on_every_failure() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        for (mode, expected) in [
            ("startup_hang", Some(Error::TimedOut)),
            ("wrong_version", Some(Error::Protocol)),
            ("no_tools", Some(Error::Protocol)),
            ("frame_overflow", Some(Error::Protocol)),
            ("duplicate", Some(Error::Catalog)),
            ("catalog_bound", Some(Error::Catalog)),
            ("schema", Some(Error::Catalog)),
            ("cursor", Some(Error::Catalog)),
            ("call_hang", Some(Error::TimedOut)),
            ("rpc_error", Some(Error::Protocol)),
            ("unsupported", Some(Error::UnsupportedResult)),
            ("invalid_result", Some(Error::InvalidResult)),
            ("progress_overflow", Some(Error::Protocol)),
            ("progress", None),
            ("stderr", None),
            ("descendant", None),
            ("tool_error", None),
        ] {
            let cwd =
                std::env::temp_dir().join(format!("pablo-mcp-fault-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&cwd).unwrap();
            let server:Server=serde_json::from_value(serde_json::json!({"transport":"stdio","command":root.join(".pablo/mcp-fixture-venv/bin/python"),"args":[root.join("tests/fixtures/mcp/adversarial.py"),Path::new(mode)]})).unwrap();
            let cancellation = CancellationToken::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            let started = StdioSession::start(
                &server,
                &cwd,
                BTreeMap::new(),
                if mode == "startup_hang" {
                    Instant::now() + Duration::from_millis(150)
                } else {
                    deadline
                },
                &cancellation,
            )
            .await;
            let actual = match started {
                Err(error) => Some(error),
                Ok(mut session) => {
                    let result = session
                        .call(
                            "read",
                            serde_json::json!({}),
                            if mode == "call_hang" {
                                Instant::now() + Duration::from_millis(100)
                            } else {
                                deadline
                            },
                            &cancellation,
                        )
                        .await;
                    if let Ok(result) = &result {
                        assert_eq!(result.is_error, mode == "tool_error");
                    }
                    session.close().await.unwrap();
                    result.err()
                }
            };
            assert_eq!(actual, expected, "{mode}");
            let pid = std::fs::read_to_string(cwd.join("pid"))
                .unwrap()
                .parse::<i32>()
                .unwrap();
            assert_eq!(
                rustix::process::test_kill_process_group(Pid::from_raw(pid).unwrap()),
                Err(rustix::io::Errno::SRCH),
                "{mode}"
            );
            std::fs::remove_dir_all(cwd).unwrap();
        }
    }
    #[tokio::test]
    #[ignore = "requires the isolated Python fixture interpreter"]
    async fn cancellation_and_invalid_arguments_do_not_leave_owned_work() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let cwd = std::env::temp_dir().join(format!("pablo-mcp-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&cwd).unwrap();
        let server:Server=serde_json::from_value(serde_json::json!({"transport":"stdio","command":root.join(".pablo/mcp-fixture-venv/bin/python"),"args":[root.join("tests/fixtures/mcp/adversarial.py"),"call_hang"]})).unwrap();
        let cancellation = CancellationToken::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut session =
            StdioSession::start(&server, &cwd, BTreeMap::new(), deadline, &cancellation)
                .await
                .unwrap();
        assert_eq!(
            session
                .call(
                    "read",
                    serde_json::json!({"unlisted":"value"}),
                    deadline,
                    &cancellation
                )
                .await,
            Err(Error::InvalidArguments)
        );
        assert!(!cwd.join("called").exists());
        let cancel = cancellation.clone();
        let trigger = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            cancel.cancel();
        });
        assert_eq!(
            session
                .call("read", serde_json::json!({}), deadline, &cancellation)
                .await,
            Err(Error::Cancelled)
        );
        trigger.await.unwrap();
        session.close().await.unwrap();
        assert_eq!(
            rustix::process::test_kill_process_group(session.pid),
            Err(rustix::io::Errno::SRCH)
        );
        std::fs::remove_dir_all(cwd).unwrap();
    }
}
