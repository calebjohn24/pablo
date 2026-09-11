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
    use pablo_core::{CancellationToken, a2a::CardClient};
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
        let card=CardClient::new().unwrap().fetch_fixture(&host(),"https://agent.example.test/.well-known/agent-card.json",ready["url"].as_str().unwrap(),tokio::time::Instant::now()+std::time::Duration::from_secs(10),&CancellationToken::new()).await.unwrap();
        assert_eq!(card.protocol_version,"1.0");assert_eq!(card.endpoint,"https://agent.example.test/rpc");assert!(card.streaming);
        let requests=std::fs::read_to_string(cwd.join("requests.jsonl")).unwrap();let requests:Vec<Value>=requests.lines().map(|l|serde_json::from_str(l).unwrap()).collect();
        assert_eq!(requests,json!([{"method":"GET","path":"/.well-known/agent-card.json","version":"1.0","authenticated":false}]).as_array().unwrap().clone());
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
            let (mut stream,_)=listener.accept().await.unwrap();let mut bytes=vec![0;8192];let n=stream.read(&mut bytes).await.unwrap();
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
