use futures_util::FutureExt;
use pablo_core::a2a::wire::{self, Error, Mode, Reply};
use serde_json::{Value, json};
fn vectors() -> Value {
    serde_json::from_str(include_str!("../../../tests/fixtures/a2a/wire.json")).unwrap()
}
fn response(value: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"rpc","result":value})).unwrap()
}
#[test]
fn request_encoders_match_independent_sdk_proto_vectors() {
    let v = vectors();
    assert_eq!(
        serde_json::from_slice::<Value>(
            &wire::send_request(
                "rpc-send",
                "local-message",
                "explicit selected input",
                false
            )
            .unwrap()
        )
        .unwrap(),
        v["send"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(
            &wire::send_request(
                "rpc-stream",
                "local-message",
                "explicit selected input",
                true
            )
            .unwrap()
        )
        .unwrap(),
        v["stream"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(
            &wire::cancel_request("rpc-cancel", "remote-task").unwrap()
        )
        .unwrap(),
        v["cancel"]
    );
    let escaped = "\u{0000}".repeat(wire::MAX_INPUT_BYTES);
    assert!(
        wire::send_request("rpc", "message", &escaped, false)
            .unwrap()
            .len()
            <= wire::MAX_REQUEST_BYTES
    );
    assert_eq!(
        wire::send_request("rpc", "message", &(escaped + "x"), false),
        Err(Error::Bound)
    );
    assert_eq!(wire::cancel_request("rpc", ""), Err(Error::Invalid));
}
#[test]
fn sdk_send_stream_and_cancel_shapes_preserve_remote_ids_and_states() {
    let v = vectors();
    for name in ["message_result", "stream_message"] {
        let Reply::Message(m) =
            wire::decode(&response(v[name].clone()), "rpc", Mode::Send, false).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (&*m.message_id, &*m.context_id),
            ("remote-message", "remote-context")
        );
        assert!(m.task_id.is_none());
        assert_eq!(m.parts[0].text.as_deref(), Some("selected result"));
    }
    for (name, mode) in [
        ("task_result", Mode::Send),
        ("stream_task", Mode::Stream),
        ("cancel_result", Mode::Cancel),
    ] {
        let Reply::Task(t) = wire::decode(&response(v[name].clone()), "rpc", mode, false).unwrap()
        else {
            panic!()
        };
        assert_eq!((&*t.id, &*t.context_id), ("remote-task", "remote-context"));
        assert_eq!(t.status.state, wire::TaskState::Completed);
        assert_eq!(t.artifacts[0].artifact_id, "remote-artifact");
    }
    assert!(matches!(
        wire::decode(
            &response(v["stream_status"].clone()),
            "rpc",
            Mode::Stream,
            false
        )
        .unwrap(),
        Reply::Status(_)
    ));
    let Reply::Artifact(a) = wire::decode(
        &response(v["stream_artifact"].clone()),
        "rpc",
        Mode::Stream,
        false,
    )
    .unwrap() else {
        panic!()
    };
    assert!(a.last_chunk);
    assert!(!a.append);
    assert!(
        wire::decode(
            &response(v["task_result"].clone()),
            "rpc",
            Mode::Cancel,
            false
        )
        .is_err()
    );
    assert!(
        wire::decode(
            &response(v["stream_status"].clone()),
            "rpc",
            Mode::Send,
            false
        )
        .is_err()
    );
}
#[test]
fn malformed_union_ids_legacy_aliases_and_unsupported_parts_reject() {
    let v = vectors();
    for parts in [
        json!([{"url":"file:///private/local"}]),
        json!([{"raw":"invalid!"}]),
        json!([{"text":"x","data":null}]),
        json!([{"kind":"text"}]),
    ] {
        let mut m = v["message_result"].clone();
        m["message"]["parts"] = parts;
        assert!(wire::decode(&response(m), "rpc", Mode::Send, false).is_err());
    }
    let mut both = v["message_result"].clone();
    both["task"] = v["task_result"]["task"].clone();
    assert!(wire::decode(&response(both), "rpc", Mode::Send, false).is_err());
    let mut m = v["message_result"].clone();
    m["message"]["role"] = "agent".into();
    assert!(wire::decode(&response(m), "rpc", Mode::Send, false).is_err());
    let mut m = v["message_result"].clone();
    m["message"]["contextId"] = "x".repeat(257).into();
    assert!(wire::decode(&response(m), "rpc", Mode::Send, false).is_err());
    let bytes = response(v["message_result"].clone());
    assert!(wire::decode(&bytes, "foreign", Mode::Send, false).is_err());
    let duplicate = String::from_utf8(bytes).unwrap().replace(
        "\"messageId\":",
        "\"messageId\":\"duplicate\",\"messageId\":",
    );
    assert!(wire::decode(duplicate.as_bytes(), "rpc", Mode::Send, false).is_err());
    let mut task = v["task_result"].clone();
    task["task"]["history"] = json!([{}]);
    assert!(wire::decode(&response(task), "rpc", Mode::Send, false).is_err());
}
#[test]
fn structured_null_and_extensions_obey_selected_profile() {
    let mut m = vectors()["message_result"].clone();
    m["message"]["parts"] = json!([{"data":null}]);
    let Reply::Message(message) =
        wire::decode(&response(m.clone()), "rpc", Mode::Send, false).unwrap()
    else {
        panic!()
    };
    assert_eq!(message.parts[0].data, Some(Value::Null));
    m["message"]["extensions"] = json!([pablo_core::a2a::TRACE_EXTENSION]);
    assert!(wire::decode(&response(m.clone()), "rpc", Mode::Send, false).is_ok());
    assert!(wire::decode(&response(m.clone()), "rpc", Mode::Send, true).is_ok());
    m["message"]["extensions"] = json!(["urn:unknown:required"]);
    assert!(wire::decode(&response(m), "rpc", Mode::Send, true).is_ok());
}
#[test]
fn errors_and_stream_bounds_remain_local_and_finite() {
    for code in [
        -32700, -32600, -32601, -32602, -32603, -32001, -32002, -32003, -32004, -32005, -32006,
        -32007, -32008, -32009,
    ] {
        let bytes=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"rpc","error":{"code":code,"message":"untrusted instructions","data":{"private":"not a local instruction"}}})).unwrap();
        assert!(
            matches!(wire::decode(&bytes,"rpc",Mode::Send,false),Err(Error::Remote(c)) if c==code)
        );
    }
    let mut budget = wire::StreamBudget::default();
    for _ in 0..wire::MAX_STREAM_UPDATES {
        budget.charge(1).unwrap();
    }
    assert_eq!(budget.charge(1), Err(Error::Bound));
    let mut budget = wire::StreamBudget::default();
    for _ in 0..wire::MAX_STREAM_BYTES / wire::MAX_RESPONSE_BYTES {
        budget.charge(wire::MAX_RESPONSE_BYTES).unwrap();
    }
    assert_eq!(budget.charge(1), Err(Error::Bound));
    assert!(matches!(
        wire::decode(
            &vec![b' '; wire::MAX_RESPONSE_BYTES + 1],
            "rpc",
            Mode::Send,
            false
        ),
        Err(Error::Bound)
    ));
}

