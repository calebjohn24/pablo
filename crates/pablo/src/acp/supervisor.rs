//! Root-owned temporary children. The host must await close before root settlement.
//! Configured CLI and ACP roots share this supervisor and its native consumer.
use super::*;
use pablo_core::{
    children::{
        AgentRef, AgentState, MAX_RESULT_BYTES, MAX_WAIT_MS, WaitMode,
        ledger::{
            RootLedger,
            resources::{ResourceLease, Resources},
        },
    },
    deployment::PreparedRun,
    task::Accounting,
};
use std::collections::{BTreeMap, VecDeque};
use tokio::time::Instant;
pub(crate) mod factory;

#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct Snapshot {
    pub agent: AgentRef,
    pub accounting: Accounting,
    pub outcome: Option<RunOutcome>,
    pub error: Option<&'static str>,
    pub trace: Option<Trace>,
    pub validation: Option<Box<pablo_core::output::OutputValidation>>,
    pub repair: Option<Box<pablo_core::output::OutputRepair>>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(super) struct Trace {
    pub run_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
}
#[derive(serde::Serialize)]
pub(super) struct WaitResult {
    pub settled: Vec<Snapshot>,
    pub remaining: Vec<AgentRef>,
}
/// Original native records stay bounded; acknowledgement belongs to the root consumer.
/// The routing handle supplements the immutable identity already carried by the event.
pub(super) struct Update {
    pub agent: AgentRef,
    pub update: delivery::NativeUpdate,
}
struct Child {
    snapshot: Mutex<Snapshot>,
    cancel: CancellationToken,
    retained: Mutex<Option<ResourceLease>>,
}
struct Job {
    child: Arc<Child>,
    prepared: PreparedRun,
    parent: opentelemetry::Context,
    lease: ResourceLease,
}
#[derive(Default)]
struct Registry {
    entries: BTreeMap<String, Arc<Child>>,
    queue: VecDeque<Job>,
    closed: bool,
}
struct Inner {
    root_claimed: std::sync::atomic::AtomicBool,
    root_cancel: CancellationToken,
    root: AgentRef,
    parent: PreparedRun,
    parent_tools: Arc<pablo_core::tool::ToolRegistry>,
    root_mcp: Mutex<Option<Arc<ResourceLease>>>,
    ledger: RootLedger,
    options: Arc<Options>,
    registry: Mutex<Registry>,
    cancel: CancellationToken,
    ready: Notify,
    changed: Notify,
    updates: async_channel::Sender<Update>,
}
pub(super) struct Supervisor {
    inner: Arc<Inner>,
    join: tokio::sync::Mutex<Shutdown>,
}
struct Shutdown {
    task: Option<tokio::task::JoinHandle<Result<(), &'static str>>>,
    result: Option<Result<(), &'static str>>,
}
impl Supervisor {
    /// The host joins this consumer alongside its root run. Receiver loss wakes
    /// the supervisor; sink failure cancels the root and never acknowledges loss.
    pub async fn consume_updates<S: pablo_core::EventSink>(
        updates: async_channel::Receiver<Update>,
        mut sink: pablo_core::events::tree::TreeSink<S>,
        root_cancel: CancellationToken,
    ) -> Result<(), pablo_core::SinkError> {
        while let Ok(update) = updates.recv().await {
            if let Err(error) = sink.emit_owned(update.update.event) {
                updates.close();
                root_cancel.cancel();
                return Err(error);
            }
            // Cancellation can retire the producer's receipt before we drain its
            // queued record. Successful delivery remains successful in that race.
            let _ = update.update.consumed.send(());
        }
        Ok(())
    }

