use pablo_core::a2a::{AdmissionError, CardAdmission, MAX_CARD_BYTES, TRACE_EXTENSION};
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../tests/fixtures/a2a/card.json")).unwrap()
}
fn host() -> CardAdmission {
    CardAdmission::new("https://agent.example.test/rpc", None, false).unwrap()
}
#[test]
fn sdk_card_admits_only_exact_host_interface_and_preserves_untrusted_metadata() {
    let mut card = fixture();
    card["localPolicy"] = json!({"allow":"all"});
    let bytes = serde_json::to_vec(&card).unwrap();
    let admitted = host().validate(&bytes).unwrap();
    assert_eq!(admitted.endpoint, "https://agent.example.test/rpc");
    assert_eq!(admitted.protocol_version, "1.0");
    assert_eq!(admitted.binding, "JSONRPC");
    assert!(admitted.streaming);
    assert!(!admitted.trace_context);
    assert_eq!(admitted.card_sha256.len(), 64);
    assert!(
        !serde_json::to_string(&admitted)
            .unwrap()
            .contains("localPolicy")
    );
    let first = admitted.card_sha256;
    card["description"] = "Other untrusted metadata".into();
    assert_ne!(
        host()
            .validate(&serde_json::to_vec(&card).unwrap())
            .unwrap()
            .card_sha256,
        first
    );
    for (key, value) in [
        ("url", "https://other.example.test/rpc"),
        ("protocolVersion", "0.3"),
        ("protocolBinding", "GRPC"),
        ("tenant", "foreign"),
    ] {
        let mut card = fixture();
        card["supportedInterfaces"][0][key] = value.into();
        assert_eq!(
            host()
                .validate(&serde_json::to_vec(&card).unwrap())
                .unwrap_err(),
            AdmissionError::UnsupportedInterface
        );
    }
}
#[test]
fn required_extensions_and_security_cannot_choose_host_authority() {
    let mut card = fixture();
    card["capabilities"]["extensions"] = json!([{"uri":TRACE_EXTENSION,"required":true}]);
    let bytes = serde_json::to_vec(&card).unwrap();
    assert_eq!(
        host().validate(&bytes).unwrap_err(),
        AdmissionError::UnsupportedExtension
    );
    let opted = CardAdmission::new("https://agent.example.test/rpc", None, true).unwrap();
    assert!(opted.validate(&bytes).unwrap().trace_context);
    card["capabilities"]["extensions"][0]["uri"] = "urn:unsupported:required".into();
    assert_eq!(
        opted
            .validate(&serde_json::to_vec(&card).unwrap())
            .unwrap_err(),
        AdmissionError::UnsupportedExtension
    );
    card = fixture();
    card["securitySchemes"] = json!({"service":{"httpAuthSecurityScheme":{"scheme":"Bearer"}}});
    card["securityRequirements"] = json!([{"schemes":{"service":{"list":[]}}}]);
    let bytes = serde_json::to_vec(&card).unwrap();
    assert_eq!(
        host().validate(&bytes).unwrap_err(),
        AdmissionError::UnsupportedSecurity
    );
    let bearer = CardAdmission::new(
        "https://agent.example.test/rpc",
        Some("service".into()),
        false,
    )
    .unwrap();
    assert!(bearer.validate(&bytes).is_ok());
    card["securityRequirements"][0]["schemes"]["service"]["list"] = json!(["remote-admin"]);
    assert_eq!(
        bearer
            .validate(&serde_json::to_vec(&card).unwrap())
            .unwrap_err(),
        AdmissionError::UnsupportedSecurity
    );
}
#[test]
fn malformed_ambiguous_and_oversized_cards_fail_closed() {
    assert_eq!(
        host()
            .validate(&vec![b' '; MAX_CARD_BYTES + 1])
            .unwrap_err(),
        AdmissionError::CardBound
    );
    assert_eq!(
        host().validate(b"{}").unwrap_err(),
        AdmissionError::InvalidCard
    );
    let mut card = fixture();
    let duplicate_interface = card["supportedInterfaces"][0].clone();
    card["supportedInterfaces"]
        .as_array_mut()
        .unwrap()
        .push(duplicate_interface);
    assert_eq!(
        host()
            .validate(&serde_json::to_vec(&card).unwrap())
            .unwrap_err(),
        AdmissionError::UnsupportedInterface
    );
    let raw = serde_json::to_string(&fixture()).unwrap();
    let duplicate = raw.replacen("{", "{\"name\":\"injected\",", 1);
    assert_eq!(
        host().validate(duplicate.as_bytes()).unwrap_err(),
        AdmissionError::InvalidCard
    );
    for endpoint in [
        "http://agent.example.test/rpc",
        "https://user:pass@agent.example.test/rpc",
        "https://agent.example.test/rpc?token=secret",
        "https://agent.example.test/rpc#fragment",
    ] {
        assert!(CardAdmission::new(endpoint, None, false).is_err());
    }
}

