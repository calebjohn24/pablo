use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    cwd: std::path::PathBuf,
    supervisor: Arc<Supervisor>,
    updates: async_channel::Receiver<Update>,
    root_cancel: CancellationToken,
    ledger: RootLedger,
}
impl Fixture {
    fn new(endpoint: &str) -> Self {
        Self::with_duration(endpoint, 900_000)
    }
    fn with_duration(endpoint: &str, duration: u64) -> Self {
        Self::with_settings(endpoint, duration, "")
    }
    fn with_settings(endpoint: &str, duration: u64, extra: &str) -> Self {
        let cwd = std::env::temp_dir().join(format!("pablo-supervisor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&cwd).unwrap();
        let cwd = cwd.canonicalize().unwrap();
        let entry = cwd.join("entry.toml");
        std::fs::write(
            &entry,
            include_str!(
                "../../../../../docs/project/fixtures/c3-model-routes/three-providers.toml"
            )
            .replace("max_model_calls=2", "max_model_calls=20")
            .replace(
                "max_tool_calls=1",
                &format!("max_tool_calls=1\nmax_run_duration_ms={duration}"),
            ) + extra,
        )
        .unwrap();
        let options = Options::parse(
            "acp".into(),
            [
                "--stdio".into(),
                "--config".into(),
                entry.into_os_string(),
                "--bind".into(),
                format!("workspace={}", cwd.display()).into(),
                "--fixture-endpoint".into(),
                endpoint.into(),
            ]
            .into_iter(),
        )
        .unwrap();
        let parent = options
            .prepare_run(
                Some("parent private transcript".into()),
                Some(cwd.clone()),
                Some("root-session".into()),
            )
            .unwrap()
            .unwrap();
        let tools = Arc::new(parent.tools().unwrap());
        let root = AgentRef::root(uuid::Uuid::new_v4().to_string(), "root-session".into());
        let ledger = RootLedger::new(&root, parent.spec().limits.clone()).unwrap();
        let root_cancel = CancellationToken::new();
        let (supervisor, updates) =
            Supervisor::new(options, root, ledger.clone(), &root_cancel, parent, tools).unwrap();
        Self {
            cwd,
            supervisor: Arc::new(supervisor),
            updates,
            root_cancel,
            ledger,
        }
    }
    fn spawn(&self, input: &str) -> AgentRef {
        let request = serde_json::from_value(
            json!({"input":input,"capabilities":{"model_route":["secondary"],"tools":[]}}),
        )
        .unwrap();
        self.supervisor
            .spawn(&request, opentelemetry::Context::new())
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cwd);
    }
}
async fn request(stream: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = vec![];
    loop {
        let mut buffer = [0; 4096];
        let n = stream.read(&mut buffer).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buffer[..n]);
        assert!(bytes.len() < 1024 * 1024);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let length: usize = String::from_utf8_lossy(&bytes[..end])
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|s| s.parse().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}
async fn held(stream: &mut tokio::net::TcpStream) {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 99999\r\nConnection: close\r\n\r\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"working\"},\"finish_reason\":null}]}\n\n").await.unwrap();
}
async fn done(stream: &mut tokio::net::TcpStream) {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"finished child\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn immediate_handles_fifo_queue_stop_wait_and_joined_close() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let first = fixture.spawn("first selected task");
        let second = fixture.spawn("second selected task");
        let third = fixture.spawn("third selected task");
        assert_eq!(first.state(), AgentState::Queued);
        assert_eq!(
            fixture.ledger.total().model_calls,
            0,
            "spawn returns before a provider task can run"
        );
        assert!(
            fixture
                .supervisor
                .inspect(&uuid::Uuid::new_v4().to_string())
                .is_err()
        );
        let bad: pablo_core::children::SpawnRequest = serde_json::from_value(
            json!({"input":"denied","capabilities":{"tools":["unapproved"]}}),
        )
        .unwrap();
        assert!(
            fixture
                .supervisor
                .spawn(&bad, opentelemetry::Context::new())
                .is_err()
        );
        let selected = vec![
            third.agent_id().into(),
            first.agent_id().into(),
            second.agent_id().into(),
        ];
        assert_eq!(
            fixture
                .supervisor
                .wait(&selected, WaitMode::All, 0)
                .await
                .unwrap()
                .remaining
                .len(),
            3
        );
        let first_update = Arc::new(Notify::new());
        let drain = tokio::spawn({
            let first_update = first_update.clone();
            let updates = fixture.updates.clone();
            async move {
                let mut agents = vec![];
                while let Ok(update) = updates.recv().await {
                    agents.push(update.agent.agent_id().to_owned());
                    let _ = update.update.consumed.send(());
                    first_update.notify_one();
                }
                agents
            }
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = request(&mut stream).await;
        assert!(body.to_string().contains("first selected task"));
        assert!(!body.to_string().contains("parent private transcript"));
        held(&mut stream).await;
        first_update.notified().await;
        let stopped = fixture.supervisor.stop(second.agent_id()).await.unwrap();
        assert_eq!(stopped.outcome, Some(RunOutcome::Cancelled));
        assert_eq!(fixture.ledger.resources().pending_children, 1);
        let any = fixture
            .supervisor
            .wait(&selected, WaitMode::Any, 1000)
            .await
            .unwrap();
        assert_eq!(any.settled.len(), 1);
        assert_eq!(any.settled[0].agent.agent_id(), second.agent_id());
        assert_eq!(any.remaining[0].agent_id(), third.agent_id());
        let stopped = fixture.supervisor.stop(first.agent_id()).await.unwrap();
        assert_eq!(stopped.outcome, Some(RunOutcome::Cancelled));
        let mut byte = [0; 1];
        assert_eq!(
            stream.read(&mut byte).await.unwrap(),
            0,
            "stop joined the held provider"
        );
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(
            request(&mut stream)
                .await
                .to_string()
                .contains("third selected task")
        );
        done(&mut stream).await;
        let all = fixture
            .supervisor
            .wait(&selected, WaitMode::All, 5000)
            .await
            .unwrap();
        assert!(all.remaining.is_empty());
        assert_eq!(all.settled[0].agent.agent_id(), third.agent_id());
        assert!(all.settled[0].outcome.as_ref().unwrap().is_completed());
        assert!(all.settled[0].trace.is_some());
        let unchanged = fixture.supervisor.stop(third.agent_id()).await.unwrap();
        assert_eq!(unchanged.outcome, all.settled[0].outcome);
        assert!(!fixture.root_cancel.is_cancelled());
        assert_eq!(fixture.ledger.total().model_calls, 2);
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_eq!(fixture.ledger.resources().context_bytes, 0);
        assert_eq!(
            fixture.ledger.resources().result_bytes,
            3 * MAX_RESULT_BYTES
        );
        let (a, b) = tokio::join!(fixture.supervisor.close(), fixture.supervisor.close());
        a.unwrap();
        b.unwrap();
        let updates = drain.await.unwrap();
        assert!(updates.iter().any(|id| id == first.agent_id()));
        assert!(updates.iter().any(|id| id == third.agent_id()));
        assert!(!updates.iter().any(|id| id == second.agent_id()));
        assert!(
            fixture
                .supervisor
                .spawn(&bad, opentelemetry::Context::new())
                .is_err()
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stalled_ack_root_cancel_and_dropped_close_caller_cannot_abandon_children() {
    use std::{future::Future, task::Poll};
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let first = fixture.spawn("held");
        let second = fixture.spawn("queued");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        let held = fixture.updates.recv().await.unwrap();
        fixture.root_cancel.cancel();
        let mut close = Box::pin(fixture.supervisor.close());
        futures::future::poll_fn(|cx| {
            assert!(close.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(close);
        fixture.supervisor.close().await.unwrap();
        drop(held);
        root_owner::assert_closed(&mut stream).await;
        assert_eq!(
            fixture
                .supervisor
                .inspect(first.agent_id())
                .unwrap()
                .outcome,
            Some(RunOutcome::Cancelled)
        );
        assert_eq!(
            fixture
                .supervisor
                .inspect(second.agent_id())
                .unwrap()
                .outcome,
            Some(RunOutcome::Cancelled)
        );
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_eq!(fixture.ledger.resources().pending_children, 0);
        assert_eq!(fixture.ledger.total().model_calls, 1);
        assert!(
            fixture
                .supervisor
                .inspect(first.agent_id())
                .unwrap()
                .trace
                .is_some(),
            "native terminal survives a stalled ACP acknowledgement"
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn lost_receiver_joins_active_work_and_context_denial_never_dispatches() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let root_context = fixture
            .ledger
            .reserve_resources(
                fixture.supervisor.inner.root.agent_id(),
                Resources {
                    context_bytes: fixture
                        .supervisor
                        .inner
                        .parent
                        .spec()
                        .limits
                        .max_context_bytes,
                    ..Resources::default()
                },
            )
            .unwrap();
        let rejected = fixture.spawn("cannot fit alongside root");
        let result = fixture
            .supervisor
            .wait(&[rejected.agent_id().into()], WaitMode::All, 1000)
            .await
            .unwrap();
        assert!(result.remaining.is_empty());
        assert_eq!(result.settled[0].outcome, Some(admission_failed()));
        assert_eq!(fixture.ledger.total().model_calls, 0);
        assert_eq!(fixture.ledger.resources().active_children, 0);
        drop(root_context);
        let first = fixture.spawn("active");
        let second = fixture.spawn("pending");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        let waiting = fixture
            .supervisor
            .wait(&[first.agent_id().into()], WaitMode::All, 1)
            .await
            .unwrap();
        assert!(waiting.settled.is_empty());
        assert_eq!(waiting.remaining.len(), 1);
        assert!(
            !fixture
                .supervisor
                .child(first.agent_id())
                .unwrap()
                .cancel
                .is_cancelled()
        );
        fixture.updates.close();
        let result = fixture
            .supervisor
            .wait(
                &[first.agent_id().into(), second.agent_id().into()],
                WaitMode::All,
                5000,
            )
            .await
            .unwrap();
        assert!(result.remaining.is_empty());
        assert!(
            result
                .settled
                .iter()
                .all(|s| s.outcome == Some(RunOutcome::Cancelled))
        );
        fixture.supervisor.close().await.unwrap();
        root_owner::assert_closed(&mut stream).await;
        assert!(!fixture.root_cancel.is_cancelled());
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_eq!(fixture.ledger.resources().pending_children, 0);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn root_deadline_settles_active_and_queued_children_without_resetting_the_clock() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture =
            Fixture::with_duration(&format!("http://{}", listener.local_addr().unwrap()), 1000);
        let first = fixture.spawn("active before deadline");
        let second = fixture.spawn("waiting for deadline");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        tokio::time::sleep_until(fixture.ledger.deadline() + Duration::from_millis(25)).await;
        // Observe manager completion without introducing an extra cancellation.
        while !fixture
            .supervisor
            .join
            .lock()
            .await
            .task
            .as_ref()
            .unwrap()
            .is_finished()
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let first = fixture.supervisor.stop(first.agent_id()).await.unwrap();
        let second = fixture.supervisor.stop(second.agent_id()).await.unwrap();
        assert_eq!(first.outcome, Some(RunOutcome::TimedOut));
        assert_eq!(second.outcome, Some(RunOutcome::TimedOut));
        fixture.supervisor.close().await.unwrap();
        root_owner::assert_closed(&mut stream).await;
        assert_eq!(fixture.ledger.total().model_calls, 1);
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert!(!fixture.root_cancel.is_cancelled());
    })
    .await
    .unwrap();
}

mod root_owner;

#[tokio::test]
async fn supervised_native_stream_keeps_lifecycle_deltas_identity_and_causal_parent() {
    use opentelemetry::trace::{TraceContextExt, Tracer};
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let parent = opentelemetry::Context::current_with_span(pablo_core::telemetry::tracer(&sdk).start("execute_tool subagent"));
        let parent_trace = parent.span().span_context().trace_id().to_string();
        let parent_span = parent.span().span_context().span_id().to_string();
        let request_spec = serde_json::from_value(json!({"input":"native selected task","capabilities":{"model_route":["secondary"],"tools":[]}})).unwrap();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = pablo_core::events::tree::TreeSink::new({
            let delivered = delivered.clone();
            move |event: &RunEvent| { delivered.lock().unwrap().push(event.clone()); Ok(()) }
        }, &fixture.ledger, fixture.supervisor.inner.root.clone()).unwrap();
        let child = fixture.supervisor.spawn(&request_spec, parent.clone()).unwrap();
        let drain = tokio::spawn(Supervisor::consume_updates(fixture.updates.clone(), sink, fixture.root_cancel.clone()));
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = request(&mut stream).await;
        assert!(body.to_string().contains("native selected task"));
        assert!(!body.to_string().contains("parent private transcript"));
        let mut body = String::new();
        for text in ["first", "second", "third"] {
            body.push_str(&format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})));
        }
        body.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n");
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        let wait = fixture.supervisor.wait(&[child.agent_id().into()], WaitMode::All, 5000).await.unwrap();
        assert!(wait.remaining.is_empty());
        assert!(wait.settled[0].outcome.as_ref().unwrap().is_completed());
        fixture.supervisor.close().await.unwrap();
        drain.await.unwrap().unwrap();
        let events = delivered.lock().unwrap().clone();
        assert_eq!(events.len(), 7);
        assert!(matches!(events[0].kind, EventKind::RunStarted));
        assert!(matches!(events[1].kind, EventKind::ModelStarted { .. }));
        assert!(matches!(events[5].kind, EventKind::ModelFinished { .. }));
        assert!(matches!(events[6].kind, EventKind::RunFinished { .. }));
        for (event, text) in events[2..5].iter().zip(["first", "second", "third"]) {
            assert!(matches!(&event.kind, EventKind::TextDelta { text: actual } if actual == text));
        }
        let snapshot = &wait.settled[0];
        assert!(snapshot.error.is_none());
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.seq, index as u64 + 1);
            assert_eq!(event.root_seq, Some(index as u64 + 1));
            assert_eq!(event.trace_id, parent_trace);
            assert_eq!(event.run_id, snapshot.trace.as_ref().unwrap().run_id);
            let agent = event.agent.as_ref().unwrap();
            assert_eq!(agent.agent_id(), child.agent_id());
            assert_eq!(agent.root_run_id(), fixture.supervisor.inner.root.root_run_id());
            assert_eq!(agent.session_id(), event.session_id);
            assert_eq!(agent.parent_agent_id(), Some(fixture.supervisor.inner.root.agent_id()));
            if index < 6 { assert!(event.accounting.is_none()); }
        }
        assert_eq!(events[6].accounting.as_deref(), Some(&snapshot.accounting));
        assert!(events[1].model_route.is_some());
        assert!(events[5].model_route.is_some());
        assert_eq!(events[0].parent_span_id.as_deref(), Some(parent_span.as_str()));
        assert_eq!(events[6].span_id, events[0].span_id);
        assert_eq!(events[1].parent_span_id.as_deref(), Some(events[0].span_id.as_str()));
        assert_eq!(events[5].span_id, events[1].span_id);
        assert!(events.windows(2).all(|pair| pair[0].timestamp_unix_micros <= pair[1].timestamp_unix_micros));
        assert_eq!(fixture.ledger.event_counts().used, events.len() as u64);
        fixture.supervisor.close().await.unwrap();
        parent.span().end();
        sdk.shutdown().unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn root_consumer_failure_cancels_and_joins_owned_children() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let sink = pablo_core::events::tree::TreeSink::new(
            |_: &RunEvent| {
                Err(pablo_core::SinkError::Io(std::io::Error::other(
                    "fixture sink failure",
                )))
            },
            &fixture.ledger,
            fixture.supervisor.inner.root.clone(),
        )
        .unwrap();
        let child = fixture.spawn("cancel when observer fails");
        assert!(
            Supervisor::consume_updates(fixture.updates.clone(), sink, fixture.root_cancel.clone())
                .await
                .is_err()
        );
        assert!(fixture.root_cancel.is_cancelled());
        fixture.supervisor.close().await.unwrap();
        assert_eq!(
            fixture
                .supervisor
                .inspect(child.agent_id())
                .unwrap()
                .agent
                .state(),
            AgentState::Settled
        );
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_eq!(fixture.ledger.resources().pending_children, 0);
    })
    .await
    .unwrap();
}