    pub fn new(
        options: Options,
        root: AgentRef,
        ledger: RootLedger,
        root_cancel: &CancellationToken,
        parent: PreparedRun,
        parent_tools: Arc<pablo_core::tool::ToolRegistry>,
    ) -> Result<(Self, async_channel::Receiver<Update>), &'static str> {
        Self::with_root_resources(
            Arc::new(options),
            root,
            ledger,
            root_cancel,
            parent,
            parent_tools,
            None,
        )
    }
    fn with_root_resources(
        options: Arc<Options>,
        root: AgentRef,
        ledger: RootLedger,
        root_cancel: &CancellationToken,
        parent: PreparedRun,
        parent_tools: Arc<pablo_core::ToolRegistry>,
        root_mcp: Option<Arc<ResourceLease>>,
    ) -> Result<(Self, async_channel::Receiver<Update>), &'static str> {
        let required = parent
            .mcp_resources()
            .map_err(|_| "invalid root MCP capacity")?;
        if required.mcp_sessions != 0
            && root_mcp
                .as_ref()
                .is_none_or(|lease| !lease.covers(&ledger, root.agent_id(), required))
        {
            return Err("root MCP capacity missing");
        }
        if parent.is_child()
            || root.depth() != 0
            || ledger.agent(root.agent_id()).is_none()
            || options.deployment.is_none()
            || tokio::runtime::Handle::try_current().is_err()
        {
            return Err("invalid supervisor owner");
        }
        if parent.trace_path().is_some() {
            ledger
                .configure_trace(&parent.spec().trace)
                .map_err(|_| "invalid supervisor trace capacity")?;
        }
        let (updates, receiver) = async_channel::bounded(QUEUE_EVENTS);
        let inner = Arc::new(Inner {
            root_claimed: std::sync::atomic::AtomicBool::new(false),
            root_cancel: root_cancel.clone(),
            root,
            parent,
            parent_tools,
            root_mcp: Mutex::new(root_mcp),
            ledger,
            options,
            registry: Mutex::default(),
            cancel: root_cancel.child_token(),
            ready: Notify::new(),
            changed: Notify::new(),
            updates,
        });
        let owned = inner.clone();
        let join = tokio::spawn(async move {
            owned.run().await;
            let result = owned
                .parent_tools
                .close()
                .await
                .map_err(|_| "root MCP cleanup failed");
            owned.root_mcp.lock().unwrap().take();
            result
        });
        Ok((
            Self {
                inner,
                join: tokio::sync::Mutex::new(Shutdown {
                    task: Some(join),
                    result: None,
                }),
            },
            receiver,
        ))
    }
    /// Project the fixed parent's authority, then atomically admit queued ownership.
    /// No task or provider future is awaited before the owned handle is returned.
    pub fn spawn(
        &self,
        request: &pablo_core::children::SpawnRequest,
        parent: opentelemetry::Context,
    ) -> Result<AgentRef, &'static str> {
        let prepared = self
            .inner
            .parent
            .prepare_child(request, &self.inner.parent_tools)
            .map_err(|_| "child authority rejected")?;
        let mut registry = self.inner.registry.lock().unwrap();
        if registry.closed
            || self.inner.cancel.is_cancelled()
            || Instant::now() >= self.inner.ledger.deadline()
        {
            return Err("root closed");
        }
        let agent = self
            .inner
            .root
            .temporary_child()
            .map_err(|_| "child depth rejected")?;
        let lease = self
            .inner
            .ledger
            .admit_child(
                &agent,
                prepared.spec().limits.clone(),
                Resources {
                    pending_children: 1,
                    queued_input_bytes: prepared.spec().input.len(),
                    result_bytes: MAX_RESULT_BYTES,
                    ..Resources::default()
                },
            )
            .map_err(|_| "child admission rejected")?;
        let child = Arc::new(Child {
            snapshot: Mutex::new(Snapshot {
                agent: agent.clone(),
                accounting: Accounting::default(),
                outcome: None,
                error: None,
                trace: None,
                validation: None,
                repair: None,
            }),
            cancel: self.inner.cancel.child_token(),
            retained: Mutex::new(None),
        });
        registry
            .entries
            .insert(agent.agent_id().into(), child.clone());
        registry.queue.push_back(Job {
            child,
            prepared,
            parent,
            lease,
        });
        self.inner.ready.notify_one();
        Ok(agent)
    }
    fn child(&self, id: &str) -> Result<Arc<Child>, &'static str> {
        self.inner
            .registry
            .lock()
            .unwrap()
            .entries
            .get(id)
            .cloned()
            .ok_or("unknown child")
    }
    pub fn inspect(&self, id: &str) -> Result<Snapshot, &'static str> {
        let child = self.child(id)?;
        let mut snapshot = child.snapshot.lock().unwrap().clone();
        snapshot.accounting = self.inner.ledger.agent(id).expect("registered child");
        Ok(snapshot)
    }
    pub async fn wait(
        &self,
        ids: &[String],
        mode: WaitMode,
        timeout_ms: u64,
    ) -> Result<WaitResult, &'static str> {
        let action = pablo_core::children::ChildAction::Wait {
            agent_ids: ids.to_vec(),
            mode,
            timeout_ms,
        };
        action
            .validate_shape()
            .map_err(|_| "invalid wait selection")?;
        for id in ids {
            self.child(id)?;
        }
        let deadline = (Instant::now() + Duration::from_millis(timeout_ms.min(MAX_WAIT_MS)))
            .min(self.inner.ledger.deadline());
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let result = self.snapshots(ids)?;
            if result.remaining.is_empty()
                || (matches!(mode, WaitMode::Any) && !result.settled.is_empty())
                || Instant::now() >= deadline
            {
                return Ok(result);
            }
            tokio::select! { _ = changed => {}, _ = tokio::time::sleep_until(deadline) => {} }
        }
    }
    fn snapshots(&self, ids: &[String]) -> Result<WaitResult, &'static str> {
        let mut result = WaitResult {
            settled: vec![],
            remaining: vec![],
        };
        for id in ids {
            let snapshot = self.inspect(id)?;
            if snapshot.agent.state() == AgentState::Settled {
                result.settled.push(snapshot);
            } else {
                result.remaining.push(snapshot.agent);
            }
        }
        Ok(result)
    }
    pub async fn stop(&self, id: &str) -> Result<Snapshot, &'static str> {
        let child = self.child(id)?;
        {
            let mut snapshot = child.snapshot.lock().unwrap();
            if snapshot.agent.state() != AgentState::Settled {
                if snapshot.agent.state() != AgentState::Stopping {
                    snapshot
                        .agent
                        .transition(AgentState::Stopping, None)
                        .expect("owned transition");
                }
                child.cancel.cancel();
            }
        }
        let queued = {
            let mut registry = self.inner.registry.lock().unwrap();
            registry
                .queue
                .iter()
                .position(|job| job.child.snapshot.lock().unwrap().agent.agent_id() == id)
                .and_then(|index| registry.queue.remove(index))
        };
        if let Some(job) = queued {
            self.inner
                .settle(job.child, job.lease, RunOutcome::Cancelled, None, None);
        }
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let snapshot = self.inspect(id)?;
            if snapshot.agent.state() == AgentState::Settled {
                return Ok(snapshot);
            }
            changed.await;
        }
    }
    pub async fn close(&self) -> Result<(), &'static str> {
        pablo_core::children::owner::RootOwner::cancel(self);
        let mut join = self.join.lock().await;
        if let Some(result) = join.result {
            return result;
        }
        let result = if let Some(task) = join.task.as_mut() {
            task.await
                .map_err(|_| "child supervisor cleanup failed")
                .and_then(|result| result)
        } else {
            Ok(())
        };
        join.task = None;
        join.result = Some(result);
        result
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.inner.cancel.cancel();
        self.inner.updates.close();
    }
}
impl Inner {
    fn settle(
        &self,
        child: Arc<Child>,
        mut lease: ResourceLease,
        outcome: RunOutcome,
        error: Option<&'static str>,
        terminal: Option<RunEvent>,
    ) {
        lease
            .reduce(Resources {
                result_bytes: MAX_RESULT_BYTES,
                ..Resources::default()
            })
            .expect("joined execution only releases capacity");
        let mut snapshot = child.snapshot.lock().unwrap();
        snapshot.outcome = Some(outcome);
        snapshot.error = error;
        snapshot.accounting = self
            .ledger
            .agent(snapshot.agent.agent_id())
            .expect("registered child");
        if let Some(event) = terminal {
            snapshot.trace = Some(Trace {
                run_id: event.run_id,
                trace_id: event.trace_id,
                span_id: event.span_id,
                parent_span_id: event.parent_span_id,
            });
            snapshot.validation = event.output_validation;
            snapshot.repair = event.output_repair;
        }
        snapshot
            .agent
            .transition(AgentState::Settled, None)
            .expect("one settlement");
        // Refuse oversize results without corrupting a successful structured value.
        if serde_json::to_vec(&*snapshot).map_or(true, |bytes| bytes.len() > MAX_RESULT_BYTES) {
            snapshot.outcome = Some(RunOutcome::LimitExceeded {
                limit: LimitKind::OutputBytes,
            });
            snapshot.validation = None;
            snapshot.repair = None;
            snapshot.error = Some("child result exceeds retained bound");
        }
        *child.retained.lock().unwrap() = Some(lease);
        drop(snapshot);
        self.changed.notify_waiters();
    }
    async fn run(&self) {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if self.cancel.is_cancelled()
                || self.updates.is_closed()
                || Instant::now() >= self.ledger.deadline()
            {
                break;
            }
            let job = self.registry.lock().unwrap().queue.pop_front();
            if let Some(job) = job {
                self.execute(job).await;
            } else {
                tokio::select! {
                    _ = ready => {}, _ = self.cancel.cancelled() => {},
                    _ = self.updates.closed() => {}, _ = tokio::time::sleep_until(self.ledger.deadline()) => {},
                }
            }
        }
        self.cancel.cancel();
        let queue = {
            let mut registry = self.registry.lock().unwrap();
            registry.closed = true;
            std::mem::take(&mut registry.queue)
        };
        for job in queue {
            let outcome = if Instant::now() >= self.ledger.deadline() {
                RunOutcome::TimedOut
            } else {
                RunOutcome::Cancelled
            };
            self.settle(job.child, job.lease, outcome, None, None);
        }
        self.updates.close();
    }
    async fn execute(&self, mut job: Job) {
        if job.child.cancel.is_cancelled() {
            self.settle(job.child, job.lease, RunOutcome::Cancelled, None, None);
            return;
        }
        let capabilities = match job.prepared.mcp_resources() {
            Ok(resources) => resources,
            Err(_) => {
                self.settle(
                    job.child,
                    job.lease,
                    admission_failed(),
                    Some("child MCP capacity invalid"),
                    None,
                );
                return;
            }
        };
        if job
            .lease
            .replace(Resources {
                active_children: 1,
                context_bytes: job.prepared.spec().limits.max_context_bytes,
                result_bytes: MAX_RESULT_BYTES,
                ..capabilities
            })
            .is_err()
        {
            self.settle(
                job.child,
                job.lease,
                admission_failed(),
                Some("child promotion rejected"),
                None,
            );
            return;
        }
        let agent = {
            let mut snapshot = job.child.snapshot.lock().unwrap();
            if snapshot.agent.state() == AgentState::Stopping {
                drop(snapshot);
                self.settle(job.child, job.lease, RunOutcome::Cancelled, None, None);
                return;
            }
            snapshot
                .agent
                .transition(AgentState::Starting, None)
                .expect("queued child");
            snapshot.agent.clone()
        };
        let input = job.prepared.spec().input.clone();
        let cwd = job.prepared.spec().workspace.clone();
        let lease = Arc::new(job.lease);
        let bound = in_memory::Dispatcher::bind_child(
            self.options.clone(),
            job.prepared,
            self.ledger.clone(),
            &agent,
            job.child.cancel.clone(),
            job.parent,
            lease.clone(),
        );
        let mut terminal = None;
        let mut error = None;
        let outcome = if let Ok((dispatcher, _typed_updates)) = bound {
            let (dispatcher, updates) = dispatcher.native_output();
            let session = dispatcher
                .initialize(wire::InitializeRequest::new(
                    agent_client_protocol::schema::ProtocolVersion::V1,
                ))
                .and_then(|_| dispatcher.new_session(wire::NewSessionRequest::new(cwd)));
            let outcome = if let Ok(session) = session {
                {
                    let mut snapshot = job.child.snapshot.lock().unwrap();
                    snapshot
                        .agent
                        .bind_session(session.session_id.to_string())
                        .expect("admitted session");
                }
                self.changed.notify_waiters();
                let (receipt, completed) = tokio::sync::oneshot::channel();
                let request = wire::PromptRequest::new(
                    session.session_id,
                    vec![wire::ContentBlock::Text(wire::TextContent::new(input))],
                );
                let prompt = dispatcher.prompt_observed(request, &job.child.cancel, receipt);
                tokio::pin!(prompt);
                let mut stopping = false;
                'forwarding: loop {
                    tokio::select! {
                        biased;
                        _ = job.child.cancel.cancelled(), if !stopping => { stopping = true; },
                        _ = tokio::time::sleep_until(self.ledger.deadline()), if !stopping => {
                            job.child.cancel.cancel(); stopping = true;
                        },
                        _ = self.updates.closed() => {
                            error = Some("root update receiver closed");
                            job.child.cancel.cancel(); let _ = dispatcher.close().await; let _ = prompt.await; break;
                        },
                        result = &mut prompt => { if result.is_err() { error = Some("child event delivery failed"); } break; },
                        update = updates.recv() => {
                            if let Ok(update) = update {
                                let agent = job.child.snapshot.lock().unwrap().agent.clone();
                                let send = self.updates.send(Update {agent, update});
                                tokio::pin!(send);
                                loop {
                                    tokio::select! {
                                        biased;
                                        _ = job.child.cancel.cancelled(), if !stopping => { stopping = true; },
                                        _ = tokio::time::sleep_until(self.ledger.deadline()), if !stopping => {
                                            job.child.cancel.cancel(); stopping = true;
                                        },
                                        result = &mut prompt => {
                                            if result.is_err() { error = Some("child event delivery failed"); }
                                            break 'forwarding;
                                        },
                                        result = &mut send => {
                                            if result.is_err() {
                                                job.child.cancel.cancel(); error = Some("root update receiver closed");
                                                let _ = dispatcher.close().await; let _ = prompt.await;
                                                break 'forwarding;
                                            }
                                            break;
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
                // A prompt which failed before admission drops its receipt sender.
                match completed.await {
                    Ok(completion) => {
                        terminal = completion.terminal;
                        completion.outcome.unwrap_or_else(|_| {
                            error = Some("child execution failed");
                            admission_failed()
                        })
                    }
                    Err(_) if job.child.cancel.is_cancelled() => RunOutcome::Cancelled,
                    Err(_) => {
                        error = Some("child setup failed");
                        admission_failed()
                    }
                }
            } else if job.child.cancel.is_cancelled() {
                RunOutcome::Cancelled
            } else {
                error = Some("child session admission failed");
                admission_failed()
            };
            if dispatcher.close().await.is_err() {
                error = Some("child cleanup failed");
            }
            drop(dispatcher);
            outcome
        } else {
            error = Some("child dispatch admission failed");
            admission_failed()
        };
        let lease = Arc::try_unwrap(lease)
            .unwrap_or_else(|_| panic!("joined dispatcher releases its lease"));
        self.settle(job.child, lease, outcome, error, terminal);
    }
}
fn admission_failed() -> RunOutcome {
    RunOutcome::Failed {
        code: pablo_core::FailureCode::ChildAdmission,
        delivery: pablo_core::DeliveryCertainty::NotSent,
    }
}

#[cfg(test)]
mod tests;

mod tool;
