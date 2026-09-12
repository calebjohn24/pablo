use super::*;
use crate::provider::ModelRequest;
use opentelemetry::Context;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn profile() -> OpenResponsesProfile {
    OpenResponsesProfile::new(
        "https://responses.example.test/v1/responses",
        "fixture-text-tools-v1",
        PROFILE,
        "Authorization",
        "bearer",
    )
    .unwrap()
}

#[test]
fn k01_bounded_sse_mutations_never_escape_adapter_limits_or_panic() {
    let seed = include_bytes!("../../../../../docs/project/fixtures/c3-open-responses/turn-1.sse");
    for bytes in crate::protocol_properties::mutations(seed) {
        let mut decoder = transport::SseDecoder::default();
        let mut completion = Completion::new(&request(), &profile());
        for byte in bytes {
            let frame = match decoder.push(byte) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(_) => break,
            };
            if completion
                .frame(decoder.take_event().as_deref(), &frame)
                .is_err()
            {
                break;
            }
        }
    }
}
fn request() -> ModelRequest<'static> {
    ModelRequest {
        model: "fixture-text-tools-v1",
        input: "",
        instructions: "",
        messages: &[],
        continuations: &[],
        max_continuation_bytes: MAX_STATE,
        max_context_bytes: MAX_STATE,
        max_tool_input_bytes: 1024 * 1024,
        max_output_bytes: 4 * 1024 * 1024,
        tools: &[],
        allow_tool_calls: true,
        max_output_tokens: 4096,
        deadline: Instant::now() + Duration::from_secs(60),
        context: Context::new(),
        cancellation: CancellationToken::new(),
    }
}
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../docs/project/fixtures/c3-open-responses/roundtrip.json"
    ))
    .unwrap()
}
fn events() -> Vec<Value> {
    fixture()["turns"][0]["events"].as_array().unwrap().clone()
}
fn feed(events: &[Value]) -> Result<Completion, ProviderError> {
    let mut completion = Completion::new(&request(), &profile());
    for event in events {
        completion.frame(
            Some(event["type"].as_str().unwrap().as_bytes()),
            &serde_json::to_vec(event).unwrap(),
        )?;
    }
    Ok(completion)
}

