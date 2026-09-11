use pablo_core::a2a::{
    lifecycle::{Disposition, Error, Lifecycle},
    wire::Mode,
};
use serde_json::{Value, json};
fn envelope(result: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"rpc","result":result})).unwrap()
}
fn fixture(key: &str) -> Value {
    serde_json::from_str::<Value>(include_str!("../../../tests/fixtures/a2a/wire.json")).unwrap()
        [key]
        .clone()
}
fn status(state: &str) -> Value {
    json!({"statusUpdate":{"taskId":"remote-task","contextId":"remote-context","status":{"state":state}}})
}
fn artifact(id: &str, parts: Value, append: bool, last: bool) -> Value {
    json!({"artifactUpdate":{"taskId":"remote-task","contextId":"remote-context","artifact":{"artifactId":id,"parts":parts},"append":append,"lastChunk":last}})
}
fn ingest(l: &mut Lifecycle, v: Value) -> Result<(), Error> {
    l.ingest(&envelope(v), "rpc", Mode::Stream, false)
        .map(|_| ())
}
#[test]
fn sdk_immediate_and_task_shapes_preserve_remote_identity() {
    for key in ["message_result", "task_result"] {
        let mut l = Lifecycle::default();
        l.ingest(&envelope(fixture(key)), "rpc", Mode::Send, false)
            .unwrap();
        let result = l.finish().unwrap();
        assert_eq!(result.disposition, Some(Disposition::Completed));
        assert_eq!(result.remote.context_id.as_deref(), Some("remote-context"));
        if key == "task_result" {
            assert_eq!(result.remote.task_id.as_deref(), Some("remote-task"));
            assert_eq!(
                result.artifacts[0].parts[0].text.as_deref(),
                Some("selected result")
            );
        } else {
            assert_eq!(result.message.unwrap().message_id, "remote-message");
            assert_eq!(result.remote.task_id, None);
        }
    }
}
#[test]
fn streamed_parts_remain_inert_and_in_order() {
    let mut l = Lifecycle::default();
    ingest(&mut l, fixture("stream_status")).unwrap();
    ingest(
        &mut l,
        artifact("a", json!([{"text":"first"}]), false, false),
    )
    .unwrap();
    ingest(&mut l,artifact("a",json!([{"data":{"selected":42}},{"raw":"AP8=","filename":"../../not-local","mediaType":"application/octet-stream"},{"url":"https://files.example.test/inert"}]),true,true)).unwrap();
    ingest(&mut l, status("TASK_STATE_COMPLETED")).unwrap();
    let r = l.finish().unwrap();
    assert_eq!(r.artifacts.len(), 1);
    assert_eq!(r.artifacts[0].parts.len(), 4);
    assert_eq!(
        r.artifacts[0].parts[2].file_bytes().unwrap().unwrap(),
        [0, 255]
    );
    assert_eq!(
        r.artifacts[0].parts[3].url.as_deref(),
        Some("https://files.example.test/inert")
    );
}
#[test]
fn snapshot_replaces_deltas_and_replacement_restarts_artifact() {
    let mut l = Lifecycle::default();
    ingest(&mut l, artifact("a", json!([{"text":"old"}]), false, true)).unwrap();
    ingest(&mut l, artifact("a", json!([{"text":"new"}]), false, false)).unwrap();
    ingest(&mut l, artifact("a", json!([{"text":"tail"}]), true, true)).unwrap();
    assert_eq!(l.result().artifacts[0].parts.len(), 2);
    ingest(&mut l, fixture("stream_task")).unwrap();
    let r = l.finish().unwrap();
    assert_eq!(r.artifacts.len(), 1);
    assert_eq!(r.artifacts[0].artifact_id, "remote-artifact");
}
#[test]
fn changed_identity_and_late_updates_cannot_rewrite_accepted_result() {
    let mut l = Lifecycle::default();
    ingest(&mut l, fixture("stream_status")).unwrap();
    let before = serde_json::to_value(l.result()).unwrap();
    let mut changed = fixture("stream_artifact");
    changed["artifactUpdate"]["taskId"] = "foreign".into();
    assert_eq!(ingest(&mut l, changed), Err(Error::Identity));
    assert_eq!(serde_json::to_value(l.result()).unwrap(), before);
    assert!(l.finish().is_err());
    let mut l = Lifecycle::default();
    ingest(&mut l, fixture("stream_task")).unwrap();
    let before = serde_json::to_value(l.result()).unwrap();
    assert_eq!(
        ingest(&mut l, fixture("stream_artifact")),
        Err(Error::Sequence)
    );
    assert_eq!(serde_json::to_value(l.result()).unwrap(), before);
}
#[test]
fn incomplete_artifacts_and_stream_loss_are_not_success() {
    let mut l = Lifecycle::default();
    ingest(
        &mut l,
        artifact("a", json!([{"text":"partial"}]), false, false),
    )
    .unwrap();
    assert_eq!(
        ingest(&mut l, status("TASK_STATE_COMPLETED")),
        Err(Error::Incomplete)
    );
    assert!(l.finish().is_err());
    let mut l = Lifecycle::default();
    ingest(&mut l, fixture("stream_status")).unwrap();
    assert!(l.finish().is_err());
    for first in [true, false] {
        let mut l = Lifecycle::default();
        if !first {
            ingest(&mut l, artifact("a", json!([{"text":"done"}]), false, true)).unwrap();
        }
        assert_eq!(
            ingest(
                &mut l,
                artifact("a", json!([{"text":"invalid append"}]), true, true)
            ),
            Err(Error::Sequence)
        );
    }
}
#[test]
fn remote_dispositions_remain_distinct() {
    for (state, expected) in [
        ("TASK_STATE_FAILED", Disposition::Failed),
        ("TASK_STATE_CANCELED", Disposition::Canceled),
        ("TASK_STATE_REJECTED", Disposition::Rejected),
        ("TASK_STATE_INPUT_REQUIRED", Disposition::InputRequired),
        ("TASK_STATE_AUTH_REQUIRED", Disposition::AuthRequired),
    ] {
        let mut l = Lifecycle::default();
        ingest(&mut l, status(state)).unwrap();
        assert_eq!(l.finish().unwrap().disposition, Some(expected));
    }
}
#[test]
fn total_result_and_part_count_are_bounded_across_small_envelopes() {
    let mut l = Lifecycle::default();
    ingest(
        &mut l,
        artifact("a", json!([{"text":"x".repeat(40000)}]), false, false),
    )
    .unwrap();
    assert_eq!(
        ingest(
            &mut l,
            artifact("a", json!([{"text":"y".repeat(30000)}]), true, true)
        ),
        Err(Error::Bound)
    );
    assert_eq!(l.result().artifacts[0].parts.len(), 1);
    let mut l = Lifecycle::default();
    ingest(
        &mut l,
        artifact("a", json!(vec![json!({"text":"x"}); 32]), false, false),
    )
    .unwrap();
    assert_eq!(
        ingest(
            &mut l,
            artifact("a", json!([{"text":"one too many"}]), true, true)
        ),
        Err(Error::Bound)
    );
    let mut l = Lifecycle::default();
    for i in 0..16 {
        ingest(
            &mut l,
            artifact(&i.to_string(), json!([{"text":"x"}]), false, true),
        )
        .unwrap();
    }
    assert_eq!(
        ingest(&mut l, artifact("17", json!([{"text":"x"}]), false, true)),
        Err(Error::Bound)
    );
}
#[test]
fn malformed_content_and_protocol_errors_poison_lifecycle() {
    let mut l = Lifecycle::default();
    assert!(
        ingest(
            &mut l,
            artifact("a", json!([{"unknownRequiredContent":"x"}]), false, true)
        )
        .is_err()
    );
    assert!(ingest(&mut l, fixture("stream_task")).is_err());
    assert!(l.finish().is_err());
    let mut l = Lifecycle::default();
    assert!(
        l.ingest(
            &envelope(fixture("task_result")),
            "wrong",
            Mode::Stream,
            false
        )
        .is_err()
    );
    assert!(l.finish().is_err());
}

#[test]
fn valid_task_identity_survives_retained_result_rejection_for_cleanup() {
    let mut value = fixture("task_result");
    value["task"]["artifacts"][0]["parts"][0]["text"] = "".into();
    let remaining = pablo_core::a2a::wire::MAX_RESPONSE_BYTES - envelope(value.clone()).len();
    value["task"]["artifacts"][0]["parts"][0]["text"] = "x".repeat(remaining).into();
    let bytes = envelope(value);
    assert!(pablo_core::a2a::wire::decode(&bytes, "rpc", Mode::Send, false).is_ok());
    let mut lifecycle = Lifecycle::default();
    assert_eq!(
        lifecycle.ingest(&bytes, "rpc", Mode::Send, false),
        Err(Error::Bound)
    );
    assert_eq!(lifecycle.observed().task_id.as_deref(), Some("remote-task"));
    assert_eq!(
        lifecycle.observed().context_id.as_deref(),
        Some("remote-context")
    );
    assert!(lifecycle.result().remote.task_id.is_none());
    assert!(lifecycle.finish().is_err());
}
