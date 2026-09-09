#![cfg(unix)]
use futures_util::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use pablo_core::{
    filesystem::{FilesystemResult, FsError},
    policy::{DefaultDecision, Policy, PolicySet, Rule, Rules},
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    tool::{ToolResult, ToolStatus},
    *,
};
use serde_json::{Value, json};
use std::os::unix::fs::symlink;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
struct Fixture {
    base: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("pablo-fs-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(base.join("workspace/nested")).unwrap();
        fs::create_dir(base.join("outside")).unwrap();
        let base = base.canonicalize().unwrap();
        let root = base.join("workspace");
        for (path, text) in [
            ("a.txt", "alpha\nbeta alpha\n"),
            ("unicode.txt", "Aé🙂Z\n"),
            ("nested/b.txt", "alpha\r\nnone\n"),
            ("nested/c.txt", "ALPHA\n"),
            ("empty.txt", ""),
            (".hidden", "alpha\n"),
        ] {
            fs::write(root.join(path), text).unwrap();
        }
        fs::write(root.join("binary.bin"), [0, 255, 97]).unwrap();
        fs::write(base.join("outside/outside.txt"), "outside sentinel\n").unwrap();
        symlink("a.txt", root.join("internal-link")).unwrap();
        symlink(base.join("outside"), root.join("outside-link")).unwrap();
        Self { base, root }
    }
    fn spec(&self) -> RunSpec {
        let mut s = RunSpec::new("Read synthetic evidence", self.root.clone(), "fixture/fs");
        s.limits.max_model_calls = Some(2);
        s.limits.max_tool_calls = Some(1);
        s
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}
struct RoundTrip {
    name: String,
    args: Value,
    calls: AtomicUsize,
    catalog: Mutex<Option<Value>>,
    result: Mutex<Option<ToolResult>>,
}
impl RoundTrip {
    fn new(name: &str, args: Value) -> Self {
        Self {
            name: name.into(),
            args,
            calls: AtomicUsize::new(0),
            catalog: Mutex::new(None),
            result: Mutex::new(None),
        }
    }
}
impl Provider for RoundTrip {
    fn name(&self) -> &'static str {
        "filesystem-fixture"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                *self.catalog.lock().unwrap() = Some(serde_json::to_value(request.tools).unwrap());
                vec![
                    ProviderEvent::ToolCallStart {
                        id: "fs-call".into(),
                        name: self.name.clone(),
                    },
                    ProviderEvent::ToolCallArgumentsDelta {
                        id: "fs-call".into(),
                        delta: self.args.to_string(),
                    },
                    ProviderEvent::Finished {
                        reason: FinishReason::ToolCalls,
                        usage: Usage::default(),
                    },
                ]
            } else {
                assert_eq!(
                    Some(serde_json::to_value(request.tools).unwrap()),
                    *self.catalog.lock().unwrap()
                );
                let Message::Tool {
                    call_id,
                    name,
                    result,
                } = request.messages.last().unwrap()
                else {
                    panic!("missing actual tool result")
                };
                assert_eq!(call_id, "fs-call");
                assert_eq!(name, &self.name);
                *self.result.lock().unwrap() = Some(result.clone());
                vec![
                    ProviderEvent::TextDelta("observed actual result".into()),
                    ProviderEvent::Finished {
                        reason: FinishReason::Stop,
                        usage: Usage::default(),
                    },
                ]
            };
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}
struct Observed {
    outcome: RunOutcome,
    result: Option<ToolResult>,
    events: Vec<RunEvent>,
    spans: Vec<opentelemetry_sdk::trace::SpanData>,
    native: String,
    calls: usize,
}
async fn run(
    spec: RunSpec,
    name: &str,
    args: Value,
    policy: Policy,
    cancel_at_tool: bool,
) -> Observed {
    run_configured(
        spec,
        name,
        args,
        policy,
        Flags {
            cancel_start: cancel_at_tool,
            ..Flags::default()
        },
    )
    .await
}
#[derive(Default)]
struct Flags {
    writes: bool,
    cancel_start: bool,
    cancel_commit: bool,
}
async fn run_configured(
    spec: RunSpec,
    name: &str,
    args: Value,
    policy: impl Into<PolicySet>,
    flags: Flags,
) -> Observed {
    let provider = RoundTrip::new(name, args);
    let tools =
        ToolRegistry::configured_with_policy_set(false, true, flags.writes, policy.into()).unwrap();
    let exporter = InMemorySpanExporter::default();
    let sdk = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let cancel = CancellationToken::new();
    let mut events = Vec::new();
    let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
    let mut sink = |e: &RunEvent| {
        trace.emit(e)?;
        events.push(e.clone());
        if flags.cancel_start && matches!(e.kind, EventKind::ToolStarted { .. }) {
            cancel.cancel();
        }
        if flags.cancel_commit
            && matches!(&e.kind,EventKind::ToolFinished{result,..} if matches!(result.filesystem.as_deref(),Some(FilesystemResult::Mutation{committed:true,..})))
        {
            cancel.cancel();
        }
        Ok(())
    };
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(&spec, &provider, &tools, &cancel, &mut sink)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::ShellStarted { .. }))
    );
    let result = events.iter().find_map(|e| {
        if let EventKind::ToolFinished { result, .. } = &e.kind {
            Some(result.clone())
        } else {
            None
        }
    });
    if outcome.is_completed() {
        assert_eq!(result, *provider.result.lock().unwrap());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }
    Observed {
        outcome,
        result,
        events,
        spans: exporter.get_finished_spans().unwrap(),
        native: String::from_utf8(trace.into_inner()).unwrap(),
        calls: provider.calls.load(Ordering::SeqCst),
    }
}
async fn call(f: &Fixture, name: &str, args: Value) -> Observed {
    run(f.spec(), name, args, Policy::default(), false).await
}