#[tokio::test]
#[ignore = "requires isolated pinned A2A Python SDK HTTP reference server"]
async fn independent_sdk_dispatcher_roundtrips_send_sse_cancel_and_version_error() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let cwd = std::env::temp_dir().join(format!("pablo-a2a-wire-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&cwd).unwrap();
    let mut peer = tokio::process::Command::new(root.join(".pablo/a2a-fixture-venv/bin/python"))
        .arg(root.join("tests/fixtures/a2a/wire_server.py"))
        .current_dir(&cwd)
        .env_clear()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        std::panic::AssertUnwindSafe(async {
            let mut lines = BufReader::new(peer.stdout.take().unwrap()).lines();
            let ready: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let url = ready["url"].as_str().unwrap();
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            for (input, task) in [("explicit synthetic input", false), ("task", true)] {
                let bytes = client
                    .post(url)
                    .header("A2A-Version", "1.0")
                    .header("Content-Type", "application/json")
                    .body(wire::send_request("rpc", "message", input, false).unwrap())
                    .send()
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap();
                let reply = wire::decode(&bytes, "rpc", Mode::Send, false).unwrap();
                match reply {
                    Reply::Message(m) => {
                        assert!(!task);
                        assert_eq!(m.parts[0].text.as_deref(), Some(input));
                    }
                    Reply::Task(_) => assert!(task),
                    _ => panic!(),
                }
            }
            let reply = client
                .post(url)
                .header("A2A-Version", "1.0")
                .header("Content-Type", "application/json")
                .body(wire::send_request("rpc", "message", "stream", true).unwrap())
                .send()
                .await
                .unwrap();
            assert!(
                reply.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("text/event-stream")
            );
            let text = reply.text().await.unwrap();
            assert!(text.len() < wire::MAX_STREAM_BYTES);
            let mut budget = wire::StreamBudget::default();
            let mut kinds = Vec::new();
            for line in text.lines().filter_map(|line| line.strip_prefix("data: ")) {
                budget.charge(line.len()).unwrap();
                kinds.push(
                    match wire::decode(line.as_bytes(), "rpc", Mode::Stream, false).unwrap() {
                        Reply::Status(_) => "status",
                        Reply::Artifact(_) => "artifact",
                        Reply::Task(_) => "task",
                        _ => panic!(),
                    },
                );
            }
            assert_eq!(kinds, ["status", "artifact", "task"]);
            for task_id in ["remote-task", "missing"] {
                let bytes = client
                    .post(url)
                    .header("A2A-Version", "1.0")
                    .header("Content-Type", "application/json")
                    .body(wire::cancel_request("rpc", task_id).unwrap())
                    .send()
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap();
                match wire::decode(&bytes, "rpc", Mode::Cancel, false) {
                    Ok(Reply::Task(t)) => {
                        assert_eq!(task_id, "remote-task");
                        assert_eq!(t.status.state, wire::TaskState::Canceled);
                    }
                    Err(Error::Remote(-32001)) => assert_eq!(task_id, "missing"),
                    other => panic!("unexpected {other:?}"),
                }
            }
            let before = std::fs::read_to_string(cwd.join("calls.jsonl")).unwrap();
            let bytes = client
                .post(url)
                .header("A2A-Version", "9.9")
                .header("Content-Type", "application/json")
                .body(wire::send_request("rpc", "message", "must not dispatch", false).unwrap())
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            assert!(matches!(
                wire::decode(&bytes, "rpc", Mode::Send, false),
                Err(Error::Remote(-32009))
            ));
            assert_eq!(
                std::fs::read_to_string(cwd.join("calls.jsonl")).unwrap(),
                before
            );
            assert_eq!(before.lines().count(), 5);
            let trace = pablo_core::a2a::trace::TraceContext::new(
                &format!("00-{}-{}-01", "1".repeat(32), "2".repeat(16)),
                Some("vendor=value"),
            )
            .unwrap();
            for (part, negotiated) in [
                (
                    wire::Part::file(
                        b"\0\xffsynthetic file",
                        "application/octet-stream",
                        Some("fixture.bin"),
                    )
                    .unwrap(),
                    false,
                ),
                (
                    wire::Part::url_reference(
                        "http://127.0.0.1:1/must-not-fetch",
                        "application/octet-stream",
                    )
                    .unwrap(),
                    false,
                ),
                (wire::Part::data(json!({"selected":42.0})), false),
                (wire::Part::text("traced"), true),
                (wire::Part::text("generic"), false),
            ] {
                let expected = serde_json::to_value(&part).unwrap();
                let request = wire::send_parts_request(
                    "rpc",
                    "message",
                    &[part],
                    &wire::SendOptions {
                        accepted_output_modes: &[
                            "text/plain",
                            "application/json",
                            "application/octet-stream",
                        ],
                        trace: Some(&trace),
                        trace_negotiated: negotiated,
                        ..Default::default()
                    },
                )
                .unwrap();
                let bytes = client
                    .post(url)
                    .headers(request.headers)
                    .body(request.body)
                    .send()
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap();
                let reply = wire::decode(&bytes, "rpc", Mode::Send, negotiated).unwrap();
                assert_eq!(
                    reply.remote_trace().unwrap(),
                    negotiated.then_some(trace.clone())
                );
                let Reply::Message(message) = reply else {
                    panic!()
                };
                assert_eq!(serde_json::to_value(&message.parts[0]).unwrap(), expected);
            }
            assert_eq!(
                std::fs::read_to_string(cwd.join("calls.jsonl"))
                    .unwrap()
                    .lines()
                    .count(),
                10
            );
        })
        .catch_unwind(),
    )
    .await;
    peer.kill().await.unwrap();
    peer.wait().await.unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
    outcome.unwrap().unwrap();
}

