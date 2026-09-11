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
    fn admission(
        endpoint: &str,
        duration: u64,
        extra: &str,
    ) -> (std::path::PathBuf, Options, PreparedRun) {
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
        (cwd, options, parent)
    }
    fn with_settings(endpoint: &str, duration: u64, extra: &str) -> Self {
        let (cwd, options, parent) = Self::admission(endpoint, duration, extra);
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
async fn overlapping_children_fifo_queue_independent_stop_wait_and_joined_close() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let first = fixture.spawn("first selected task");
        let second = fixture.spawn("second selected task");
        let removed = fixture.spawn("removed queued task");
        let fourth = fixture.spawn("fourth selected task");
        assert_eq!(first.state(), AgentState::Queued);
        assert_eq!(fixture.ledger.total().model_calls, 0);
        assert!(
            fixture
                .supervisor
                .inspect(&uuid::Uuid::new_v4().to_string())
                .is_err()
        );
        let drain = tokio::spawn({
            let updates = fixture.updates.clone();
            async move {
                let mut agents = vec![];
                while let Ok(update) = updates.recv().await {
                    agents.push(update.agent.agent_id().to_owned());
                    let _ = update.update.consumed.send(());
                }
                agents
            }
        });
        // Neither request receives completion until both independent model
        // operations have reached the provider: a concurrency barrier.
        let (mut a, _) = listener.accept().await.unwrap();
        let a_body = request(&mut a).await.to_string();
        let (mut b, _) = listener.accept().await.unwrap();
        let b_body = request(&mut b).await.to_string();
        assert!(!a_body.contains("parent private transcript"));
        assert!(!b_body.contains("parent private transcript"));
        let (mut first_stream, mut second_stream) = if a_body.contains("first selected task") {
            assert!(b_body.contains("second selected task"));
            (a, b)
        } else {
            assert!(a_body.contains("second selected task"));
            assert!(b_body.contains("first selected task"));
            (b, a)
        };
        held(&mut first_stream).await;
        held(&mut second_stream).await;
        assert_eq!(fixture.ledger.resources().active_children, 2);
        assert_eq!(fixture.ledger.resources().pending_children, 2);
        let stopped = fixture.supervisor.stop(removed.agent_id()).await.unwrap();
        assert_eq!(stopped.outcome, Some(RunOutcome::Cancelled));
        assert_eq!(fixture.ledger.resources().pending_children, 1);
        let stopped = fixture.supervisor.stop(first.agent_id()).await.unwrap();
        assert_eq!(stopped.outcome, Some(RunOutcome::Cancelled));
        root_owner::assert_closed(&mut first_stream).await;
        assert_ne!(
            fixture
                .supervisor
                .inspect(second.agent_id())
                .unwrap()
                .agent
                .state(),
            AgentState::Settled
        );
        assert!(!fixture.root_cancel.is_cancelled());
        let (mut fourth_stream, _) = listener.accept().await.unwrap();
        assert!(
            request(&mut fourth_stream)
                .await
                .to_string()
                .contains("fourth selected task")
        );
        done(&mut fourth_stream).await;
        let selected = vec![second.agent_id().to_owned(), fourth.agent_id().to_owned()];
        let any = fixture
            .supervisor
            .wait(&selected, WaitMode::Any, 5000)
            .await
            .unwrap();
        assert_eq!(any.settled.len(), 1);
        assert_eq!(any.settled[0].agent.agent_id(), fourth.agent_id());
        assert_eq!(any.remaining[0].agent_id(), second.agent_id());
        done(&mut second_stream).await;
        let all = fixture
            .supervisor
            .wait(&selected, WaitMode::All, 5000)
            .await
            .unwrap();
        assert!(all.remaining.is_empty());
        assert_eq!(all.settled[0].agent.agent_id(), second.agent_id());
        assert!(
            all.settled
                .iter()
                .all(|s| s.outcome.as_ref().unwrap().is_completed())
        );
        assert_eq!(
            fixture
                .supervisor
                .stop(second.agent_id())
                .await
                .unwrap()
                .outcome,
            all.settled[0].outcome
        );
        assert_eq!(fixture.ledger.total().model_calls, 3);
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_eq!(fixture.ledger.resources().context_bytes, 0);
        assert_eq!(
            fixture.ledger.resources().result_bytes,
            4 * MAX_RESULT_BYTES
        );
        let (a, b) = tokio::join!(fixture.supervisor.close(), fixture.supervisor.close());
        a.unwrap();
        b.unwrap();
        let delivered = drain.await.unwrap();
        for child in [&first, &second, &fourth] {
            assert!(delivered.iter().any(|id| id == child.agent_id()));
        }
        assert!(!delivered.iter().any(|id| id == removed.agent_id()));
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
        let queued = fixture.spawn("third stays queued");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        let (mut other, _) = listener.accept().await.unwrap();
        request(&mut other).await;
        held(&mut other).await;
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
        root_owner::assert_closed(&mut other).await;
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
        assert_eq!(
            fixture
                .supervisor
                .inspect(queued.agent_id())
                .unwrap()
                .outcome,
            Some(RunOutcome::Cancelled)
        );
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_eq!(fixture.ledger.resources().pending_children, 0);
        assert_eq!(fixture.ledger.total().model_calls, 2);
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
        let queued = fixture.spawn("third stays queued");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        let (mut other, _) = listener.accept().await.unwrap();
        request(&mut other).await;
        held(&mut other).await;
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
        root_owner::assert_closed(&mut other).await;
        assert_eq!(fixture.ledger.total().model_calls, 2);
        assert_eq!(
            fixture
                .supervisor
                .inspect(queued.agent_id())
                .unwrap()
                .outcome,
            Some(RunOutcome::TimedOut)
        );
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert!(!fixture.root_cancel.is_cancelled());
    })
    .await
    .unwrap();
}