#[tokio::test]
async fn authority_policy_allowlists_intersect_and_all_deciding_ids_survive() {
    let f = Fixture::new();
    let policy = || {
        let first: Policy = serde_json::from_value(json!({"tools": {
            "default":"allow", "allow":[
                {"id":"first.read","value":"fs.read"},
                {"id":"first.list","value":"fs.list"}
            ]
        }, "read_roots": {"default":"deny", "allow":[{"id":"first.root","value":"."}]}}))
        .unwrap();
        let second: Policy = serde_json::from_value(json!({"tools": {
            "default":"allow", "allow":[
                {"id":"second.read","value":"fs.read"},
                {"id":"second.search","value":"fs.search"}
            ]
        }, "read_roots": {"default":"deny", "allow":[{"id":"second.root","value":"nested"}]}}))
        .unwrap();
        PolicySet::new(Policy::default(), vec![first, second]).unwrap()
    };
    let observed = run_configured(
        f.spec(),
        "fs.read",
        json!({"path":"nested/b.txt"}),
        policy(),
        Flags::default(),
    )
    .await;
    assert!(observed.outcome.is_completed());
    assert_eq!(
        &*observed.result.unwrap().policy_decisions,
        &[
            "builtin.tools.default_allow",
            "first.read",
            "second.read",
            "builtin.read_roots.default_allow",
            "first.root",
            "second.root"
        ]
    );
    for (name, args) in [
        ("fs.list", json!({"path":"nested"})),
        ("fs.search", json!({"path":"nested","pattern":"alpha"})),
    ] {
        let observed = run_configured(f.spec(), name, args, policy(), Flags::default()).await;
        assert!(matches!(observed.outcome, RunOutcome::PolicyDenied { .. }));
        assert_eq!(observed.calls, 1);
        assert!(
            !observed
                .events
                .iter()
                .any(|event| matches!(event.kind, EventKind::ToolStarted { .. }))
        );
    }
    let observed = run_configured(
        f.spec(),
        "fs.read",
        json!({"path":"a.txt"}),
        policy(),
        Flags::default(),
    )
    .await;
    assert!(matches!(observed.outcome, RunOutcome::PolicyDenied { .. }));
    assert!(!observed.native.contains("alpha\\nbeta"));
}

