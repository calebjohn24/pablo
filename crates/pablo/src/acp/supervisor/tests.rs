use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    cwd: std::path::PathBuf,
    supervisor: Supervisor,
    updates: async_channel::Receiver<Update>,
    root_cancel: CancellationToken,
    ledger: RootLedger,
}
impl Fixture {
    fn new(endpoint: &str) -> Self {
        Self::with_duration(endpoint, 900_000)
    }
    fn with_duration(endpoint: &str, duration: u64) -> Self {
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
            ),
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
        let root = AgentRef::root("root-run".into(), "root-session".into());
        let ledger = RootLedger::new(&root, parent.spec().limits.clone()).unwrap();
        let root_cancel = CancellationToken::new();
        let (supervisor, updates) =
            Supervisor::new(options, root, ledger.clone(), &root_cancel, parent, tools).unwrap();
        Self {
            cwd,
            supervisor,
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
        let mut byte = [0; 1];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
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
        let mut byte = [0; 1];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
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
        let mut byte = [0; 1];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
        assert_eq!(fixture.ledger.total().model_calls, 1);
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert!(!fixture.root_cancel.is_cancelled());
    })
    .await
    .unwrap();
}