#[tokio::test]
#[ignore = "requires isolated .pablo/a2a-fixture-venv Python SDK reference server"]
async fn independent_sdk_card_is_fetched_with_version_and_no_task_or_credentials() {
    use pablo_core::{
        CancellationToken,
        children::{AgentKind, AgentRef},
        deployment::{self, ConfigInput, ResolveRequest},
    };
    use tokio::io::{AsyncBufReadExt, BufReader};
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let cwd = std::env::temp_dir().join(format!("pablo-a2a-card-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&cwd).unwrap();
    let mut peer = tokio::process::Command::new(root.join(".pablo/a2a-fixture-venv/bin/python"))
        .arg(root.join("tests/fixtures/a2a/server.py"))
        .current_dir(&cwd)
        .env_clear()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let outcome=tokio::time::timeout(std::time::Duration::from_secs(15),async {
        let mut lines=BufReader::new(peer.stdout.take().unwrap()).lines();
        let ready:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        let mut request=ResolveRequest::new(cwd.canonicalize().unwrap(),"unused");
        request.path_bindings.insert("workspace".into(),cwd.canonicalize().unwrap());
        let tuple=json!({"card_url":"https://agent.example.test/.well-known/agent-card.json","endpoint":"https://agent.example.test/rpc"});
        request.entry=ConfigInput::Document(json!({"schema_version":1,"options":{"a2a":{"remotes":{"echo":tuple}}},"authority":[{"id":"host","a2a_remotes":{"echo":tuple}}],"credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"never-look-up"}]}}}));
        let deployment=deployment::resolve(request).unwrap();
        let parent=AgentRef::root("local-root-run".into(),"local-root-session".into());
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(10);
        let cancel=CancellationToken::new();let url=ready["url"].as_str().unwrap();
        assert!(matches!(deployment.resolve_a2a_fixture(&parent,"missing",url,deadline,&cancel).await,Err(pablo_core::a2a::ResolveError::Configuration(_))));
        let child=parent.temporary_child().unwrap();
        assert!(matches!(deployment.resolve_a2a_fixture(&child,"echo",url,deadline,&cancel).await,Err(pablo_core::a2a::ResolveError::Ownership(_))));
        let mut first=deployment.resolve_a2a_fixture(&parent,"echo",url,deadline,&cancel).await.unwrap();
        let second=deployment.resolve_a2a_fixture(&parent,"echo",url,deadline,&cancel).await.unwrap();
        assert_eq!(first.name(),"echo");assert_eq!(first.agent().kind(),AgentKind::RemoteA2a);
        assert_eq!(first.agent().parent_agent_id(),Some(parent.agent_id()));
        assert_ne!(first.agent().agent_id(),second.agent().agent_id());
        assert_eq!(first.remote(),&pablo_core::a2a::RemoteIdentity::default());
        assert_eq!(first.card().card_sha256,second.card().card_sha256);
        assert_eq!(first.card().protocol_version,"1.0");assert_eq!(first.card().binding,"JSONRPC");
        assert_eq!(first.card().endpoint,"https://agent.example.test/rpc");assert!(first.card().streaming);
        assert!(first.agent().temporary_child().is_err());
        verify_proxy_ledger(&parent,&first);
        let local=serde_json::to_value(first.agent()).unwrap();
        first.observe_remote("remote-context",None).unwrap();
        first.observe_remote("remote-context",Some("remote-task")).unwrap();
        let bound=first.remote().clone();
        assert_eq!(first.observe_remote("changed-context",Some("remote-task")),Err(pablo_core::a2a::IdentityError::Changed));
        assert_eq!(first.observe_remote("remote-context",Some("changed-task")),Err(pablo_core::a2a::IdentityError::Changed));
        assert_eq!(first.observe_remote("remote-context",Some(&"x".repeat(257))),Err(pablo_core::a2a::IdentityError::Invalid));
        assert_eq!(first.remote(),&bound);assert_eq!(serde_json::to_value(first.agent()).unwrap(),local);
        let requests=std::fs::read_to_string(cwd.join("requests.jsonl")).unwrap();let requests:Vec<Value>=requests.lines().map(|l|serde_json::from_str(l).unwrap()).collect();
        assert_eq!(requests,vec![json!({"method":"GET","path":"/.well-known/agent-card.json","version":"1.0","authenticated":false});2]);
    }).await;
    peer.kill().await.unwrap();
    peer.wait().await.unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
    outcome.unwrap();
}

#[tokio::test]
async fn card_fetch_rejects_redirects_media_overflow_and_stop_before_network() {
    use pablo_core::{
        CancellationToken,
        a2a::{CardClient, FetchError},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let client = CardClient::new().unwrap();
    for (response,expected) in [
        ("HTTP/1.1 302 Found\r\nLocation: https://other.example.test/card\r\nContent-Length: 0\r\n\r\n".to_owned(),FetchError::HttpStatus(302)),
        ("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n".to_owned(),FetchError::MediaType),
        (format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",MAX_CARD_BYTES+1),FetchError::Admission(AdmissionError::CardBound)),
    ] {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target=format!("http://{}/card",listener.local_addr().unwrap());
        let peer=async {
            let (mut stream,_)=listener.accept().await.unwrap();let mut bytes=vec![0;8192];let mut n=0;
            while !bytes[..n].windows(4).any(|w|w==b"\r\n\r\n") {
                assert!(n<bytes.len());let read=stream.read(&mut bytes[n..]).await.unwrap();assert!(read>0);n+=read;
            }
            let request=String::from_utf8_lossy(&bytes[..n]).to_lowercase();assert!(request.contains("a2a-version: 1.0"));assert!(!request.contains("authorization:"));
            stream.write_all(response.as_bytes()).await.unwrap();
        };
        let selection=host();let cancel=CancellationToken::new();
        let fetch=client.fetch_fixture(&selection,"https://agent.example.test/card",&target,tokio::time::Instant::now()+std::time::Duration::from_secs(5),&cancel);
        let (result,())=tokio::join!(fetch,peer);assert_eq!(result.unwrap_err(),expected);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!("http://{}/card", listener.local_addr().unwrap());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        client
            .fetch_fixture(
                &host(),
                "https://agent.example.test/card",
                &target,
                tokio::time::Instant::now() + std::time::Duration::from_secs(5),
                &cancelled
            )
            .await
            .unwrap_err(),
        FetchError::Cancelled
    );
    assert_eq!(
        client
            .fetch_fixture(
                &host(),
                "https://agent.example.test/card",
                &target,
                tokio::time::Instant::now(),
                &CancellationToken::new()
            )
            .await
            .unwrap_err(),
        FetchError::TimedOut
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn legacy_or_malformed_security_requirements_do_not_become_public_access() {
    let mut card = fixture();
    card["securityRequirements"] = json!([{"bearer":["admin"]}]);
    assert_eq!(
        host()
            .validate(&serde_json::to_vec(&card).unwrap())
            .unwrap_err(),
        AdmissionError::InvalidCard
    );
    card["securityRequirements"] = json!([{"schemes":{"bearer":{"scopes":["admin"]}}}]);
    assert_eq!(
        host()
            .validate(&serde_json::to_vec(&card).unwrap())
            .unwrap_err(),
        AdmissionError::InvalidCard
    );
}

fn verify_proxy_ledger(
    parent: &pablo_core::children::AgentRef,
    proxy: &pablo_core::a2a::RemoteProxy,
) {
    use pablo_core::{
        RunEvent, RunLimits,
        children::{AgentKind, ledger::RootLedger},
    };
    let ledger = RootLedger::new(parent, RunLimits::default()).unwrap();
    assert!(
        ledger
            .bind_execution(proxy.agent().agent_id(), None)
            .is_err()
    );
    ledger
        .register_child(proxy.agent(), RunLimits::default())
        .unwrap();
    let binding = ledger
        .bind_execution(proxy.agent().agent_id(), None)
        .unwrap();
    assert_eq!(binding.agent.kind(), AgentKind::RemoteA2a);
    assert_ne!(binding.agent.session_id(), parent.root_session_id());
    let mut event = json!({"schema_version":"0.1","seq":0,"timestamp_unix_micros":0,"run_id":binding.run_id,"session_id":binding.agent.session_id(),"agent":binding.agent,"trace_id":"00","span_id":"00","parent_span_id":null,"trace_flags":"00","type":"run.started"});
    assert!(
        ledger.event_source_matches(&serde_json::from_value::<RunEvent>(event.clone()).unwrap())
    );
    event["agent"]["kind"] = "local_acp_temporary".into();
    assert!(!ledger.event_source_matches(&serde_json::from_value::<RunEvent>(event).unwrap()));
}