#[tokio::test]
async fn authority_write_denial_survives_ordinary_allow_and_omission_adds_no_constraint() {
    let f = Fixture::new();
    let ceiling: Policy = serde_json::from_value(json!({"write_roots": {
        "default":"allow", "deny":[{"id":"sealed.writes","value":"."}]
    }}))
    .unwrap();
    let args = json!({"path":"new.txt","text":"synthetic mutation","expected_revision":null});
    let observed = run_configured(
        f.spec(),
        "fs.write",
        args.clone(),
        PolicySet::new(Policy::default(), vec![ceiling]).unwrap(),
        Flags {
            writes: true,
            ..Flags::default()
        },
    )
    .await;
    assert!(
        matches!(observed.outcome, RunOutcome::PolicyDenied { rule: PolicyRule::Configured { ref id }} if &**id == "sealed.writes")
    );
    assert!(!f.root.join("new.txt").exists());
    let observed = run_configured(
        f.spec(),
        "fs.write",
        args,
        PolicySet::new(Policy::default(), vec![Policy::default()]).unwrap(),
        Flags {
            writes: true,
            ..Flags::default()
        },
    )
    .await;
    assert!(observed.outcome.is_completed());
    assert_eq!(
        fs::read_to_string(f.root.join("new.txt")).unwrap(),
        "synthetic mutation"
    );
}
fn payload(o: &Observed) -> Value {
    serde_json::to_value(o.result.as_ref().unwrap().filesystem.as_ref().unwrap()).unwrap()
}
fn error(o: &Observed, code: FsError) {
    assert!(o.outcome.is_completed());
    assert_eq!(
        o.result.as_ref().unwrap().status,
        ToolStatus::RecoverableError
    );
    assert_eq!(
        o.result.as_ref().unwrap().filesystem,
        Some(Box::new(FilesystemResult::Error { code }))
    );
}
#[tokio::test]
async fn actual_read_reaches_model_and_correlates_redacted_native_spans() {
    let f = Fixture::new();
    let o = call(&f, "fs.read", json!({"path":"a.txt"})).await;
    let p = payload(&o);
    assert_eq!(p["text"], "alpha\nbeta alpha\n");
    assert_eq!(p["size_bytes"], 17);
    assert_eq!(p["next_offset"], Value::Null);
    assert_eq!(p["truncated"], false);
    // Independent known SHA-256 implementation in system utility, not the tool.
    let hash = std::process::Command::new(if cfg!(target_os = "macos") {
        "shasum"
    } else {
        "sha256sum"
    })
    .args(if cfg!(target_os = "macos") {
        vec!["-a", "256"]
    } else {
        vec![]
    })
    .arg(f.root.join("a.txt"))
    .output()
    .unwrap();
    assert!(hash.status.success());
    assert_eq!(
        p["revision"],
        String::from_utf8(hash.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
    );
    assert!(
        !o.native.contains("a.txt")
            && !o.native.contains("alpha")
            && !o.native.contains(p["revision"].as_str().unwrap())
    );
    let started = o
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
        .unwrap();
    let finished = o
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::ToolFinished { .. }))
        .unwrap();
    let span = o
        .spans
        .iter()
        .find(|s| s.name == "execute_tool fs.read")
        .unwrap();
    assert_eq!(span.span_context.span_id().to_string(), started.span_id);
    assert_eq!(started.span_id, finished.span_id);
    assert_eq!(
        span.start_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros(),
        u128::from(started.timestamp_unix_micros)
    );
    assert_eq!(
        span.end_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros(),
        u128::from(finished.timestamp_unix_micros)
    );
    assert!(!format!("{:?}", o.spans).contains("alpha"));
}
#[tokio::test]
async fn utf8_pages_eof_and_invalid_boundaries() {
    let f = Fixture::new();
    let mut revision = None;
    for (offset, max, text, next) in [
        (0, 4, "Aé", Some(3)),
        (3, 4, "🙂", Some(7)),
        (7, 4, "Z\n", None),
        (9, 4, "", None),
    ] {
        let o = call(
            &f,
            "fs.read",
            json!({"path":"unicode.txt","offset":offset,"max_bytes":max}),
        )
        .await;
        let p = payload(&o);
        assert_eq!(p["text"], text);
        assert_eq!(p["next_offset"], json!(next));
        assert_eq!(p["size_bytes"], 9);
        if let Some(r) = &revision {
            assert_eq!(r, &p["revision"])
        } else {
            revision = Some(p["revision"].clone());
        }
    }
    for (offset, max) in [(2, 4), (3, 1), (10, 4)] {
        error(
            &call(
                &f,
                "fs.read",
                json!({"path":"unicode.txt","offset":offset,"max_bytes":max}),
            )
            .await,
            FsError::InvalidRange,
        );
    }
}
#[tokio::test]
async fn sorted_direct_listing_pages_do_not_resolve_links() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("listing")).unwrap();
    for n in ["b", "a"] {
        fs::write(f.root.join("listing").join(n), "").unwrap();
    }
    fs::create_dir(f.root.join("listing/c")).unwrap();
    symlink(f.base.join("outside"), f.root.join("listing/d")).unwrap();
    for (offset, expected, next) in [
        (0, vec!["listing/a", "listing/b"], Some(2)),
        (2, vec!["listing/c", "listing/d"], None),
        (4, vec![], None),
    ] {
        let p = payload(
            &call(
                &f,
                "fs.list",
                json!({"path":"listing","offset":offset,"max_entries":2}),
            )
            .await,
        );
        assert_eq!(
            p["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["path"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(p["next_offset"], json!(next));
        assert!(!p.to_string().contains("outside"));
    }
    error(
        &call(&f, "fs.list", json!({"path":"listing","offset":5})).await,
        FsError::InvalidRange,
    );
}
#[tokio::test]
async fn recursive_literal_search_has_exact_lines_and_explicit_truncation() {
    let f = Fixture::new();
    let p = payload(&call(&f, "fs.search", json!({"path":".","query":"alpha"})).await);
    assert_eq!(
        p["matches"],
        json!([{"path":".hidden","line":1,"text":"alpha"},{"path":"a.txt","line":1,"text":"alpha"},{"path":"a.txt","line":2,"text":"beta alpha"},{"path":"nested/b.txt","line":1,"text":"alpha"}])
    );
    assert_eq!(p["truncated"], false);
    let limited = payload(
        &call(
            &f,
            "fs.search",
            json!({"path":".","query":"alpha","max_matches":2}),
        )
        .await,
    );
    assert_eq!(limited["matches"].as_array().unwrap().len(), 2);
    assert_eq!(limited["truncated"], true);
    assert_eq!(
        payload(&call(&f, "fs.search", json!({"path":".","query":"a.*"})).await)["matches"],
        json!([])
    );
}
#[tokio::test]
async fn recoverable_errors_and_schema_failures_have_distinct_disposition() {
    let f = Fixture::new();
    for (name, path, code) in [
        ("fs.read", "missing", FsError::NotFound),
        ("fs.read", "nested", FsError::NotFile),
        ("fs.list", "a.txt", FsError::NotDirectory),
        ("fs.read", "binary.bin", FsError::UnsupportedEncoding),
    ] {
        error(&call(&f, name, json!({"path":path})).await, code);
    }
    for (name, args) in [
        ("fs.read", json!({"path":"a.txt","unknown":true})),
        ("fs.read", json!({"path":"a.txt","max_bytes":0})),
        ("fs.read", json!({"path":"a\u{0000}"})),
        ("fs.search", json!({"path":".","query":"a\nb"})),
    ] {
        let o = call(&f, name, args).await;
        assert!(matches!(
            o.outcome,
            RunOutcome::Failed {
                code: FailureCode::InvalidToolArguments,
                ..
            }
        ));
        assert_eq!(o.calls, 1);
    }
}
#[tokio::test]
async fn absolute_parent_and_symlink_paths_cannot_escape() {
    let f = Fixture::new();
    for path in [
        f.base
            .join("outside/outside.txt")
            .to_string_lossy()
            .into_owned(),
        "../outside/outside.txt".into(),
        "internal-link".into(),
        "outside-link/outside.txt".into(),
        format!("{}-other/file", f.root.display()),
    ] {
        let o = call(&f, "fs.read", json!({"path":path})).await;
        assert!(matches!(o.outcome, RunOutcome::PolicyDenied { .. }));
        assert_eq!(o.calls, 1);
        assert!(!o.native.contains("outside sentinel"));
    }
    let o = call(&f, "fs.read", json!({"path":f.root.join("a.txt")})).await;
    assert!(o.outcome.is_completed());
    assert_eq!(
        fs::read_to_string(f.base.join("outside/outside.txt")).unwrap(),
        "outside sentinel\n"
    );
}
#[tokio::test]
async fn default_read_edit_and_search_cross_former_file_and_scan_caps() {
    let f = Fixture::new();
    let mut text = "padding\n".repeat(9 * 1024 * 1024);
    text.push_str("unique needle\n");
    fs::write(f.root.join("large.txt"), &text).unwrap();
    let read = call(&f, "fs.read", json!({"path":"large.txt","max_bytes":8})).await;
    assert!(read.outcome.is_completed(), "{:?}", read.outcome);
    assert_eq!(payload(&read)["text"], "padding\n");
    assert_eq!(payload(&read)["size_bytes"], text.len());
    assert_eq!(payload(&read)["truncated"], true);
    let edit = mutation(
        &f,
        "fs.edit",
        json!({
            "path":"large.txt", "expected_revision":payload(&read)["revision"],
            "old_text":"unique needle", "new_text":"changed needle"
        }),
    )
    .await;
    assert!(edit.outcome.is_completed(), "{:?}", edit.outcome);
    assert_eq!(payload(&edit)["committed"], true);
    text = text.replace("unique needle", "changed needle");
    assert_eq!(fs::read_to_string(f.root.join("large.txt")).unwrap(), text);
    let search = call(
        &f,
        "fs.search",
        json!({"path":".","query":"changed needle"}),
    )
    .await;
    assert!(search.outcome.is_completed(), "{:?}", search.outcome);
    assert_eq!(payload(&search)["matches"][0]["text"], "changed needle");
    assert_eq!(payload(&search)["truncated"], false);
}

#[tokio::test]
async fn default_listing_pages_past_ten_thousand_entries() {
    let f = Fixture::new();
    let directory = f.root.join("wide");
    fs::create_dir(&directory).unwrap();
    for i in 0..10_001 {
        fs::write(directory.join(format!("{i:05}")), "").unwrap();
    }
    let result = call(
        &f,
        "fs.list",
        json!({"path":"wide","offset":10000,"max_entries":1}),
    )
    .await;
    assert!(result.outcome.is_completed(), "{:?}", result.outcome);
    assert_eq!(payload(&result)["entries"][0]["path"], "wide/10000");
    assert_eq!(payload(&result)["truncated"], false);
}

#[tokio::test]
async fn default_search_crosses_thirty_two_levels_in_sorted_depth_first_order() {
    let f = Fixture::new();
    let mut nested = f.root.join("deep");
    for _ in 0..40 {
        nested.push("a");
    }
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("first.txt"), "needle first").unwrap();
    fs::write(f.root.join("deep/z.txt"), "needle last").unwrap();
    let result = call(&f, "fs.search", json!({"path":"deep","query":"needle"})).await;
    assert!(result.outcome.is_completed(), "{:?}", result.outcome);
    assert_eq!(payload(&result)["matches"][0]["text"], "needle first");
    assert_eq!(payload(&result)["matches"][1]["text"], "needle last");
    assert_eq!(payload(&result)["truncated"], false);
}

