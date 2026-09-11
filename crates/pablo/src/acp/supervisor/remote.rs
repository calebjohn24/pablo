//! Remote work uses the same queue, resource leases and native consumer as ACP.
use super::*;
use opentelemetry::trace::{TraceContextExt, Tracer};
use pablo_core::a2a::{
    self,
    lifecycle::Disposition,
    transport::{CancelReceipt, TaskClient},
};
#[derive(Clone, Debug, serde::Serialize)]
pub(in crate::acp) struct Snapshot {
    pub name: String,
    pub card_sha256: Option<String>,
    pub endpoint: Option<String>,
    pub protocol_version: &'static str,
    pub binding: &'static str,
    pub status: Option<a2a::wire::TaskState>,
    pub remote_reported_usage: Option<a2a::usage::ReportedUsage>,
    pub result: Option<a2a::lifecycle::ResultData>,
    pub remote: a2a::RemoteIdentity,
    pub delivery: pablo_core::DeliveryCertainty,
    pub cancellation: CancelReceipt,
}
pub(super) struct Job {
    pub child: Arc<Child>,
    pub lease: ResourceLease,
    pending: a2a::PendingRemote,
    request: a2a::request::SpawnRequest,
    parent: opentelemetry::Context,
    input_bytes: usize,
    duration_ms: u64,
    fixture: Option<(String, String)>,
}
impl Supervisor {
    pub fn spawn_remote(
        &self,
        request: &a2a::request::SpawnRequest,
        parent: opentelemetry::Context,
    ) -> Result<AgentRef, &'static str> {
        let fixture = self
            .inner
            .options
            .deployment
            .as_ref()
            .and_then(|bootstrap| bootstrap.fixture_a2a_endpoints.get(&request.remote))
            .map(|rpc| {
                let mut card = reqwest::Url::parse(rpc).expect("validated fixture");
                card.set_path("/.well-known/agent-card.json");
                (card.to_string(), rpc.clone())
            });
        self.spawn_remote_inner(request, parent, fixture)
    }
    fn spawn_remote_inner(
        &self,
        request: &a2a::request::SpawnRequest,
        parent: opentelemetry::Context,
        fixture: Option<(String, String)>,
    ) -> Result<AgentRef, &'static str> {
        request
            .validate_shape()
            .map_err(|_| "invalid remote input")?;
        let pending = self
            .inner
            .parent
            .deployment()
            .prepare_a2a(&self.inner.root, &request.remote)
            .map_err(|_| "remote authority rejected")?;
        let agent = pending.agent().clone();
        let input_bytes = serde_json::to_vec(request)
            .map_err(|_| "invalid remote input")?
            .len();
        let mut limits = self.inner.parent.spec().limits.clone();
        limits.max_run_duration_ms = limits
            .max_run_duration_ms
            .min(request.max_duration_ms.unwrap_or(a2a::wire::MAX_TASK_MS))
            .min(a2a::wire::MAX_TASK_MS);
        let duration_ms = limits.max_run_duration_ms;
        let mut registry = self.inner.registry.lock().unwrap();
        if registry.closed
            || self.inner.cancel.is_cancelled()
            || Instant::now() >= self.inner.ledger.deadline()
        {
            return Err("root closed");
        }
        let lease = self
            .inner
            .ledger
            .admit_child(
                &agent,
                limits,
                Resources {
                    pending_children: 1,
                    queued_input_bytes: input_bytes,
                    result_bytes: MAX_RESULT_BYTES,
                    ..Default::default()
                },
            )
            .map_err(|_| "remote admission rejected")?;
        let child = Arc::new(Child {
            snapshot: Mutex::new(super::Snapshot {
                agent: agent.clone(),
                accounting: Accounting::default(),
                result_id: None,
                handoffs: vec![],
                outcome: None,
                error: None,
                trace: None,
                validation: None,
                repair: None,
                remote: Some(Box::new(Snapshot {
                    name: request.remote.clone(),
                    card_sha256: None,
                    endpoint: None,
                    protocol_version: a2a::PROTOCOL_VERSION,
                    binding: "JSONRPC",
                    status: None,
                    remote_reported_usage: None,
                    result: None,
                    remote: a2a::RemoteIdentity::default(),
                    delivery: pablo_core::DeliveryCertainty::NotSent,
                    cancellation: CancelReceipt::NotNeeded,
                })),
            }),
            cancel: self.inner.cancel.child_token(),
            retained: Mutex::new(None),
        });
        registry
            .entries
            .insert(agent.agent_id().into(), child.clone());
        registry.queue.push_back(super::Job::Remote(Box::new(Job {
            child,
            lease,
            pending,
            request: request.clone(),
            parent,
            input_bytes,
            duration_ms,
            fixture,
        })));
        self.inner.ready.notify_one();
        Ok(agent)
    }
}
impl Inner {
    pub(super) async fn execute_remote(&self, mut job: Job) {
        if job.child.cancel.is_cancelled() {
            self.settle(job.child, job.lease, RunOutcome::Cancelled, None, None);
            return;
        }
        if job
            .lease
            .replace(Resources {
                active_children: 1,
                context_bytes: job.input_bytes,
                result_bytes: MAX_RESULT_BYTES,
                ..Default::default()
            })
            .is_err()
        {
            self.settle(
                job.child,
                job.lease,
                admission_failed(),
                Some("remote promotion rejected"),
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
                .expect("owned remote");
            snapshot.agent.clone()
        };
        let deadline =
            (Instant::now() + Duration::from_millis(job.duration_ms)).min(self.ledger.deadline());
        let mut terminal = None;
        let result: Result<RunOutcome, &'static str> = async {
            let otel = &self.parent.deployment().options()["otel"];
            let headers = if otel["sdk_disabled"] != true && otel["exporter"] == "otlp" {
                self.parent
                    .credential(
                        pablo_core::deployment::CredentialConsumer::OtelHeaders,
                        &pablo_core::deployment::ProcessCredentials,
                    )
                    .map_err(|_| "remote telemetry credentials rejected")?
            } else {
                None
            };
            let identity = self
                .ledger
                .bind_execution(agent.agent_id(), None)
                .map_err(|_| "remote execution identity rejected")?;
            let closing = self
                .ledger
                .claim_run_event(agent.agent_id())
                .map_err(|_| "remote closing capacity rejected")?;
            let telemetry = crate::otel::Telemetry::configured(&self.parent, headers.as_ref())
                .map_err(|_| "remote telemetry setup failed")?;
            job.child
                .snapshot
                .lock()
                .unwrap()
                .agent
                .bind_session(identity.agent.session_id().into())
                .expect("owned remote session");
            let tracer = pablo_core::telemetry::tracer(&telemetry.sdk);
            let started = timestamp();
            let attributes = vec![
                opentelemetry::KeyValue::new("pablo.run.id", identity.run_id.clone()),
                opentelemetry::KeyValue::new(
                    "pablo.agent.id",
                    identity.agent.agent_id().to_owned(),
                ),
                opentelemetry::KeyValue::new("pablo.agent.kind", "remote_a2a"),
                opentelemetry::KeyValue::new(
                    "pablo.agent.session.id",
                    identity.agent.session_id().to_owned(),
                ),
                opentelemetry::KeyValue::new(
                    "pablo.root.run.id",
                    identity.agent.root_run_id().to_owned(),
                ),
                opentelemetry::KeyValue::new(
                    "pablo.root.session.id",
                    identity.agent.root_session_id().to_owned(),
                ),
                opentelemetry::KeyValue::new(
                    "pablo.parent.agent.id",
                    agent.parent_agent_id().unwrap().to_owned(),
                ),
                opentelemetry::KeyValue::new("pablo.agent.depth", 1i64),
            ];
            let context = job.parent.with_span(
                tracer
                    .span_builder("agent.run")
                    .with_start_time(started)
                    .with_attributes(attributes)
                    .start_with_context(&tracer, &job.parent),
            );
            let parent_id = job
                .parent
                .span()
                .span_context()
                .is_valid()
                .then(|| job.parent.span().span_context().span_id().to_string());
            let mut sink = Sink {
                inner: self,
                child: job.child.clone(),
                identity,
                context: context.clone(),
                parent_id,
                seq: 0,
                started,
                output_bytes: self.parent.spec().limits.max_output_bytes,
                failure: None,
                closing: Some(closing),
            };
            let work: Result<RunOutcome, &'static str> = async {
                sink.emit(EventKind::RunStarted)
                    .await
                    .map_err(|_| "remote event delivery failed")?;
                let proxy = match &job.fixture {
                    Some((card, _)) => {
                        job.pending
                            .resolve_fixture(card, deadline, &job.child.cancel)
                            .await
                    }
                    None => job.pending.resolve(deadline, &job.child.cancel).await,
                };
                let proxy = match proxy {
                    Ok(proxy) => proxy,
                    Err(a2a::ResolveError::Fetch(a2a::FetchError::Cancelled)) => {
                        return Ok(RunOutcome::Cancelled);
                    }
                    Err(a2a::ResolveError::Fetch(a2a::FetchError::TimedOut)) => {
                        return Ok(RunOutcome::TimedOut);
                    }
                    Err(_) => return Ok(admission_failed()),
                };
                job.child
                    .snapshot
                    .lock()
                    .unwrap()
                    .remote
                    .as_mut()
                    .unwrap()
                    .card_sha256 = Some(proxy.card().card_sha256.clone());
                job.child
                    .snapshot
                    .lock()
                    .unwrap()
                    .remote
                    .as_mut()
                    .unwrap()
                    .endpoint = Some(proxy.card().endpoint.clone());
                let credential = if job.fixture.is_none() {
                    self.parent
                        .a2a_credential(proxy.name(), &pablo_core::deployment::ProcessCredentials)
                        .map_err(|_| "remote credential rejected")?
                } else {
                    None
                };
                let client = match &job.fixture {
                    Some((_, rpc)) => TaskClient::fixture(proxy.card(), rpc),
                    None => TaskClient::new(proxy.card(), credential.as_ref()),
                }
                .map_err(|_| "remote transport setup failed")?;
                let span = context.span();
                let sc = span.span_context();
                let trace = a2a::trace::TraceContext::new(
                    &format!(
                        "00-{}-{}-{:02x}",
                        sc.trace_id(),
                        sc.span_id(),
                        sc.trace_flags().to_u8()
                    ),
                    (!sc.trace_state().header().is_empty())
                        .then(|| sc.trace_state().header())
                        .as_deref(),
                )
                .ok();
                let modes = job
                    .request
                    .accepted_output_modes
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let execution = client
                    .execute(
                        a2a::transport::Request {
                            parts: &job.request.parts,
                            accepted_output_modes: &modes,
                            stream: job.request.stream.unwrap_or(proxy.card().streaming),
                            trace: trace.as_ref(),
                            deadline,
                            cleanup_deadline: deadline
                                + Duration::from_millis(a2a::wire::MAX_CANCEL_MS),
                        },
                        &job.child.cancel,
                        &mut sink,
                    )
                    .await;
                let outcome = sink.failure.take().unwrap_or_else(|| outcome(&execution));
                let mut snapshot = job.child.snapshot.lock().unwrap();
                let remote = snapshot.remote.as_mut().unwrap();
                remote.remote = execution.remote;
                remote.delivery = execution.delivery;
                remote.cancellation = execution.cancellation;
                remote.result = execution.result.ok();
                Ok(outcome)
            }
            .await;
            let outcome = work.unwrap_or_else(|error| RunOutcome::Failed {
                code: if error == "remote event delivery failed" {
                    pablo_core::FailureCode::EventSinkIo
                } else {
                    pablo_core::FailureCode::ChildAdmission
                },
                delivery: job
                    .child
                    .snapshot
                    .lock()
                    .unwrap()
                    .remote
                    .as_ref()
                    .unwrap()
                    .delivery,
            });
            let outcome = sink.failure.take().unwrap_or(outcome);
            context.span().set_attribute(opentelemetry::KeyValue::new(
                "pablo.run.outcome",
                outcome.label(),
            ));
            terminal = sink
                .emit(EventKind::RunFinished {
                    outcome: outcome.clone(),
                })
                .await
                .ok();
            context.span().end_with_timestamp(
                terminal
                    .as_ref()
                    .map(|event| {
                        std::time::UNIX_EPOCH + Duration::from_micros(event.timestamp_unix_micros)
                    })
                    .unwrap_or_else(timestamp),
            );
            telemetry.shutdown().await;
            if terminal.is_none() {
                return Err("remote terminal delivery failed");
            }
            Ok(outcome)
        }
        .await;
        let (outcome, error) = match result {
            Ok(outcome) => (outcome, None),
            Err(error) => (
                RunOutcome::Failed {
                    code: if error == "remote terminal delivery failed" {
                        pablo_core::FailureCode::EventSinkIo
                    } else {
                        pablo_core::FailureCode::ChildAdmission
                    },
                    delivery: job
                        .child
                        .snapshot
                        .lock()
                        .unwrap()
                        .remote
                        .as_ref()
                        .unwrap()
                        .delivery,
                },
                Some(error),
            ),
        };
        self.settle(job.child, job.lease, outcome, error, terminal);
    }
}
fn outcome(execution: &a2a::transport::Execution) -> RunOutcome {
    use a2a::transport::Error;
    match &execution.result {
        Ok(result) if result.disposition == Some(Disposition::Completed) => RunOutcome::Completed {
            output: String::new(),
            finish_reason: pablo_core::FinishReason::Stop,
            usage: pablo_core::Usage::default(),
        },
        Ok(result) if result.disposition == Some(Disposition::Canceled) => RunOutcome::Cancelled,
        Err(Error::Sink) => RunOutcome::Failed {
            code: pablo_core::FailureCode::EventSinkIo,
            delivery: execution.delivery,
        },
        Err(Error::Cancelled) => RunOutcome::Cancelled,
        Err(Error::TimedOut | Error::Idle) => RunOutcome::TimedOut,
        Err(
            Error::Wire(a2a::wire::Error::Bound)
            | Error::Lifecycle(
                a2a::lifecycle::Error::Bound | a2a::lifecycle::Error::Wire(a2a::wire::Error::Bound),
            ),
        ) => RunOutcome::LimitExceeded {
            limit: LimitKind::OutputBytes,
        },
        _ => RunOutcome::Failed {
            code: pablo_core::FailureCode::RemoteTask,
            delivery: execution.delivery,
        },
    }
}
struct Sink<'a> {
    inner: &'a Inner,
    child: Arc<Child>,
    identity: pablo_core::children::ExecutionIdentity,
    context: opentelemetry::Context,
    parent_id: Option<String>,
    seq: u64,
    started: std::time::SystemTime,
    output_bytes: usize,
    failure: Option<RunOutcome>,
    closing: Option<pablo_core::children::ledger::events::EventReservation>,
}
impl Sink<'_> {
    async fn emit(&mut self, kind: EventKind) -> Result<RunEvent, a2a::transport::Error> {
        let next_seq = self.seq.checked_add(1).ok_or(a2a::transport::Error::Sink)?;
        let span = self.context.span();
        let sc = span.span_context();
        let time = if next_seq == 1 {
            self.started
        } else {
            timestamp()
        };
        let micros = time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros()
            .min(u64::MAX as u128) as u64;
        let event = RunEvent {
            root_seq: None,
            agent: Some(Box::new(self.identity.agent.clone())),
            output_repair: None,
            compaction: None,
            output_validation: None,
            model_route: None,
            model_profile: None,
            deployment: None,
            accounting: matches!(&kind, EventKind::RunFinished { .. }).then(|| {
                Box::new(
                    self.inner
                        .ledger
                        .agent(self.identity.agent.agent_id())
                        .expect("owned remote accounting"),
                )
            }),
            schema_version: pablo_core::SCHEMA_VERSION.into(),
            seq: next_seq,
            timestamp_unix_micros: micros,
            run_id: self.identity.run_id.clone(),
            session_id: self.identity.agent.session_id().into(),
            trace_id: sc.trace_id().to_string(),
            span_id: sc.span_id().to_string(),
            parent_span_id: self.parent_id.clone(),
            trace_flags: format!("{:02x}", sc.trace_flags().to_u8()),
            kind,
        };
        if matches!(event.kind, EventKind::RunFinished { .. }) {
            self.closing
                .take()
                .ok_or(a2a::transport::Error::Sink)?
                .consume_record(&event)
        } else {
            self.inner
                .ledger
                .admit_event_record(self.identity.agent.agent_id(), &event)
        }
        .map_err(|error| {
            self.failure = Some(match error {
                pablo_core::children::ledger::AdmissionError::Outcome(outcome) => outcome,
                pablo_core::children::ledger::AdmissionError::Closed
                    if self.child.cancel.is_cancelled() =>
                {
                    RunOutcome::Cancelled
                }
                _ => RunOutcome::Failed {
                    code: pablo_core::FailureCode::EventSinkIo,
                    delivery: self
                        .child
                        .snapshot
                        .lock()
                        .unwrap()
                        .remote
                        .as_ref()
                        .unwrap()
                        .delivery,
                },
            });
            a2a::transport::Error::Sink
        })?;
        self.seq = next_seq;
        span.add_event_with_timestamp(
            match event.kind {
                EventKind::RunStarted => "run.started",
                EventKind::RunFinished { .. } => "run.finished",
                _ => "a2a.update",
            },
            time,
            vec![opentelemetry::KeyValue::new(
                "pablo.event.seq",
                self.seq as i64,
            )],
        );
        let (consumed, receipt) = tokio::sync::oneshot::channel();
        let agent = self.child.snapshot.lock().unwrap().agent.clone();
        tokio::time::timeout(Duration::from_secs(2), async {
            self.inner
                .updates
                .send(super::Update {
                    agent,
                    update: delivery::NativeUpdate {
                        event: event.clone(),
                        consumed,
                    },
                })
                .await
                .map_err(|_| a2a::transport::Error::Sink)?;
            receipt.await.map_err(|_| a2a::transport::Error::Sink)
        })
        .await
        .map_err(|_| a2a::transport::Error::Sink)??;
        Ok(event)
    }
}
impl a2a::transport::Sink for Sink<'_> {
    fn update<'a>(
        &'a mut self,
        kind: a2a::lifecycle::Update,
        result: &'a a2a::lifecycle::ResultData,
    ) -> futures::future::BoxFuture<'a, Result<(), a2a::transport::Error>> {
        Box::pin(async move {
            {
                let mut snapshot = self.child.snapshot.lock().unwrap();
                let remote = snapshot.remote.as_mut().unwrap();
                remote.remote = result.remote.clone();
                remote.status = result.status;
                remote.remote_reported_usage = result.remote_reported_usage;
                remote.delivery = pablo_core::DeliveryCertainty::ResponseReceived;
            }
            if result
                .output_bytes()
                .map_or(true, |bytes| bytes > self.output_bytes)
            {
                self.failure = Some(RunOutcome::LimitExceeded {
                    limit: LimitKind::OutputBytes,
                });
                return Err(a2a::transport::Error::Sink);
            }
            self.inner.changed.notify_waiters();
            self.emit(EventKind::A2aUpdate {
                remote: a2a::events::Record::update(kind, result),
            })
            .await
            .map(|_| ())
        })
    }
}

fn timestamp() -> std::time::SystemTime {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u64::MAX as u128) as u64;
    std::time::UNIX_EPOCH + Duration::from_micros(micros)
}