#[test]
fn file_parts_match_sdk_and_enforce_bytes_without_local_file_semantics() {
    use wire::{Part, SendOptions};
    let v = vectors();
    let file = Part::file(
        b"\0\xffsynthetic file",
        "application/octet-stream",
        Some("fixture.bin"),
    )
    .unwrap();
    let request = wire::send_parts_request(
        "rpc-file",
        "local-message",
        &[file],
        &SendOptions {
            accepted_output_modes: &["application/octet-stream"],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body).unwrap(),
        v["file_send"]
    );
    let url = Part::url_reference(
        "https://files.example.test/inert",
        "application/octet-stream",
    )
    .unwrap();
    assert!(url.file_bytes().unwrap().is_none());
    let request = wire::send_parts_request(
        "rpc-url",
        "local-message",
        &[url],
        &SendOptions {
            accepted_output_modes: &["application/octet-stream"],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body).unwrap(),
        v["url_send"]
    );
    let file = Part::file(
        &vec![255; wire::MAX_FILE_BYTES],
        "application/octet-stream",
        Some("../../not-a-local-path"),
    )
    .unwrap();
    assert_eq!(
        file.file_bytes().unwrap().unwrap(),
        vec![255; wire::MAX_FILE_BYTES]
    );
    wire::send_parts_request("rpc", "message", &[file], &SendOptions::default()).unwrap();
    assert!(matches!(
        Part::file(
            &vec![0; wire::MAX_FILE_BYTES + 1],
            "application/octet-stream",
            None
        ),
        Err(Error::Bound)
    ));
    let file = Part::file(
        &vec![0; wire::MAX_FILE_BYTES],
        "application/octet-stream",
        None,
    )
    .unwrap();
    assert!(matches!(
        wire::send_parts_request(
            "rpc",
            "message",
            &[file, Part::text("x")],
            &SendOptions::default()
        ),
        Err(Error::Bound)
    ));
    for raw in ["Zg==", "Zg", "_w==", "_w"] {
        let part: Part = serde_json::from_value(json!({"raw":raw})).unwrap();
        assert_eq!(part.file_bytes().unwrap().unwrap().len(), 1);
    }
    for raw in ["not base64!", "Zh==", "Zg==="] {
        let part: Part = serde_json::from_value(json!({"raw":raw})).unwrap();
        assert!(part.file_bytes().is_err());
    }
    assert!(Part::url_reference("file:///private/file", "application/octet-stream").is_err());
    assert!(
        Part::url_reference(
            "https://user:password@example.test/file",
            "application/octet-stream"
        )
        .is_err()
    );
}