#[tokio::test]
async fn hard_file_directory_scan_depth_and_serialized_limits_stop_the_run() {
    let f = Fixture::new();
    for kind in 0..5 {
        let mut s = f.spec();
        let (name, args) = match kind {
            0 => {
                s.limits.filesystem.max_file_bytes = Some(16);
                ("fs.read", json!({"path":"a.txt"}))
            }
            1 => {
                s.limits.filesystem.max_entries = Some(2);
                ("fs.list", json!({"path":"."}))
            }
            2 => {
                s.limits.filesystem.max_scan_bytes = Some(1);
                ("fs.search", json!({"path":".","query":"alpha"}))
            }
            3 => {
                fs::create_dir_all(f.root.join("nested/deep/too-deep")).unwrap();
                s.limits.filesystem.max_depth = Some(1);
                ("fs.search", json!({"path":"nested","query":"alpha"}))
            }
            _ => {
                fs::write(f.root.join("long.txt"), "\u{0001}".repeat(2000)).unwrap();
                s.limits.max_tool_output_bytes = 1024;
                ("fs.read", json!({"path":"long.txt"}))
            }
        };
        let o = run(s, name, args, Policy::default(), false).await;
        assert_eq!(
            o.outcome,
            RunOutcome::LimitExceeded {
                limit: if kind == 4 {
                    LimitKind::ToolOutputBytes
                } else {
                    LimitKind::FilesystemWork
                }
            }
        );
        assert_eq!(o.calls, 1);
    }
}
#[tokio::test]
async fn denied_subtrees_are_hidden_and_explicit_reads_denied() {
    let f = Fixture::new();
    let policy = Policy {
        read_roots: Some(Rules {
            default: DefaultDecision::Allow,
            allow: vec![],
            deny: vec![Rule {
                id: "deny.nested".into(),
                value: "nested".into(),
            }],
        }),
        ..Policy::default()
    };
    let o = run(
        f.spec(),
        "fs.search",
        json!({"path":".","query":"alpha"}),
        policy.clone(),
        false,
    )
    .await;
    assert_eq!(payload(&o)["matches"].as_array().unwrap().len(), 3);
    let o = run(
        f.spec(),
        "fs.read",
        json!({"path":"nested/b.txt"}),
        policy,
        false,
    )
    .await;
    assert_eq!(
        o.outcome,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::Configured {
                id: "deny.nested".into()
            }
        }
    );
}
#[tokio::test]
async fn fifo_rejection_does_not_block_and_non_utf8_names_are_not_lossy() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(f.root.join("fifo"))
            .status()
            .unwrap()
            .success()
    );
    let o = tokio::time::timeout(
        Duration::from_secs(2),
        call(&f, "fs.read", json!({"path":"fifo"})),
    )
    .await
    .unwrap();
    error(&o, FsError::NotFile);
    let created = fs::write(f.root.join(std::ffi::OsString::from_vec(vec![255])), "");
    match created {
        Err(error) if cfg!(target_os = "macos") => assert_eq!(error.raw_os_error(), Some(92)),
        result => {
            result.unwrap();
            error(
                &call(&f, "fs.list", json!({"path":"."})).await,
                FsError::UnsupportedEncoding,
            );
        }
    }
}
#[tokio::test]
async fn cancellation_before_worker_effect_finishes_once_and_next_run_is_clean() {
    let f = Fixture::new();
    let o = run(
        f.spec(),
        "fs.search",
        json!({"path":".","query":"alpha"}),
        Policy::default(),
        true,
    )
    .await;
    assert_eq!(o.outcome, RunOutcome::Cancelled);
    assert_eq!(o.result.unwrap().status, ToolStatus::Cancelled);
    assert_eq!(o.calls, 1);
    assert!(
        call(&f, "fs.read", json!({"path":"a.txt"}))
            .await
            .outcome
            .is_completed()
    );
}
#[tokio::test]
async fn capture_is_explicit_and_registry_grants_no_other_capabilities() {
    let f = Fixture::new();
    let mut s = f.spec();
    s.trace.capture_content = true;
    let o = run(
        s,
        "fs.read",
        json!({"path":"a.txt"}),
        Policy::default(),
        false,
    )
    .await;
    assert!(o.native.contains("alpha"));
    assert!(!format!("{:?}", o.spans).contains("alpha"));
    let names = ToolRegistry::with_filesystem_reads()
        .unwrap()
        .descriptors()
        .iter()
        .map(|d| d.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(names, ["fs.read", "fs.list", "fs.search"]);
    assert!(ToolRegistry::default().descriptors().is_empty());
}

#[tokio::test]
async fn unreadable_in_policy_file_is_an_error_not_a_complete_search() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let path = f.root.join("unreadable");
    fs::write(&path, "alpha").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    if fs::File::open(&path).is_ok() {
        // A privileged runner must use a subprocess with dropped identity for
        // permission-denial acceptance; do not pretend chmod denied root.
        panic!("permission fixture needs an unprivileged test runner");
    }
    error(
        &call(&f, "fs.search", json!({"path":".","query":"alpha"})).await,
        FsError::IoError,
    );
}