#[test]
fn pinned_wires_preserve_private_items_phases_text_fragments_and_usage() {
    let fixture = fixture();
    for (n, wire) in [
        include_bytes!("../../../../../docs/project/fixtures/c3-open-responses/turn-1.sse")
            .as_slice(),
        include_bytes!("../../../../../docs/project/fixtures/c3-open-responses/turn-2.sse")
            .as_slice(),
        include_bytes!("../../../../../docs/project/fixtures/c3-open-responses/turn-3.sse")
            .as_slice(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut decoder = transport::SseDecoder::default();
        let mut completion = Completion::new(&request(), &profile());
        let mut output = Vec::new();
        for byte in wire {
            if let Some(frame) = decoder.push(*byte).unwrap() {
                completion
                    .frame(decoder.take_event().as_deref(), &frame)
                    .unwrap();
                output.extend(completion.pending.drain(..));
            }
        }
        assert_eq!(
            output
                .iter()
                .filter(|e| matches!(e, ProviderEvent::Finished { .. }))
                .count(),
            1
        );
        assert_eq!(
            output
                .iter()
                .filter(|e| matches!(e, ProviderEvent::Continuation(_)))
                .count(),
            1
        );
        let text = output
            .iter()
            .filter_map(|e| {
                if let ProviderEvent::TextDelta(text) = e {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<String>();
        assert_eq!(text, fixture["turns"][n]["expected"]["text"]);
        let continuation = output
            .iter()
            .find_map(|e| {
                if let ProviderEvent::Continuation(c) = e {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            Value::Array(continuation.items.clone()),
            fixture["turns"][n]["events"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["response"]["output"]
        );
        for sentinel in fixture["private_sentinels"].as_array().unwrap() {
            assert!(!format!("{output:?}").contains(sentinel.as_str().unwrap()));
        }
        let ProviderEvent::Finished { usage, reason } = output.last().unwrap() else {
            panic!("missing finish")
        };
        assert_eq!(
            serde_json::to_value(usage).unwrap(),
            fixture["turns"][n]["expected"]["usage"]
        );
        assert_eq!(
            *reason,
            if n < 2 {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            }
        );
    }
}

#[test]
fn invalid_sequence_identity_arguments_and_snapshots_reject() {
    for case in 0..10 {
        let mut e = events();
        match case {
            0 => e[1]["sequence_number"] = 0.into(),
            1 => e[1]["response"]["id"] = "changed".into(),
            2 => e[2]["output_index"] = 4096.into(),
            3 => {
                e.iter_mut()
                    .find(|v| v["type"] == "response.output_text.delta")
                    .unwrap()["item_id"] = "wrong".into()
            }
            4 => {
                e.iter_mut()
                    .find(|v| v["type"] == "response.output_text.done")
                    .unwrap()["text"] = "mismatch".into()
            }
            5 => {
                e.iter_mut()
                    .find(|v| v["type"] == "response.function_call_arguments.done")
                    .unwrap()["arguments"] = "{}".into()
            }
            6 => e.last_mut().unwrap()["response"]["output"] = json!([]),
            7 => {
                e.iter_mut()
                    .find(|v| {
                        v["type"] == "response.output_item.done"
                            && v["item"]["type"] == "function_call"
                    })
                    .unwrap()["item"]["call_id"] = "changed".into()
            }
            8 => {
                let mut last = e.last().unwrap().clone();
                last["sequence_number"] = 100.into();
                e.push(last);
            }
            9 => e[0]["response"]["background"] = true.into(),
            _ => unreachable!(),
        }
        assert!(feed(&e).is_err(), "case {case}");
    }
    let mut c = Completion::new(&request(), &profile());
    assert!(c.frame(None, b"[DONE]").is_err());
    for event in [None, Some(b"response.unknown".as_slice())] {
        let mut c = Completion::new(&request(), &profile());
        assert!(
            c.frame(event, &serde_json::to_vec(&events()[0]).unwrap())
                .is_err()
        );
    }
}

#[test]
fn unsupported_semantics_and_duplicate_nested_fields_fail_closed() {
    for case in 0..4 {
        let mut e = events();
        match case {
            0 => e[2]["item"]["content"] = json!([{"type":"reasoning_text","text":"private"}]),
            1 => e[2]["item"]["type"] = "vendor:opaque_tool".into(),
            2 => e[2]["type"] = "response.unknown_required_event".into(),
            3 => e[2]["item"]["unrecognized_semantics"] = true.into(),
            _ => unreachable!(),
        }
        assert_eq!(
            feed(&e).err().unwrap().code,
            FailureCode::UnsupportedProviderContent
        );
    }
    for bytes in [
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"usage":{"input_tokens":1,"input_tokens":2}}"#,
        b"\xff",
    ] {
        assert_eq!(
            unique::parse(bytes).unwrap_err().code,
            FailureCode::MalformedStream
        );
    }
}

#[test]
fn bounded_retention_token_usage_and_private_debug() {
    let mut request = request();
    request.max_continuation_bytes = 32;
    let mut c = Completion::new(&request, &profile());
    let e = events();
    for event in &e[..2] {
        c.frame(
            Some(event["type"].as_str().unwrap().as_bytes()),
            &serde_json::to_vec(event).unwrap(),
        )
        .unwrap();
    }
    assert!(
        c.frame(
            Some(b"response.output_item.added"),
            &serde_json::to_vec(&e[2]).unwrap()
        )
        .is_err()
    );
    assert_eq!(usage(&Value::Null).unwrap(), Usage::default());
    let large = json!({"input_tokens":9007199254740993u64,"output_tokens":1,"total_tokens":9007199254740994u64,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}});
    assert_eq!(usage(&large).unwrap().input_tokens, Some(9007199254740993));
    for (key, value) in [
        ("input_tokens", json!(-1)),
        ("total_tokens", json!(1)),
        ("output_tokens", json!(u64::MAX)),
    ] {
        let mut v = large.clone();
        v[key] = value;
        assert!(usage(&v).is_err());
    }
}

#[test]
fn profile_rejects_insecure_destinations_and_transport_header_override() {
    for endpoint in [
        "http://127.0.0.1/responses",
        "https://user:secret@example.test/responses",
        "https://example.test/responses?key=secret",
        "https://example.test/#fragment",
    ] {
        assert!(
            OpenResponsesProfile::new(endpoint, "model", PROFILE, "Authorization", "bearer")
                .is_err()
        );
    }
    for header in [
        "Host",
        "Cookie",
        "Content-Type",
        "Proxy-Authorization",
        "X-Forwarded-Host",
        "X-Forwarded-Auth-Token",
        "X-Http-Method-Override",
        "X-Original-URL",
        "X-B3-TraceId",
        "bad\nheader",
    ] {
        assert!(
            OpenResponsesProfile::new(profile().endpoint(), "model", PROFILE, header, "raw")
                .is_err()
        );
    }
    assert!(
        OpenResponsesProfile::new(profile().endpoint(), "model", PROFILE, "X-Api-Key", "raw")
            .is_ok()
    );
}

#[test]
fn mcp_aliases_preserve_open_responses_continuation_and_canonical_tool_identity() {
    let name = crate::mcp::qualified("remote", "read").unwrap();
    let alias = crate::mcp::provider_alias(&name).unwrap();
    let mut wire = events();
    fn rename(value: &mut Value, alias: &str) {
        match value {
            Value::Object(map) => {
                if map.get("name").is_some_and(|name| name == "fs_read") {
                    map.insert("name".into(), alias.into());
                }
                for child in map.values_mut() {
                    rename(child, alias);
                }
            }
            Value::Array(values) => {
                for child in values {
                    rename(child, alias);
                }
            }
            _ => {}
        }
    }
    for event in &mut wire {
        rename(event, &alias);
    }
    let mut completion = Completion::new(&request(), &profile());
    completion.aliases.insert(alias.clone(), name.clone());
    for event in &wire {
        completion
            .frame(
                Some(event["type"].as_str().unwrap().as_bytes()),
                &serde_json::to_vec(event).unwrap(),
            )
            .unwrap();
    }
    completion.frame(None, b"[DONE]").unwrap();
    assert!(completion.pending.iter().any(
        |event| matches!(event,ProviderEvent::ToolCallStart{name:actual,..} if actual==&name)
    ));
    assert!(
        completion
            .pending
            .iter()
            .any(|event| matches!(event, ProviderEvent::Continuation(_)))
    );
    assert!(
        feed(&wire).is_err(),
        "an unregistered alias cannot grant a tool"
    );
}
