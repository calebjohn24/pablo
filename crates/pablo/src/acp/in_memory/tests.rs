use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn cancelled_close_caller_does_not_lose_the_owned_worker_join() {
    use pablo_core::children::{AgentRef, SpawnRequest, ledger::RootLedger};
    use std::{future::Future, task::Poll};
    let cwd = std::env::temp_dir().join(format!("pablo-close-join-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let entry = cwd.join("entry.toml");
    std::fs::write(
        &entry,
        include_str!("../../../../../docs/project/fixtures/c3-model-routes/three-providers.toml")
            .to_owned()
            + "\n[options.mcp.servers.lease]\ntransport=\"stdio\"\ncommand=\"/bin/false\"\n",
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
        ]
        .into_iter(),
    )
    .unwrap();
    let parent = options
        .prepare_run(
            Some("parent".into()),
            Some(cwd.clone()),
            Some("session".into()),
        )
        .unwrap()
        .unwrap();
    let request: SpawnRequest =
        serde_json::from_value(json!({"input":"held child","capabilities":{"tools":[]}})).unwrap();
    let prepared = parent
        .prepare_child(&request, &pablo_core::ToolRegistry::default())
        .unwrap();
    let root = AgentRef::root("root".into(), "session".into());
    let ledger = RootLedger::new(&root, parent.spec().limits.clone()).unwrap();
    let child = root.temporary_child().unwrap();
    let options = Arc::new(options);
    let required = Resources {
        active_children: 1,
        context_bytes: prepared.spec().limits.max_context_bytes,
        ..prepared.mcp_resources().unwrap()
    };
    assert_eq!(required.processes, 1);
    assert_eq!(required.mcp_sessions, 1);
    let mut lease = Arc::new(
        ledger
            .admit_child(
                &child,
                prepared.spec().limits.clone(),
                Resources {
                    active_children: 1,
                    context_bytes: required.context_bytes,
                    ..Default::default()
                },
            )
            .unwrap(),
    );
    assert!(
        Dispatcher::bind_child(
            options.clone(),
            prepared.clone(),
            ledger.clone(),
            &child,
            CancellationToken::new(),
            opentelemetry::Context::new(),
            lease.clone()
        )
        .is_err()
    );
    Arc::get_mut(&mut lease).unwrap().replace(required).unwrap();
    let (dispatcher, _updates) = Dispatcher::bind_child(
        options,
        prepared,
        ledger.clone(),
        &child,
        CancellationToken::new(),
        opentelemetry::Context::new(),
        lease,
    )
    .unwrap();
    let lease = dispatcher
        .state
        .lock()
        .unwrap()
        .admitted_child
        .as_ref()
        .unwrap()
        .lease
        .as_ref()
        .unwrap()
        .clone();
    let (release, held) = std::sync::mpsc::channel();
    dispatcher.state.lock().unwrap().worker = Some(Worker::held_for_test(lease, held));
    let mut first = Box::pin(dispatcher.close());
    futures::future::poll_fn(|cx| {
        assert!(first.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(first);
    assert!(dispatcher.closing.try_lock().unwrap().task.is_some());
    let mut second = Box::pin(dispatcher.close());
    futures::future::poll_fn(|cx| {
        assert!(second.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(ledger.resources().active_children, 1);
    assert_eq!(ledger.resources().processes, 1);
    assert_eq!(ledger.resources().mcp_sessions, 1);
    drop(second);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !dispatcher
            .closing
            .lock()
            .await
            .task
            .as_ref()
            .unwrap()
            .is_finished()
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        ledger.resources().active_children,
        1,
        "dispatcher retains capacity until close acknowledges the join"
    );
    dispatcher.close().await.unwrap();
    assert_eq!(ledger.resources(), Resources::default());
    dispatcher.close().await.unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn admitted_child_uses_fixed_route_root_accounting_and_joined_root_cancellation() {
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
    };
    use pablo_core::{
        Usage,
        children::{
            AgentRef, SpawnRequest,
            ledger::{RootLedger, resources::Resources},
        },
        provider::AccountingBounds,
    };
    let cwd = std::env::temp_dir().join(format!("pablo-bound-acp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let entry = cwd.join("entry.toml");
    std::fs::write(
        &entry,
        include_str!("../../../../../docs/project/fixtures/c3-model-routes/three-providers.toml"),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stream.read(&mut buffer).await.unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&buffer[..n]);
            assert!(request.len() < 1024 * 1024);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let length: usize = String::from_utf8_lossy(&request[..end])
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|n| n.parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    let body: Value =
                        serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
                    assert_eq!(body["model"], "zai/glm-5.3-flash");
                    assert!(body.to_string().contains("selected child input"));
                    assert!(!body.to_string().contains("parent private input"));
                    assert!(body["tools"].as_array().is_none_or(Vec::is_empty));
                    break;
                }
            }
        }
        let delta = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"child is running\"},\"finish_reason\":null}]}\n\n";
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n{delta}").as_bytes()).await.unwrap();
        assert_eq!(stream.read(&mut buffer).await.unwrap(), 0);
    });
    let options = Options::parse(
        "acp".into(),
        [
            "--stdio".into(),
            "--config".into(),
            entry.clone().into_os_string(),
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
            Some("parent private input".into()),
            Some(cwd.clone()),
            Some("parent-session".into()),
        )
        .unwrap()
        .unwrap();
    let request: SpawnRequest = serde_json::from_value(json!({"input":"selected child input","capabilities":{"model_route":["secondary"],"tools":[]}})).unwrap();
    let prepared = parent
        .prepare_child(&request, &parent.tools().unwrap())
        .unwrap();
    let root = AgentRef::root(uuid::Uuid::new_v4().to_string(), "parent-session".into());
    let child = root.temporary_child().unwrap();
    let ledger = RootLedger::new(&root, parent.spec().limits.clone()).unwrap();
    ledger
        .reserve_model(root.agent_id(), AccountingBounds::default())
        .unwrap()
        .settle(&Usage::default(), None, true)
        .unwrap();
    let root_cancel = CancellationToken::new();
    let trace_id = TraceId::from_hex("11111111111111111111111111111111").unwrap();
    let parent_context = opentelemetry::Context::new().with_remote_span_context(SpanContext::new(
        trace_id,
        SpanId::from_hex("2222222222222222").unwrap(),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    ));
    let (dispatcher, updates) = Dispatcher::new_child(
        options,
        prepared,
        ledger.clone(),
        &child,
        root_cancel.clone(),
        parent_context,
    )
    .unwrap();
    assert_eq!(ledger.resources().active_children, 1);
    // Mutable configuration cannot replace an already admitted child projection.
    std::fs::write(&entry, "invalid changed configuration").unwrap();
    dispatcher
        .initialize(
            wire::InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
                .client_capabilities(wire::ClientCapabilities::new().meta(meta(json!(true)))),
        )
        .unwrap();
    assert!(
        dispatcher
            .new_session(wire::NewSessionRequest::new(
                cwd.parent().unwrap().to_path_buf()
            ))
            .is_err()
    );
    let session = dispatcher
        .new_session(wire::NewSessionRequest::new(cwd.clone()))
        .unwrap();
    let request_cancel = CancellationToken::new();
    assert!(
        dispatcher
            .prompt(
                wire::PromptRequest::new(
                    session.session_id.clone(),
                    vec![wire::ContentBlock::Text(wire::TextContent::new(
                        "changed child input"
                    ))]
                ),
                &request_cancel
            )
            .await
            .is_err()
    );
    let mut prompt = wire::PromptRequest::new(
        session.session_id.clone(),
        vec![wire::ContentBlock::Text(wire::TextContent::new(
            "selected child input",
        ))],
    );
    prompt.meta = Some(meta(
        json!({"traceparent":"00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01"}),
    ));
    let prompt = dispatcher.prompt(prompt, &request_cancel);
    tokio::pin!(prompt);
    let mut saw_text = false;
    let response = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                response = &mut prompt => break response.unwrap(),
                update = updates.recv() => {
                    let update = update.unwrap();
                    if let wire::AgentNotification::SessionNotification(notification) = update.notification {
                        assert_eq!(notification.session_id, session.session_id);
                        assert_eq!(notification.meta.as_ref().unwrap()[EXTENSION]["trace_id"], trace_id.to_string());
                        let identity = &notification.meta.as_ref().unwrap()[EXTENSION]["agent"];
                        assert_eq!(identity["agent_id"], child.agent_id());
                        assert_eq!(identity["parent_agent_id"], root.agent_id());
                        assert_eq!(identity["root_run_id"], root.root_run_id());
                        assert_eq!(identity["root_session_id"], root.root_session_id());
                        assert_eq!(identity["session_id"], session.session_id.to_string());

                        if let wire::SessionUpdate::AgentMessageChunk(_) = notification.update {
                            saw_text = true;
                            assert_eq!(ledger.total().model_calls, 2);
                            assert_eq!(ledger.agent(child.agent_id()).unwrap().model_calls, 1);
                            assert!(ledger.reserve_model(root.agent_id(), AccountingBounds::default()).is_err());
                            root_cancel.cancel();
                        }
                    }
                    update.consumed.send(()).unwrap();
                }
            }
        }
    }).await.unwrap();
    assert!(saw_text);
    assert_eq!(response.stop_reason, wire::StopReason::Cancelled);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ledger.resources().active_children,
        1,
        "worker still owns setup until close"
    );
    let occupying = root.temporary_child().unwrap();
    let occupied = ledger
        .admit_child(
            &occupying,
            parent.spec().limits.clone(),
            Resources {
                active_children: 1,
                ..Default::default()
            },
        )
        .unwrap();
    let other = root.temporary_child().unwrap();
    assert!(
        ledger
            .admit_child(
                &other,
                parent.spec().limits.clone(),
                Resources {
                    active_children: 1,
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(ledger.agent(other.agent_id()).is_none());
    let (first, second) = tokio::join!(dispatcher.close(), dispatcher.close());
    first.unwrap();
    second.unwrap();
    assert_eq!(ledger.resources().active_children, 1);
    drop(occupied);
    assert_eq!(ledger.resources().active_children, 0);
    assert!(
        dispatcher
            .new_session(wire::NewSessionRequest::new(cwd.clone()))
            .is_err()
    );
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn official_values_share_fallback_updates_and_joined_session_cancellation() {
    let cwd = std::env::temp_dir().join(format!("pablo-typed-acp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let entry = cwd.join("entry.toml");
    let config =
        include_str!("../../../../../docs/project/fixtures/c3-model-routes/three-providers.toml");
    std::fs::write(&entry, config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for index in 0..6 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let count = stream.read(&mut buf).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buf[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let size: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.parse().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + size {
                        let value: Value =
                            serde_json::from_slice(&request[end + 4..end + 4 + size]).unwrap();
                        assert_eq!(
                            value["model"],
                            if index % 2 == 0 {
                                "z-ai/glm-5.3-flash"
                            } else {
                                "zai/glm-5.3-flash"
                            }
                        );
                        break;
                    }
                }
                assert!(request.len() < 1024 * 1024);
            }
            if index % 2 == 0 {
                stream.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            } else {
                let delta = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"typed evidence\"},\"finish_reason\":null}]}\n\n";
                let tail = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                let body = format!("{delta}{tail}");
                let length = if index == 1 {
                    body.len()
                } else {
                    body.len() + 1000
                };
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                stream
                    .write_all(if index == 1 {
                        body.as_bytes()
                    } else {
                        delta.as_bytes()
                    })
                    .await
                    .unwrap();
                if index >= 3 {
                    // Cancellation must drop the live provider exchange before close returns.
                    assert_eq!(stream.read(&mut buf).await.unwrap(), 0);
                }
            }
        }
    });
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
    let (dispatcher, updates) = Dispatcher::new(options);
    let init = wire::InitializeRequest::new(agent_client_protocol::schema::ProtocolVersion::V1)
        .client_capabilities(wire::ClientCapabilities::new().meta(meta(json!(true))));
    assert_eq!(
        dispatcher
            .initialize(init.clone())
            .unwrap()
            .protocol_version,
        agent_client_protocol::schema::ProtocolVersion::V1
    );
    assert!(dispatcher.initialize(init).is_err());
    for cancel_mode in 0..3 {
        let cancelled = cancel_mode != 0;
        let session = dispatcher
            .new_session(wire::NewSessionRequest::new(cwd.clone()))
            .unwrap();
        assert!(
            dispatcher
                .new_session(wire::NewSessionRequest::new(cwd.clone()))
                .is_err()
        );
        let request = wire::PromptRequest::new(
            session.session_id.clone(),
            vec![wire::ContentBlock::Text(wire::TextContent::new(
                "Read synthetic evidence",
            ))],
        );
        let request_cancel = CancellationToken::new();
        let prompt = dispatcher.prompt(request, &request_cancel);
        tokio::pin!(prompt);
        let mut text = String::new();
        let response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    response = &mut prompt => break response.unwrap(),
                    update = updates.recv() => {
                        let update = update.unwrap();
                        if let wire::AgentNotification::SessionNotification(notification) = update.notification {
                            assert_eq!(notification.session_id, session.session_id);
                            if let wire::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
                                if let wire::ContentBlock::Text(content) = chunk.content { text.push_str(&content.text); }
                                if cancel_mode == 1 { dispatcher.cancel(wire::CancelNotification::new(session.session_id.clone())).unwrap(); } else if cancel_mode == 2 { request_cancel.cancel(); }
                            }
                        }
                        update.consumed.send(()).unwrap();
                    }
                }
            }
        }).await.expect("typed dispatch timed out");
        assert_eq!(text, "typed evidence");
        assert_eq!(
            response.stop_reason,
            if cancelled {
                wire::StopReason::Cancelled
            } else {
                wire::StopReason::EndTurn
            }
        );
        assert!(!dispatcher.state.lock().unwrap().prompt_active);
    }
    dispatcher.close().await.unwrap();
    assert!(
        dispatcher
            .new_session(wire::NewSessionRequest::new(cwd.clone()))
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(dispatcher.state.lock().unwrap().worker.is_none());
    std::fs::remove_dir_all(cwd).unwrap();
}