async fn mutation(f: &Fixture, name: &str, args: Value) -> Observed {
    run_configured(
        f.spec(),
        name,
        args,
        Policy::default(),
        Flags {
            writes: true,
            ..Flags::default()
        },
    )
    .await
}
async fn file_revision(f: &Fixture, path: &str) -> Value {
    payload(&call(f, "fs.read", json!({"path":path})).await)["revision"].clone()
}
fn no_temporaries(f: &Fixture) {
    assert!(!fs::read_dir(&f.root).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pablo-tmp-")
    }));
}
#[tokio::test]
async fn create_only_write_conflicts_without_overwriting_and_keeps_private_mode() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let args = json!({"path":"new.txt","text":"new\n","expected_revision":null});
    let o = mutation(&f, "fs.write", args.clone()).await;
    assert!(o.outcome.is_completed());
    assert_eq!(payload(&o)["created"], true);
    assert_eq!(payload(&o)["committed"], true);
    assert_eq!(payload(&o)["size_bytes"], 4);
    assert_eq!(fs::read_to_string(f.root.join("new.txt")).unwrap(), "new\n");
    assert_eq!(
        fs::metadata(f.root.join("new.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    error(
        &mutation(
            &f,
            "fs.write",
            json!({"path":"new.txt","text":"changed\n","expected_revision":null}),
        )
        .await,
        FsError::Conflict,
    );
    assert_eq!(fs::read_to_string(f.root.join("new.txt")).unwrap(), "new\n");
    no_temporaries(&f);
}
#[tokio::test]
async fn replacement_requires_current_revision_preserves_mode_and_breaks_only_target_link() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let revision = file_revision(&f, "a.txt").await;
    fs::set_permissions(f.root.join("a.txt"), fs::Permissions::from_mode(0o2640)).unwrap();
    fs::hard_link(f.root.join("a.txt"), f.root.join("alias")).unwrap();
    let o = mutation(
        &f,
        "fs.write",
        json!({"path":"a.txt","text":"new\n","expected_revision":revision}),
    )
    .await;
    assert_eq!(payload(&o)["created"], false);
    assert_eq!(
        fs::read_to_string(f.root.join("alias")).unwrap(),
        "alpha\nbeta alpha\n"
    );
    assert_eq!(
        fs::metadata(f.root.join("a.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o640
    );
    error(
        &mutation(
            &f,
            "fs.write",
            json!({"path":"a.txt","text":"stale","expected_revision":revision}),
        )
        .await,
        FsError::Conflict,
    );
    error(
        &mutation(
            &f,
            "fs.write",
            json!({"path":"missing","text":"missing","expected_revision":revision}),
        )
        .await,
        FsError::NotFound,
    );
    assert_eq!(fs::read_to_string(f.root.join("a.txt")).unwrap(), "new\n");
    no_temporaries(&f);
}
#[tokio::test]
async fn edit_requires_one_exact_match_and_preserves_unrelated_bytes() {
    let f = Fixture::new();
    let revision = file_revision(&f, "a.txt").await;
    for old in ["alpha", "absent"] {
        error(
            &mutation(
                &f,
                "fs.edit",
                json!({"path":"a.txt","expected_revision":revision,"old_text":old,"new_text":"x"}),
            )
            .await,
            FsError::MatchNotUnique,
        );
    }
    let invalid = mutation(
        &f,
        "fs.edit",
        json!({"path":"a.txt","expected_revision":revision,"old_text":"","new_text":"x"}),
    )
    .await;
    assert!(matches!(
        invalid.outcome,
        RunOutcome::Failed {
            code: FailureCode::InvalidToolArguments,
            ..
        }
    ));
    let o = mutation(
        &f,
        "fs.edit",
        json!({"path":"a.txt","expected_revision":revision,"old_text":"beta","new_text":"gamma"}),
    )
    .await;
    assert_eq!(payload(&o)["committed"], true);
    assert_eq!(
        fs::read_to_string(f.root.join("a.txt")).unwrap(),
        "alpha\ngamma alpha\n"
    );
    no_temporaries(&f);
}
#[tokio::test]
async fn mutation_capability_policy_parents_and_symlinks_are_enforced() {
    let f = Fixture::new();
    let args = json!({"path":"new.txt","text":"new","expected_revision":null});
    let denied = call(&f, "fs.write", args.clone()).await;
    assert_eq!(
        denied.outcome,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::ToolUnavailable
        }
    );
    assert!(!f.root.join("new.txt").exists());
    let policy = Policy {
        write_roots: Some(Rules {
            default: DefaultDecision::Deny,
            ..Rules::default()
        }),
        ..Policy::default()
    };
    let denied = run_configured(
        f.spec(),
        "fs.write",
        args,
        policy,
        Flags {
            writes: true,
            ..Flags::default()
        },
    )
    .await;
    assert!(matches!(denied.outcome, RunOutcome::PolicyDenied { .. }));
    error(
        &mutation(
            &f,
            "fs.write",
            json!({"path":"absent/new.txt","text":"new","expected_revision":null}),
        )
        .await,
        FsError::NotFound,
    );
    assert!(!f.root.join("absent").exists());
    for path in ["internal-link", "outside-link/new.txt"] {
        assert!(matches!(
            mutation(
                &f,
                "fs.write",
                json!({"path":path,"text":"new","expected_revision":null})
            )
            .await
            .outcome,
            RunOutcome::PolicyDenied {
                rule: PolicyRule::Symlink
            }
        ));
    }
    assert_eq!(
        fs::read_to_string(f.base.join("outside/outside.txt")).unwrap(),
        "outside sentinel\n"
    );
    no_temporaries(&f);
}
#[tokio::test]
async fn cancellation_after_commit_preserves_tool_truth_while_run_is_cancelled() {
    let f = Fixture::new();
    let args = json!({"path":"new.txt","text":"new","expected_revision":null});
    let before = run_configured(
        f.spec(),
        "fs.write",
        args.clone(),
        Policy::default(),
        Flags {
            writes: true,
            cancel_start: true,
            ..Flags::default()
        },
    )
    .await;
    assert_eq!(before.outcome, RunOutcome::Cancelled);
    assert!(!f.root.join("new.txt").exists());
    let after = run_configured(
        f.spec(),
        "fs.write",
        args,
        Policy::default(),
        Flags {
            writes: true,
            cancel_commit: true,
            ..Flags::default()
        },
    )
    .await;
    assert_eq!(after.outcome, RunOutcome::Cancelled);
    assert_eq!(payload(&after)["committed"], true);
    assert_eq!(after.calls, 1);
    assert_eq!(fs::read_to_string(f.root.join("new.txt")).unwrap(), "new");
    no_temporaries(&f);
}
#[tokio::test]
async fn replacement_size_limit_never_truncates_original_and_long_text_is_supported() {
    let f = Fixture::new();
    let revision = file_revision(&f, "a.txt").await;
    let mut s = f.spec();
    s.limits.filesystem.max_file_bytes = Some(17);
    let o = run_configured(
        s,
        "fs.write",
        json!({"path":"a.txt","expected_revision":revision,"text":"x".repeat(18)}),
        Policy::default(),
        Flags {
            writes: true,
            ..Flags::default()
        },
    )
    .await;
    assert_eq!(
        o.outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::FilesystemWork
        }
    );
    assert_eq!(
        fs::read_to_string(f.root.join("a.txt")).unwrap(),
        "alpha\nbeta alpha\n"
    );
    let text = "é".repeat(4000);
    let o = mutation(
        &f,
        "fs.write",
        json!({"path":"a.txt","expected_revision":revision,"text":text}),
    )
    .await;
    assert!(o.outcome.is_completed());
    assert_eq!(fs::read_to_string(f.root.join("a.txt")).unwrap(), text);
    no_temporaries(&f);
}

