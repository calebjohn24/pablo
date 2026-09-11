use super::*;
use crate::children::{
    AgentRef,
    ledger::{AdmissionError, RootLedger},
};
use serde_json::{Value, json};

#[test]
fn defaults_inherit_without_reintroducing_call_or_filesystem_work_limits() {
    let parent = RunLimits::default();
    let child = ChildCeilings::default().apply_to(&parent).unwrap();
    let mut expected = serde_json::to_value(&parent).unwrap();
    expected["max_context_bytes"] = MAX_CONTEXT_BYTES.into();
    expected["max_output_bytes"] = MAX_RESULT_BYTES.into();
    expected["max_input_bytes"] = MAX_INPUT_BYTES.into();
    assert_eq!(serde_json::to_value(&child).unwrap(), expected);
    assert!(inherits(&parent, &child));
    assert_eq!(child.max_model_calls, None);
    assert_eq!(child.max_tool_calls, None);
    assert_eq!(child.filesystem.max_scan_bytes, None);
}

#[test]
fn requested_limits_intersect_parent_and_hard_child_bounds_without_mutating_parent() {
    let parent = RunLimits {
        max_model_calls: Some(3),
        max_tool_calls: Some(4),
        max_total_tokens: Some(100),
        max_cost_microusd: Some(90),
        max_run_duration_ms: 10_000,
        max_context_bytes: 4096,
        max_output_bytes: 2048,
        max_input_bytes: 1024,
        ..RunLimits::default()
    };
    let before = serde_json::to_value(&parent).unwrap();
    let selection = json!({
        "max_model_calls": 2, "max_tool_calls": 0,
        "max_total_tokens": "80", "max_cost_microusd": "0",
        "max_duration_ms": 5000, "max_context_bytes": 2048, "max_output_bytes": 1024
    });
    let ceilings: ChildCeilings = serde_json::from_value(selection.clone()).unwrap();
    let child = ceilings.apply_to(&parent).unwrap();
    assert!(inherits(&parent, &child));
    assert_eq!(child.max_model_calls, Some(2));
    assert_eq!(child.max_tool_calls, Some(0));
    assert_eq!(child.max_total_tokens, Some(80));
    assert_eq!(child.max_cost_microusd, Some(0));
    assert_eq!(child.max_run_duration_ms, 5000);
    assert_eq!(child.max_context_bytes, 2048);
    assert_eq!(child.max_output_bytes, 1024);
    assert_eq!(child.max_input_bytes, 1024);
    for (key, widened) in [
        ("max_model_calls", json!(4)),
        ("max_tool_calls", json!(5)),
        ("max_total_tokens", json!("101")),
        ("max_cost_microusd", json!("91")),
        ("max_duration_ms", json!(10001)),
        ("max_context_bytes", json!(4097)),
        ("max_output_bytes", json!(2049)),
    ] {
        let mut invalid = selection.clone();
        invalid[key] = widened;
        let invalid: ChildCeilings = serde_json::from_value(invalid).unwrap();
        assert!(invalid.apply_to(&parent).is_err(), "{key}");
    }
    assert_eq!(serde_json::to_value(&parent).unwrap(), before);
    for ceilings in [
        ChildCeilings {
            max_context_bytes: Some(MAX_CONTEXT_BYTES + 1),
            ..Default::default()
        },
        ChildCeilings {
            max_output_bytes: Some(MAX_RESULT_BYTES + 1),
            ..Default::default()
        },
    ] {
        assert!(ceilings.apply_to(&RunLimits::default()).is_err());
    }
}

#[test]
fn direct_host_accounting_strings_require_canonical_unsigned_values() {
    for invalid in [
        "",
        "+1",
        "01",
        "-1",
        " 1",
        "1 ",
        "unlimited",
        "18446744073709551616",
    ] {
        for key in ["max_total_tokens", "max_cost_microusd"] {
            let mut value = json!({});
            value[key] = invalid.into();
            let ceilings: ChildCeilings = serde_json::from_value(value).unwrap();
            assert!(
                ceilings.apply_to(&RunLimits::default()).is_err(),
                "{key}: {invalid}"
            );
        }
    }
    let ceilings = ChildCeilings {
        max_total_tokens: Some(u64::MAX.to_string()),
        max_cost_microusd: Some("0".into()),
        ..Default::default()
    };
    let limits = ceilings.apply_to(&RunLimits::default()).unwrap();
    assert_eq!(limits.max_total_tokens, Some(u64::MAX));
    assert_eq!(limits.max_cost_microusd, Some(0));
}

#[test]
fn ledger_rejects_each_widened_host_limit_before_registration() {
    let parent: RunLimits = serde_json::from_value(json!({
        "max_model_calls": 3, "max_tool_calls": 4,
        "max_total_tokens": 100, "max_cost_microusd": 90,
        "filesystem": {"max_file_bytes": 100, "max_entries": 100, "max_depth": 10, "max_scan_bytes": 100},
        "max_tool_duration_ms": 1000, "max_tool_input_bytes": 1024,
        "max_tool_output_bytes": 2048, "max_context_bytes": 8192,
        "max_run_duration_ms": 10000, "max_input_bytes": 1024,
        "max_output_bytes": 2048, "max_output_tokens": 100, "max_events": 100
    })).unwrap();
    let root = AgentRef::root("run".into(), "session".into());
    let ledger = RootLedger::new(&root, parent.clone()).unwrap();
    let child = root.temporary_child().unwrap();
    let before = ledger.total();
    let value = serde_json::to_value(&parent).unwrap();
    let mut paths = Vec::new();
    for (key, field) in value.as_object().unwrap() {
        if let Some(nested) = field.as_object() {
            paths.extend(nested.keys().map(|name| format!("/{key}/{name}")));
        } else {
            paths.push(format!("/{key}"));
        }
    }
    for path in paths {
        let mut widened = value.clone();
        let field = widened.pointer_mut(&path).unwrap();
        *field = (field.as_u64().unwrap() + 1).into();
        let limits = serde_json::from_value(widened).unwrap();
        assert!(
            matches!(
                ledger.register_child(&child, limits),
                Err(AdmissionError::InvalidCeiling)
            ),
            "{path}"
        );
        assert!(ledger.agent(child.agent_id()).is_none());
        assert_eq!(ledger.total(), before);
    }
    for path in [
        "/max_model_calls",
        "/max_tool_calls",
        "/max_total_tokens",
        "/max_cost_microusd",
        "/filesystem/max_file_bytes",
        "/filesystem/max_entries",
        "/filesystem/max_depth",
        "/filesystem/max_scan_bytes",
    ] {
        let mut erased = value.clone();
        *erased.pointer_mut(path).unwrap() = Value::Null;
        assert!(
            matches!(
                ledger.register_child(&child, serde_json::from_value(erased).unwrap()),
                Err(AdmissionError::InvalidCeiling)
            ),
            "{path}"
        );
    }
    // Rejected registrations neither consume identity nor total-child capacity.
    ledger
        .register_child(&child, ChildCeilings::default().apply_to(&parent).unwrap())
        .unwrap();
    assert!(ledger.agent(child.agent_id()).is_some());
}