#[test]
fn trace_negotiation_bounds_private_fields_and_remote_parent_separation() {
    use opentelemetry::{
        Context,
        trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState},
    };
    use pablo_core::a2a::{TRACE_EXTENSION, trace::TraceContext};
    let parent = format!("00-{}-{}-01", "1".repeat(32), "2".repeat(16));
    let trace = TraceContext::new(&parent, Some("vendor=value")).unwrap();
    for bad in [
        "".into(),
        parent.replace("00-", "ff-"),
        parent.replace(&"1".repeat(32), &"0".repeat(32)),
        parent.replace(&"2".repeat(16), &"0".repeat(16)),
        parent.replace('1', "G"),
    ] {
        assert!(TraceContext::new(&bad, None).is_err());
    }
    for bad in [
        "a=1,a=2".into(),
        "bad key=value".into(),
        "a=line\nbreak".into(),
        "a=".to_string() + &"x".repeat(513),
    ] {
        assert!(
            TraceContext::new(&parent, Some(&bad)).is_err(),
            "accepted invalid state"
        );
    }
    let request = wire::send_parts_request(
        "rpc",
        "message",
        &[wire::Part::text("selected")],
        &wire::SendOptions {
            trace: Some(&trace),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!request.headers.contains_key("traceparent"));
    assert!(
        !String::from_utf8(request.body)
            .unwrap()
            .contains(TRACE_EXTENSION)
    );
    let local = SpanContext::new(
        TraceId::from_hex(&"3".repeat(32)).unwrap(),
        SpanId::from_hex(&"4".repeat(16)).unwrap(),
        TraceFlags::SAMPLED,
        false,
        TraceState::default(),
    );
    let _guard = Context::new()
        .with_remote_span_context(local.clone())
        .attach();
    let mut result = vectors()["message_result"].clone();
    result["message"]["metadata"] = trace.metadata();
    result["message"]["extensions"] = json!([TRACE_EXTENSION]);
    let reply = wire::decode(&response(result.clone()), "rpc", Mode::Send, true).unwrap();
    assert_eq!(reply.remote_trace().unwrap(), Some(trace));
    assert_eq!(Context::current().span().span_context(), &local);
    assert!(
        wire::decode(&response(result.clone()), "rpc", Mode::Send, false)
            .unwrap()
            .remote_trace()
            .unwrap()
            .is_none()
    );
    result["message"]["metadata"][TRACE_EXTENSION]["baggage"] = "private-policy".into();
    assert!(wire::decode(&response(result), "rpc", Mode::Send, true).is_err());
}

#[test]
fn status_message_identity_must_match_its_task() {
    let v = vectors();
    for (name, mode, key) in [
        ("task_result", Mode::Send, "task"),
        ("stream_status", Mode::Stream, "statusUpdate"),
    ] {
        let mut result = v[name].clone();
        result[key]["status"]["message"] = v["message_result"]["message"].clone();
        wire::decode(&response(result.clone()), "rpc", mode, false).unwrap();
        result[key]["status"]["message"]["contextId"] = "foreign-context".into();
        assert!(wire::decode(&response(result), "rpc", mode, false).is_err());
    }
}