#[tokio::test]
async fn exact_tool_policy_deny_precedence_allowlist_and_capability_intersection() {
    let f = Fixture::new();
    for (name, rules, expected) in [
        (
            "fs.read",
            json!({"default":"allow","allow":[{"id":"allow.read","value":"fs.read"}],"deny":[{"id":"deny.read","value":"fs.read"}]}),
            "deny.read",
        ),
        (
            "fs.list",
            json!({"default":"allow","allow":[{"id":"allow.read","value":"fs.read"}]}),
            "builtin.tools.allowlist_miss",
        ),
        (
            "fs.read",
            json!({"default":"allow","allow":[{"id":"literal.wildcard","value":"fs.*"}]}),
            "builtin.tools.allowlist_miss",
        ),
    ] {
        let policy: Policy = serde_json::from_value(json!({"tools":rules})).unwrap();
        let o = run(f.spec(), name, json!({"path":"a.txt"}), policy, false).await;
        assert_eq!(
            o.outcome,
            RunOutcome::PolicyDenied {
                rule: PolicyRule::Configured {
                    id: expected.into()
                }
            }
        );
        assert!(o.spans.iter().any(|span| {
            span.attributes.iter().any(|a| {
                a.key.as_str() == "pablo.policy.rule_id" && a.value.to_string() == expected
            })
        }));
        assert_eq!(o.calls, 1);
        assert!(
            !o.events
                .iter()
                .any(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
        );
        assert_eq!(
            o.events
                .last()
                .unwrap()
                .accounting
                .as_ref()
                .unwrap()
                .tool_calls,
            0
        );
        assert_eq!(
            fs::read_to_string(f.root.join("a.txt")).unwrap(),
            "alpha\nbeta alpha\n"
        );
    }
    let policy: Policy = serde_json::from_value(
        json!({"tools":{"default":"deny","allow":[{"id":"allow.write","value":"fs.write"}]}}),
    )
    .unwrap();
    let o = run(
        f.spec(),
        "fs.write",
        json!({"path":"a.txt","text":"bad","expected_revision":null}),
        policy,
        false,
    )
    .await;
    assert_eq!(
        o.outcome,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::ToolUnavailable
        }
    );
}