#[path = "tests/root_owner.rs"]
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

#[tokio::test]
async fn healthy_consumer_receives_stop_and_deadline_closings_before_settlement() {
    for deadline in [false, true] {
        tokio::time::timeout(Duration::from_secs(10), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let fixture = Fixture::with_duration(&format!("http://{}", listener.local_addr().unwrap()), if deadline { 1000 } else { 900_000 });
            let delivered = Arc::new(Mutex::new(Vec::new()));
            let sink = pablo_core::events::tree::TreeSink::new({
                let delivered = delivered.clone();
                move |event: &RunEvent| { delivered.lock().unwrap().push(event.clone()); Ok(()) }
            }, &fixture.ledger, fixture.supervisor.inner.root.clone()).unwrap();
            let drain = tokio::spawn(Supervisor::consume_updates(fixture.updates.clone(), sink, fixture.root_cancel.clone()));
            let child = fixture.spawn("cancel through native consumer");
            let (mut stream, _) = listener.accept().await.unwrap();
            request(&mut stream).await;
            held(&mut stream).await;
            if deadline {
                tokio::time::sleep_until(fixture.ledger.deadline()).await;
                while !fixture.supervisor.join.lock().await.task.as_ref().unwrap().is_finished() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            } else {
                let stopped = fixture.supervisor.stop(child.agent_id()).await.unwrap();
                assert_eq!(stopped.outcome, Some(RunOutcome::Cancelled));
            }
            let expected = if deadline { RunOutcome::TimedOut } else { RunOutcome::Cancelled };
            let snapshot = fixture.supervisor.inspect(child.agent_id()).unwrap();
            assert_eq!(snapshot.agent.state(), AgentState::Settled);
            assert_eq!(snapshot.outcome, Some(expected.clone()));
            assert!(snapshot.error.is_none());
            let events = delivered.lock().unwrap().clone();
            assert!(matches!(events.first().unwrap().kind, EventKind::RunStarted));
            assert!(matches!(events[events.len()-2].kind, EventKind::ModelFinished { .. }));
            assert!(matches!(&events.last().unwrap().kind, EventKind::RunFinished { outcome } if *outcome == expected));
            assert_eq!(events.iter().filter(|event|matches!(event.kind, EventKind::RunFinished { .. })).count(), 1);
            for (index, event) in events.iter().enumerate() { assert_eq!(event.seq, index as u64 + 1); assert_eq!(event.root_seq, Some(index as u64 + 1)); }
            assert_eq!(fixture.ledger.event_counts().used, events.len() as u64);
            assert_eq!(fixture.ledger.resources().active_children, 0);
            root_owner::assert_closed(&mut stream).await;
            fixture.supervisor.close().await.unwrap();
            drain.await.unwrap().unwrap();
        }).await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_drains_owned_work_even_when_the_root_update_queue_is_full() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        // Each stopped run leaves its unacknowledged first update in the bounded
        // root queue. The ninth real child reaches a full queue, with no consumer.
        for index in 0..=QUEUE_EVENTS {
            let child = fixture.spawn("stalled native consumer");
            let (mut stream, _) = listener.accept().await.unwrap();
            request(&mut stream).await;
            let snapshot = tokio::time::timeout(
                Duration::from_secs(2),
                fixture.supervisor.stop(child.agent_id()),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(snapshot.agent.state(), AgentState::Settled);
            assert_eq!(snapshot.outcome, Some(RunOutcome::Cancelled));
            assert_eq!(fixture.ledger.resources().active_children, 0);
            assert_eq!(fixture.ledger.resources().pending_children, 0);
            assert_eq!(fixture.updates.len(), (index + 1).min(QUEUE_EVENTS));
            root_owner::assert_closed(&mut stream).await;
        }
        fixture.supervisor.close().await.unwrap();
    })
    .await
    .unwrap();
}

