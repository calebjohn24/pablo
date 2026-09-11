//! One process-owned executor and reusable setup; each submitted task is isolated.
use super::{FRAME_BYTES, Options};
use pablo_core::{
    CancellationToken, EventKind, EventSink, JsonlSink, RunError, RunEvent, RunOutcome, RunSpec,
    Runtime, SinkError, ToolRegistry, gateway::GatewayProvider, telemetry,
};
use std::{
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    sync::Arc,
};
use tokio::sync::oneshot;

pub(super) struct Task {
    pub prepared: Option<pablo_core::deployment::PreparedRun>,
    pub spec: RunSpec,
    pub parent: opentelemetry::Context,
    pub cancel: CancellationToken,
    pub events: EventSender,
    pub completed: oneshot::Sender<Result<RunOutcome, String>>,
}

/// Close the per-task stream even if execution unwinds before sending an outcome.
pub(super) struct EventSender(async_channel::Sender<RunEvent>);
impl From<async_channel::Sender<RunEvent>> for EventSender {
    fn from(sender: async_channel::Sender<RunEvent>) -> Self {
        Self(sender)
    }
}
impl std::ops::Deref for EventSender {
    type Target = async_channel::Sender<RunEvent>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for EventSender {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(super) struct Worker {
    commands: async_channel::Sender<Task>,
    thread: std::thread::JoinHandle<Result<(), String>>,
}

impl Worker {
    pub fn start(options: Arc<Options>) -> Result<Self, String> {
        let (commands, rx) = async_channel::bounded(1);
        let thread = std::thread::Builder::new()
            .name("pablo-run".into())
            .spawn(move || run(options, rx))
            .map_err(|_| "cannot start runtime worker")?;
        Ok(Self { commands, thread })
    }

    pub fn submit(&self, task: Task) -> Result<(), String> {
        self.commands
            .try_send(task)
            .map_err(|_| "runtime worker unavailable".into())
    }

    pub async fn shutdown(self) -> Result<(), String> {
        self.commands.close();
        tokio::task::spawn_blocking(move || self.thread.join())
            .await
            .map_err(|_| "runtime join failed")?
            .map_err(|_| "runtime worker failed")?
    }
}

struct Resources {
    configuration: Option<(String, String, crate::deployment::Secrets)>,
    provider: Box<dyn pablo_core::Provider>,
    tools: ToolRegistry,
    telemetry: crate::otel::Telemetry,
}
impl Resources {
    fn new(options: &Options) -> Result<Self, String> {
        let provider = if let Some(endpoint) = std::env::var_os("PABLO_FIXTURE_ENDPOINT") {
            GatewayProvider::local_fixture_for(
                options.provider.unwrap_or_default(),
                endpoint.to_str().ok_or("invalid fixture endpoint")?,
            )?
        } else {
            GatewayProvider::selected(
                options.provider.unwrap_or_default(),
                &crate::config::provider_key(
                    options.provider.unwrap_or_default(),
                    options.env_file.as_deref(),
                )?,
            )?
        };
        let tools = options.tools()?;
        Ok(Self {
            configuration: None,
            provider: Box::new(provider),
            tools,
            telemetry: crate::otel::Telemetry::new(),
        })
    }
}

fn run(options: Arc<Options>, tasks: async_channel::Receiver<Task>) -> Result<(), String> {
    let Ok(first) = tasks.recv_blocking() else {
        return Ok(());
    };
    let Ok(executor) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        tasks.close();
        first.events.close();
        let _ = first.completed.send(Err("cannot create runtime".into()));
        return Err("cannot create runtime".into());
    };
    executor.block_on(async {
        let mut resources = None;
        let mut first = Some(first);
        loop {
            let task = if let Some(task) = first.take() {
                task
            } else {
                match tasks.recv().await {
                    Ok(task) => task,
                    Err(_) => break,
                }
            };
            let outcome = async {
                if let Some(prepared) = &task.prepared {
                    let bootstrap = options.deployment.as_ref().unwrap();
                    let secrets = crate::deployment::Secrets::read(prepared, bootstrap)?;
                    let reuse = resources
                        .as_ref()
                        .and_then(|r: &Resources| r.configuration.as_ref())
                        .is_some_and(|(config, bindings, prior)| {
                            config == prepared.deployment().fingerprint()
                                && bindings == prepared.bindings_fingerprint()
                                && prior.same_private_values(&secrets)
                        });
                    if !reuse {
                        let provider = secrets.provider(bootstrap)?;
                        let tools = if prepared.needs_async_tools() {
                            prepared
                                .preflight(provider.as_ref())
                                .map_err(|error| error.to_string())?;
                            ToolRegistry::default()
                        } else {
                            let tools = prepared.tools().map_err(|error| error.to_string())?;
                            pablo_core::runtime::validate_run(
                                prepared.spec(),
                                provider.as_ref(),
                                &tools,
                            )
                            .map_err(|_| "config_invalid_value at /run")?;
                            tools
                        };
                        crate::otel::Telemetry::check_configured(
                            prepared,
                            secrets.headers.as_ref(),
                        )?;
                        if let Some(prior) = resources.take() {
                            prior.telemetry.shutdown().await;
                        }
                        let telemetry =
                            crate::otel::Telemetry::configured(prepared, secrets.headers.as_ref())?;
                        resources = Some(Resources {
                            provider,
                            tools,
                            telemetry,
                            configuration: Some((
                                prepared.deployment().fingerprint().into(),
                                prepared.bindings_fingerprint().into(),
                                secrets,
                            )),
                        });
                    }
                    if prepared.needs_async_tools() {
                        prepared
                            .preflight(resources.as_ref().unwrap().provider.as_ref())
                            .map_err(|error| error.to_string())?;
                    } else {
                        pablo_core::runtime::validate_run(
                            prepared.spec(),
                            resources.as_ref().unwrap().provider.as_ref(),
                            &resources.as_ref().unwrap().tools,
                        )
                        .map_err(|_| "config_invalid_value at /run")?;
                    }
                } else if resources.is_none() {
                    resources = Some(Resources::new(&options)?);
                }
                execute(&options, resources.as_ref().unwrap(), &task).await
            }
            .await;
            // Closing the per-task stream lets the adapter finish draining while
            // this executor remains available to a subsequent independent task.
            task.events.close();
            let _ = task.completed.send(outcome);
        }
        if let Some(resources) = resources {
            resources.telemetry.shutdown().await;
        }
        Ok(())
    })
}

async fn execute(
    options: &Options,
    resources: &Resources,
    task: &Task,
) -> Result<RunOutcome, String> {
    let spec = &task.spec;
    let mut trace = if let Some(prepared) = &task.prepared {
        prepared
            .create_trace_file()
            .map_err(|e| e.to_string())?
            .map(|file| {
                JsonlSink::new(BufWriter::new(file), spec)
                    .map_err(|_| "config_invalid_value at /options/trace")
            })
            .transpose()?
    } else if let Some(path) = &options.trace_path {
        // A fixed path keeps exclusive creation; hosts running several sessions
        // can select a unique file for each with the generated session ID.
        let path = path
            .to_str()
            .filter(|path| path.contains("{session_id}"))
            .map(|path| {
                std::path::PathBuf::from(
                    path.replace("{session_id}", spec.session_id.as_deref().unwrap()),
                )
            })
            .unwrap_or_else(|| path.clone());
        let mut open = OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let file = open.open(path).map_err(|_| "cannot create trace file")?;
        Some(JsonlSink::new(BufWriter::new(file), spec).map_err(|_| "invalid trace settings")?)
    } else {
        None
    };
    let mut runtime = Runtime::new(telemetry::tracer(&resources.telemetry.sdk))
        .with_parent_context(task.parent.clone());
    if let Some(prepared) = &task.prepared {
        runtime = runtime.with_deployment(prepared.deployment());
    }
    let mut slow_reported = false;
    let mut sink = |event: &RunEvent| -> Result<(), SinkError> {
        if let Some(trace) = trace.as_mut() {
            trace.emit(event)?;
        }
        if !event_fits(event) {
            return Err(SinkError::Capacity);
        }
        if task.events.is_full() && !slow_reported {
            eprintln!("pablo: ACP consumer slow; runtime waiting for output capacity");
            slow_reported = true;
        }
        // Only this owned thread waits; transport cancellation closes the queue.
        task.events
            .send_blocking(event.clone())
            .map_err(|_| io::Error::other("ACP consumer closed"))?;
        Ok(())
    };
    let task_tools = if let Some(prepared) = task
        .prepared
        .as_ref()
        .filter(|prepared| prepared.needs_async_tools())
    {
        let deadline = tokio::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(
                spec.limits.max_run_duration_ms,
            ))
            .ok_or("invalid run duration")?;
        Some(
            options
                .deployment
                .as_ref()
                .unwrap()
                .tools(prepared, deadline, &task.cancel)
                .await
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    match runtime
        .run_with_tools(
            spec,
            resources.provider.as_ref(),
            task_tools.as_ref().unwrap_or(&resources.tools),
            &task.cancel,
            &mut sink,
        )
        .await
    {
        Ok(outcome) => Ok(outcome),
        Err(RunError::EventDelivery { outcome, .. }) => Ok(*outcome),
        Err(_) => Err("runtime rejected the run".into()),
    }
}

fn event_fits(event: &RunEvent) -> bool {
    // Six bytes per input byte covers worst-case JSON escaping. Text is the hot
    // path; a conservative bound usually avoids serialization altogether.
    if let EventKind::TextDelta { text } = &event.kind {
        let strings = text
            .len()
            .saturating_add(event.schema_version.len())
            .saturating_add(event.run_id.len())
            .saturating_add(event.session_id.len())
            .saturating_add(event.trace_id.len())
            .saturating_add(event.span_id.len())
            .saturating_add(event.parent_span_id.as_deref().map_or(0, str::len))
            .saturating_add(event.trace_flags.len());
        if strings.saturating_mul(6).saturating_add(512) <= FRAME_BYTES {
            return true;
        }
    }
    // Exact fallback admits large but lightly escaped values without allocating
    // their JSON bytes. The actual ACP wire frame is checked at the SDK sink.
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|size| *size <= FRAME_BYTES)
                .ok_or(io::ErrorKind::FileTooLarge)?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), event).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_capacity_admits_large_plain_text_but_rejects_its_escaped_expansion() {
        let mut event = RunEvent {
            model_route: None,
            compaction: None,
            output_validation: None,
            output_repair: None,
            model_profile: None,
            deployment: None,
            accounting: None,
            schema_version: "c1.2".into(),
            seq: 1,
            timestamp_unix_micros: 0,
            run_id: "run".into(),
            session_id: "session".into(),
            trace_id: "trace".into(),
            span_id: "span".into(),
            parent_span_id: None,
            trace_flags: "01".into(),
            kind: EventKind::TextDelta {
                text: "a".repeat(FRAME_BYTES / 6 + 1024),
            },
        };
        assert!(event_fits(&event));
        event.kind = EventKind::TextDelta {
            text: "\0".repeat(FRAME_BYTES / 6 + 1024),
        };
        assert!(!event_fits(&event));
        event.kind = EventKind::TextDelta {
            text: "\0🦀\"\\".repeat(1000),
        };
        assert!(event_fits(&event));
    }
}
