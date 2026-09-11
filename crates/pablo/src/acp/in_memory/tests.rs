use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