mod root_factory {
    use super::super::factory::RootFactory;
    use super::*;

    fn prepared(mode: &str) -> (std::path::PathBuf, RootFactory) {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let extra = format!(
            "\n[options.mcp.servers.local]\ntransport=\"stdio\"\ncommand={}\nargs=[{},{}]\nrequired=true\n",
            json!(root.join(".pablo/mcp-fixture-venv/bin/python")),
            json!(root.join("tests/fixtures/mcp/adversarial.py")),
            json!(mode)
        );
        let (cwd, options, parent) = Fixture::admission("http://127.0.0.1:1", 10_000, &extra);
        std::fs::write(cwd.join("catalog-version"), "original").unwrap();
        (cwd, RootFactory::new(Arc::new(options), parent).unwrap())
    }
    fn assert_reaped(cwd: &std::path::Path) {
        let pid: i32 = std::fs::read_to_string(cwd.join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "MCP process must be joined before capacity is released"
        );
    }

    #[tokio::test]
    async fn capacity_rejection_precedes_process_start() {
        let (cwd, factory) = prepared("child_catalog");
        let ledger = factory.ledger.clone();
        let occupied = ledger
            .reserve_resources(
                factory.root.agent_id(),
                Resources {
                    processes: 16,
                    mcp_sessions: 16,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            factory.open(&CancellationToken::new()).await.err().unwrap(),
            "root MCP admission rejected"
        );
        assert!(!cwd.join("pid").exists());
        assert_eq!(ledger.resources().processes, 16);
        drop(occupied);
        assert_eq!(ledger.resources(), Resources::default());
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the isolated .pablo/mcp-fixture-venv/bin/python fixture interpreter"]
    async fn root_mcp_capacity_survives_setup_and_owned_close() {
        let (cwd, factory) = prepared("child_catalog");
        let ledger = factory.ledger.clone();
        let (owner, updates) = factory.open(&CancellationToken::new()).await.unwrap();
        assert_eq!(ledger.resources().processes, 1);
        assert_eq!(ledger.resources().mcp_sessions, 1);
        assert!(
            owner
                .inner
                .parent_tools
                .descriptors()
                .iter()
                .any(|t| t.name == "mcp/local/read")
        );
        let mut abandoned_close = Box::pin(owner.close());
        assert!(futures::poll!(abandoned_close.as_mut()).is_pending());
        assert_eq!(ledger.resources().processes, 1);
        drop(abandoned_close);
        owner.close().await.unwrap();
        owner.close().await.unwrap();
        assert!(updates.is_closed());
        assert_reaped(&cwd);
        assert_eq!(ledger.resources(), Resources::default());
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the isolated .pablo/mcp-fixture-venv/bin/python fixture interpreter"]
    async fn cancelled_initialization_joins_before_releasing_capacity() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (cwd, factory) = prepared("startup_hang");
            let ledger = factory.ledger.clone();
            let cancel = CancellationToken::new();
            let setup = tokio::spawn({
                let cancel = cancel.clone();
                async move { factory.open(&cancel).await }
            });
            while !cwd.join("pid").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(ledger.resources().processes, 1);
            assert_eq!(ledger.resources().mcp_sessions, 1);
            cancel.cancel();
            assert!(setup.await.unwrap().is_err());
            assert_reaped(&cwd);
            assert_eq!(ledger.resources(), Resources::default());
            std::fs::remove_dir_all(cwd).unwrap();
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn two_active_and_fourteen_queued_children_are_bounded_and_joined_on_root_cancel() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let mut children = vec![fixture.spawn("active one"), fixture.spawn("active two")];
        let (mut first, _) = listener.accept().await.unwrap();
        request(&mut first).await;
        held(&mut first).await;
        let (mut second, _) = listener.accept().await.unwrap();
        request(&mut second).await;
        held(&mut second).await;
        assert_eq!(fixture.ledger.resources().active_children, 2);
        for _ in 0..14 {
            children.push(fixture.spawn("bounded queued work"));
        }
        let before = fixture.ledger.resources();
        assert_eq!(before.pending_children, 14);
        assert_eq!(before.result_bytes, 16 * MAX_RESULT_BYTES);
        let excess =
            serde_json::from_value(json!({"input":"excess","capabilities":{"tools":[]}})).unwrap();
        assert!(
            fixture
                .supervisor
                .spawn(&excess, opentelemetry::Context::new())
                .is_err()
        );
        assert_eq!(fixture.ledger.resources(), before);
        fixture.root_cancel.cancel();
        fixture.supervisor.close().await.unwrap();
        root_owner::assert_closed(&mut first).await;
        root_owner::assert_closed(&mut second).await;
        for child in children {
            let snapshot = fixture.supervisor.inspect(child.agent_id()).unwrap();
            assert_eq!(snapshot.agent.state(), AgentState::Settled);
            assert_eq!(snapshot.outcome, Some(RunOutcome::Cancelled));
        }
        let resources = fixture.ledger.resources();
        assert_eq!(resources.active_children, 0);
        assert_eq!(resources.pending_children, 0);
        assert_eq!(resources.context_bytes, 0);
        assert_eq!(fixture.ledger.total().model_calls, 2);
    })
    .await
    .unwrap();
}

async fn structured_done(stream: &mut tokio::net::TcpStream, value: Value) {
    let body = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"index":0,"delta":{"content":value.to_string()},"finish_reason":"stop"}]})
    );
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn handoff_fan_in_resolves_valid_owned_results_and_rejects_stale_before_provider() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        std::fs::write(fixture.cwd.join("result.json"), b"{}").unwrap();
        let drain = tokio::spawn({
            let updates = fixture.updates.clone();
            async move {
                while let Ok(update) = updates.recv().await {
                    let _ = update.update.consumed.send(());
                }
            }
        });
        let mut ids = vec![];
        for task in ["source inline PRIVATE_A", "source artifact PRIVATE_B"] {
            let request = serde_json::from_value(json!({"input":task,"capabilities":{"model_route":["secondary"],"tools":[]},"output_schema":{"type":"object"}})).unwrap();
            ids.push(fixture.supervisor.spawn(&request, opentelemetry::Context::new()).unwrap().agent_id().to_owned());
        }
        // Both fan-out requests must arrive before either receives a result.
        let (mut a, _) = listener.accept().await.unwrap();
        let a_body = request(&mut a).await;
        let (mut b, _) = listener.accept().await.unwrap();
        let b_body = request(&mut b).await;
        let reference = json!({"path":"result.json","revision":"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"});
        for (stream, body) in [(&mut a, a_body), (&mut b, b_body)] {
            structured_done(stream, if body.to_string().contains("PRIVATE_A") { json!({"answer":42}) } else { reference.clone() }).await;
        }
        let settled = fixture.supervisor.wait(&ids, WaitMode::All, 5000).await.unwrap();
        assert!(settled.remaining.is_empty());
        let selections: Vec<Value> = settled.settled.iter().enumerate().map(|(i, source)| {
            assert_eq!(source.validation.as_ref().unwrap().status, "valid");
            assert!(source.trace.is_some());
            assert_eq!(source.accounting.model_calls, 1);
            json!({"source_agent_id":source.agent.agent_id(),"result_id":source.result_id,"kind":if i==0 {"inline"} else {"artifact"}})
        }).collect();
        let selected: pablo_core::children::SpawnRequest = serde_json::from_value(json!({"input":"combine selected outputs","capabilities":{"model_route":["secondary"],"tools":["fs.read"]},"handoffs":selections})).unwrap();
        let before = fixture.ledger.resources();
        let mut forged = selected.clone();
        forged.handoffs[0].result_id = uuid::Uuid::new_v4().to_string();
        assert!(fixture.supervisor.spawn(&forged, opentelemetry::Context::new()).is_err());
        forged = selected.clone();
        forged.handoffs[0].source_agent_id = uuid::Uuid::new_v4().to_string();
        assert!(fixture.supervisor.spawn(&forged, opentelemetry::Context::new()).is_err());
        assert_eq!(fixture.ledger.resources(), before);
        let joined = fixture.supervisor.spawn(&selected, opentelemetry::Context::new()).unwrap();
        let (mut downstream, _) = listener.accept().await.unwrap();
        let body = request(&mut downstream).await.to_string();
        assert!(!body.contains("PRIVATE_A") && !body.contains("PRIVATE_B") && !body.contains("parent private transcript"));
        for source in &settled.settled {
            assert!(body.contains(source.result_id.as_ref().unwrap()));
            assert!(body.contains(&source.trace.as_ref().unwrap().trace_id));
            assert!(body.contains(&source.validation.as_ref().unwrap().schema_sha256));
        }
        assert!(body.contains("answer") && body.contains("42") && body.contains("result.json"));
        done(&mut downstream).await;
        let result = fixture.supervisor.wait(&[joined.agent_id().to_owned()], WaitMode::All, 5000).await.unwrap();
        assert!(result.settled[0].outcome.as_ref().unwrap().is_completed());
        assert_eq!(result.settled[0].handoffs.len(), 2);
        let mut oversized = selected.clone();
        oversized.input = "x".repeat(pablo_core::children::MAX_INPUT_BYTES);
        assert!(fixture.supervisor.spawn(&oversized, opentelemetry::Context::new()).is_err());
        let mut denied = selected.clone();
        denied.capabilities.tools = Some(vec![]);
        let denied = fixture.supervisor.spawn(&denied, opentelemetry::Context::new()).unwrap();
        let rejected = fixture.supervisor.wait(&[denied.agent_id().to_owned()], WaitMode::All, 5000).await.unwrap();
        assert_eq!(rejected.settled[0].outcome, Some(admission_failed()));
        assert_eq!(rejected.settled[0].accounting.model_calls, 0);
        let blocker_a = fixture.spawn("blocker A");
        let blocker_b = fixture.spawn("blocker B");
        let (mut block_a, _) = listener.accept().await.unwrap();
        request(&mut block_a).await;
        let (mut block_b, _) = listener.accept().await.unwrap();
        request(&mut block_b).await;
        held(&mut block_a).await;
        held(&mut block_b).await;
        let stale = fixture.supervisor.spawn(&selected, opentelemetry::Context::new()).unwrap();
        assert_eq!(fixture.ledger.resources().pending_children, 1);
        std::fs::write(fixture.cwd.join("result.json"), b"stale").unwrap();
        fixture.supervisor.stop(blocker_a.agent_id()).await.unwrap();
        let rejected = fixture.supervisor.wait(&[stale.agent_id().to_owned()], WaitMode::All, 5000).await.unwrap();
        assert_eq!(rejected.settled[0].outcome, Some(admission_failed()));
        assert_eq!(rejected.settled[0].accounting.model_calls, 0);
        assert!(rejected.settled[0].result_id.is_none());
        assert_eq!(fixture.ledger.total().model_calls, 5);
        fixture.supervisor.stop(blocker_b.agent_id()).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(30), listener.accept()).await.is_err());
        fixture.supervisor.close().await.unwrap();
        drain.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn handoff_rejects_invalid_oversize_cancelled_and_unvalidated_sources() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let drain = tokio::spawn({
            let updates = fixture.updates.clone();
            async move {
                while let Ok(update) = updates.recv().await {
                    let _ = update.update.consumed.send(());
                }
            }
        });
        for mode in ["invalid", "oversize", "cancelled", "unvalidated"] {
            let mut value = json!({"input":mode,"capabilities":{"model_route":["secondary"],"tools":[]}});
            if mode != "unvalidated" { value["output_schema"] = json!({"type":"object"}); }
            let source = fixture.supervisor.spawn(&serde_json::from_value(value).unwrap(), opentelemetry::Context::new()).unwrap();
            let (mut stream, _) = listener.accept().await.unwrap();
            request(&mut stream).await;
            match mode {
                "cancelled" => { held(&mut stream).await; fixture.supervisor.stop(source.agent_id()).await.unwrap(); }
                "oversize" => structured_done(&mut stream, json!({"large":"x".repeat(70_000)})).await,
                "invalid" => structured_done(&mut stream, json!(42)).await,
                _ => structured_done(&mut stream, json!({"valid_json_but_no_schema":true})).await,
            }
            let result = fixture.supervisor.wait(&[source.agent_id().to_owned()], WaitMode::All, 5000).await.unwrap();
            assert!(result.remaining.is_empty());
            let snapshot = &result.settled[0];
            assert_eq!(snapshot.outcome.as_ref().unwrap().is_completed(), mode == "unvalidated");
            let selection = serde_json::from_value(json!({"input":"must not run","handoffs":[{"source_agent_id":source.agent_id(),"result_id":snapshot.result_id.clone().unwrap_or_else(||uuid::Uuid::new_v4().to_string()),"kind":"inline"}]})).unwrap();
            let before = fixture.ledger.resources();
            assert!(fixture.supervisor.spawn(&selection, opentelemetry::Context::new()).is_err());
            assert_eq!(fixture.ledger.resources(), before);
        }
        assert_eq!(fixture.ledger.total().model_calls, 4);
        assert!(tokio::time::timeout(Duration::from_millis(30), listener.accept()).await.is_err());
        fixture.supervisor.close().await.unwrap();
        drain.await.unwrap();
    }).await.unwrap();
}

#[path = "tests/extensibility.rs"]
mod extensibility;

#[path = "tests/remote.rs"]
mod remote;