#[tokio::test]
async fn allowed_tool_rules_are_bounded_and_observable_with_root_rules() {
    let f = Fixture::new();
    let policy: Policy = serde_json::from_value(
        json!({"tools":{"default":"deny","allow":[{"id":"allow.read","value":"fs.read"}]}}),
    )
    .unwrap();
    let mut spec = f.spec();
    spec.limits.max_tool_output_bytes = 1024;
    let o = run(
        spec,
        "fs.read",
        json!({"path":"a.txt","max_output_bytes":1024}),
        policy,
        false,
    )
    .await;
    assert!(o.outcome.is_completed());
    let result = o.result.unwrap();
    assert_eq!(
        &*result.policy_decisions,
        ["allow.read", "builtin.read_roots.default_allow"]
    );
    assert!(serde_json::to_vec(&result).unwrap().len() <= 1024);
    assert!(o.native.contains("allow.read"));
    assert!(o.spans.iter().any(|span| span.attributes.iter().any(|a| {
        a.key.as_str() == "pablo.policy.rule_ids"
            && a.value
                .to_string()
                .contains("builtin.read_roots.default_allow")
    })));
    assert!(o.spans.iter().any(|span| {
        span.attributes.iter().any(|a| {
            a.key.as_str() == "pablo.policy.rule_ids" && a.value.to_string().contains("allow.read")
        })
    }));
}
