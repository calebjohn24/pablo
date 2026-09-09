#![cfg(any(target_os = "macos", target_os = "linux"))]
use futures_util::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use pablo_core::{
    policy::{Policy, PolicySet},
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    shell_policy::{ShellRestriction, ShellSettings},
    tool::ToolResult,
    *,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pablo-q01-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Loop {
    args: Value,
    calls: AtomicUsize,
}
impl Provider for Loop {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![
                    ProviderEvent::ToolCallStart {
                        id: "call".into(),
                        name: "shell.run".into(),
                    },
                    ProviderEvent::ToolCallArgumentsDelta {
                        id: "call".into(),
                        delta: self.args.to_string(),
                    },
                    ProviderEvent::Finished {
                        reason: FinishReason::ToolCalls,
                        usage: Usage::default(),
                    },
                ]
            } else {
                let Some(Message::Tool { result, .. }) = request.messages.last() else {
                    panic!("missing tool evidence")
                };
                vec![
                    ProviderEvent::TextDelta(result.shell.as_ref().unwrap().stdout.clone()),
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
struct Observation {
    outcome: RunOutcome,
    events: Vec<RunEvent>,
    native: String,
    calls: usize,
}
impl Observation {
    fn result(&self) -> &ToolResult {
        self.events
            .iter()
            .find_map(|e| match &e.kind {
                EventKind::ToolFinished { result, .. } => Some(result),
                _ => None,
            })
            .unwrap()
    }
    fn allowed(&self, stdout: &str, ids: &[&str]) {
        assert!(self.outcome.is_completed(), "{:?}", self.outcome);
        assert_eq!(self.result().shell.as_ref().unwrap().stdout, stdout);
        assert_eq!(self.calls, 2);
        assert_eq!(
            self.events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::ShellStarted { .. }))
                .count(),
            1
        );
        for id in ids {
            assert!(
                self.result().policy_decisions.iter().any(|s| s == id),
                "{id}: {:?}",
                self.result()
            );
        }
    }
    fn denied(&self, id: &str) {
        assert_eq!(
            self.outcome,
            RunOutcome::PolicyDenied {
                rule: PolicyRule::Configured { id: id.into() }
            }
        );
        assert_eq!(self.calls, 1);
        assert!(
            !self
                .events
                .iter()
                .any(|e| matches!(e.kind, EventKind::ShellStarted { .. }))
        );
        assert!(self.native.contains(id));
    }
}
fn registry(settings: Value, ceilings: Vec<Value>) -> ToolRegistry {
    ToolRegistry::with_shell()
        .unwrap()
        .with_shell_configuration(
            serde_json::from_value::<ShellSettings>(settings).unwrap(),
            ceilings
                .into_iter()
                .map(|v| serde_json::from_value::<ShellRestriction>(v).unwrap())
                .collect(),
        )
        .unwrap()
}
async fn run(f: &Fixture, args: Value, tools: &ToolRegistry, cancel_on_start: bool) -> Observation {
    let exporter = InMemorySpanExporter::default();
    let sdk = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let spec = RunSpec::new("private task sentinel", f.0.clone(), "fixture/q01");
    let provider = Loop {
        args,
        calls: AtomicUsize::new(0),
    };
    let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
    let mut events = Vec::new();
    let cancel = CancellationToken::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(&spec, &provider, tools, &cancel, &mut |event: &RunEvent| {
            if cancel_on_start && matches!(event.kind, EventKind::ShellStarted { .. }) {
                cancel.cancel();
            }
            trace.emit(event)?;
            events.push(event.clone());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
    let native = String::from_utf8(trace.into_inner()).unwrap();
    assert!(!native.contains("private task sentinel"));
    assert!(!native.contains(f.0.to_str().unwrap()));
    let spans = format!("{:?}", exporter.get_finished_spans().unwrap());
    assert!(!spans.contains("private task sentinel"));
    assert!(!spans.contains(f.0.to_str().unwrap()));
    Observation {
        outcome,
        events,
        native,
        calls: provider.calls.load(Ordering::SeqCst),
    }
}
fn rule(id: &str, exe: &str, args: &[&str], mode: &str) -> Value {
    json!({"id":id,"executable":exe,"args":args,"match":mode})
}
fn commands(default: &str, allow: Vec<Value>, deny: Vec<Value>) -> Value {
    json!({"commands":{"default":default,"allow":allow,"deny":deny}})
}
async fn command(f: &Fixture, text: &str, tools: &ToolRegistry) -> Observation {
    run(f, json!({"command":text,"cwd":"."}), tools, false).await
}

#[tokio::test]
async fn git_modes_defaults_and_deny_precedence_use_exact_arguments() {
    let f = Fixture::new();
    assert!(
        std::process::Command::new("/usr/bin/git")
            .args(["init", "--quiet"])
            .arg(&f.0)
            .status()
            .unwrap()
            .success()
    );
    let status = rule(
        "allow.status",
        "/usr/bin/git",
        &["status", "--short"],
        "exact",
    );
    let push = rule("deny.push", "/usr/bin/git", &["push"], "prefix");
    for default in ["allow", "deny"] {
        let tools = registry(commands(default, vec![status.clone()], vec![]), vec![]);
        command(&f, "git status --short", &tools)
            .await
            .allowed("", &["allow.status"]);
        for text in [
            "git 'status --short'",
            "git status --short extra",
            "git status",
            "git push",
        ] {
            command(&f, text, &tools)
                .await
                .denied("builtin.shell.commands.allowlist_miss");
        }
    }
    let tools = registry(commands("allow", vec![], vec![push.clone()]), vec![]);
    command(&f, "git status --short", &tools)
        .await
        .allowed("", &["builtin.shell.commands.default_allow"]);
    for text in [
        "git push",
        "git push origin main",
        "/usr/bin/git 'push' --dry-run",
    ] {
        command(&f, text, &tools).await.denied("deny.push");
    }
    let tools = registry(
        commands(
            "deny",
            vec![status.clone()],
            vec![
                rule(
                    "deny.status",
                    "/usr/bin/git",
                    &["status", "--short"],
                    "exact",
                ),
                push,
            ],
        ),
        vec![],
    );
    command(&f, "git status --short", &tools)
        .await
        .denied("deny.status");
    let tools = registry(commands("deny", vec![], vec![]), vec![]);
    command(&f, "git status --short", &tools)
        .await
        .denied("builtin.shell.commands.default_deny");
}

#[tokio::test]
async fn quoting_literal_metacharacters_and_aliases_execute_the_checked_argv() {
    let f = Fixture::new();
    let exe = f.script("program=literal", "printf '<%s>' \"$@\"");
    let alias = f.0.join("symbolic");
    let hard = f.0.join("hard");
    std::os::unix::fs::symlink(&exe, &alias).unwrap();
    fs::hard_link(&exe, &hard).unwrap();
    let args = ["a b", "", "ab'cd", "$HOME;*", "c d"];
    let tools = registry(
        commands(
            "deny",
            vec![rule("literal", exe.to_str().unwrap(), &args, "exact")],
            vec![],
        ),
        vec![],
    );
    for path in [&exe, &alias, &hard] {
        let text = format!("'{}' a\\ b '' \"ab'cd\" '$HOME;*' c\" d\"", path.display());
        let result = command(&f, &text, &tools).await;
        result.allowed("<a b><><ab'cd><$HOME;*><c d>", &["literal"]);
        assert!(!result.native.contains("$HOME"));
    }
    let tools = registry(
        commands(
            "allow",
            vec![],
            vec![rule("deny.alias", alias.to_str().unwrap(), &[], "prefix")],
        ),
        vec![],
    );
    for path in [&exe, &alias, &hard] {
        command(&f, &format!("'{}' arbitrary", path.display()), &tools)
            .await
            .denied("deny.alias");
    }
    let tools = registry(
        commands(
            "deny",
            vec![rule("prefix", "/usr/bin/printf", &["%s", "ok"], "prefix")],
            vec![],
        ),
        vec![],
    );
    command(&f, "printf %s ok next", &tools)
        .await
        .allowed("oknext", &["prefix"]);
    command(&f, "printf %s okay", &tools)
        .await
        .denied("builtin.shell.commands.allowlist_miss");
    command(&f, "printf '%s ok'", &tools)
        .await
        .denied("builtin.shell.commands.allowlist_miss");
}

#[tokio::test]
async fn evaluation_attempts_reject_before_launch_and_legacy_still_interprets_shell() {
    let f = Fixture::new();
    let tools = registry(commands("allow", vec![], vec![]), vec![]);
    for text in [
        "printf x; touch marker",
        "printf x | touch marker",
        "printf x && touch marker",
        "printf x & touch marker",
        "printf $(touch marker)",
        "printf `touch marker`",
        "printf x >marker",
        "printf x <marker",
        "(touch marker)",
        "printf $HOME",
        "printf \"$HOME\"",
        "printf *",
        "printf ?",
        "printf [x]",
        "printf {x,y}",
        "printf ~",
        "printf x #comment",
        "printf x\ntouch marker",
        "printf 'unclosed",
        "printf \\",
    ] {
        command(&f, text, &tools)
            .await
            .denied("builtin.shell.commands.syntax");
        assert!(!f.0.join("marker").exists());
    }
    for text in [
        "PABLO_TASK_X=a printf x",
        "./program x",
        "/missing/private-command x",
        "'' x",
    ] {
        let result = command(&f, text, &tools).await;
        result.denied("builtin.shell.commands.executable");
        assert!(!result.native.contains("private-command"));
    }
    command(
        &f,
        "printf $(printf legacy) > marker; cat marker",
        &ToolRegistry::with_shell().unwrap(),
    )
    .await
    .allowed("legacy", &[]);
    assert_eq!(fs::read_to_string(f.0.join("marker")).unwrap(), "legacy");
}

#[tokio::test]
async fn authority_dimensions_intersect_and_launcher_policy_remains_independent() {
    let f = Fixture::new();
    let ordinary = commands(
        "deny",
        vec![rule("task.printf", "/usr/bin/printf", &["%s"], "prefix")],
        vec![],
    );
    let root = commands(
        "deny",
        vec![rule(
            "root.printf",
            "/usr/bin/printf",
            &["%s", "root"],
            "prefix",
        )],
        vec![],
    );
    let child = commands(
        "deny",
        vec![rule(
            "child.printf",
            "/usr/bin/printf",
            &["%s", "root", "child"],
            "exact",
        )],
        vec![],
    );
    let tools = registry(ordinary.clone(), vec![root.clone(), child]);
    command(&f, "printf %s root child", &tools).await.allowed(
        "rootchild",
        &[
            "task.printf",
            "root.printf",
            "child.printf",
            "builtin.tools.default_allow",
            "builtin.executables.default_allow",
        ],
    );
    let denied = command(&f, "printf %s root", &tools).await;
    denied.denied("builtin.shell.commands.allowlist_miss");
    assert!(
        denied
            .result()
            .policy_decisions
            .iter()
            .any(|s| s == "root.printf")
    );
    let tools = registry(json!({}), vec![root]);
    command(&f, "printf arbitrary", &tools)
        .await
        .denied("builtin.shell.commands.allowlist_miss");
    let launcher:Policy=serde_json::from_value(json!({"executables":{"default":"allow","deny":[{"id":"launcher.denied","value":"/bin/sh"}]}})).unwrap();
    let tools = ToolRegistry::configured_with_policy_set(
        true,
        false,
        false,
        PolicySet::new(Policy::default(), vec![launcher]).unwrap(),
    )
    .unwrap()
    .with_shell_configuration(serde_json::from_value(ordinary).unwrap(), vec![])
    .unwrap();
    command(&f, "printf %s root child", &tools)
        .await
        .denied("launcher.denied");
}

#[tokio::test]
async fn environment_defaults_names_byte_bounds_and_canonical_cwd_are_enforced() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("allowed/private")).unwrap();
    fs::create_dir(f.0.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(f.0.join("elsewhere"), f.0.join("allowed/escape")).unwrap();
    let exe = f.script(
        "env",
        "printf '%s/%s/%s' \"$PABLO_TASK_VALUE\" \"$PATH\" \"${AI_GATEWAY_API_KEY-unset}\"",
    );
    let mut settings = commands(
        "deny",
        vec![rule("env.script", exe.to_str().unwrap(), &[], "exact")],
        vec![],
    );
    settings["environment"] = json!({"values":{"PABLO_TASK_VALUE":"default"},"allowed_names":["PABLO_TASK_VALUE","PABLO_TASK_OTHER"]});
    settings["cwd_roots"] =
        json!({"default":"deny","allow":[{"id":"cwd.allowed","value":"allowed"}]});
    let ceiling = json!({"environment":{"allowed_names":["PABLO_TASK_VALUE"]},"cwd_roots":{"default":"allow","deny":[{"id":"cwd.private","value":"allowed/private"}]}});
    let direct = registry(settings.clone(), vec![ceiling.clone()]);
    let mut request = deployment::ResolveRequest::new(f.0.clone(), "unused.toml");
    request
        .path_bindings
        .insert("workspace".into(), f.0.clone());
    request.entry = deployment::ConfigInput::Document(json!({
        "schema_version":1,
        "credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"environment","name":"UNREAD_SYNTHETIC_KEY"}]}},
        "options":{"shell":settings}, "authority":[{"id":"root","shell":ceiling}]
    }));
    let resolved = deployment::resolve(request).unwrap();
    let prepared = resolved
        .prepare_run(deployment::RunInput {
            input: "synthetic".into(),
            workspace: Some(f.0.clone()),
            session_id: None,
        })
        .unwrap();
    let tools = prepared.tools().unwrap();
    let base = json!({"command":format!("'{}'",exe.display()),"cwd":"allowed"});
    run(&f, base.clone(), &direct, false).await.allowed(
        "default//usr/bin:/bin/unset",
        &["env.script", "cwd.allowed"],
    );
    run(&f, base.clone(), &tools, false).await.allowed(
        "default//usr/bin:/bin/unset",
        &["env.script", "cwd.allowed"],
    );
    let mut args = base.clone();
    args["env"] = json!({"PABLO_TASK_VALUE":"task-private-sentinel"});
    let result = run(&f, args, &tools, false).await;
    result.allowed("task-private-sentinel//usr/bin:/bin/unset", &[]);
    assert!(!result.native.contains("task-private-sentinel"));
    for (cwd, id) in [
        ("allowed/private", "cwd.private"),
        ("allowed/escape", "builtin.shell.cwd_roots.allowlist_miss"),
        (
            "allowed/../elsewhere",
            "builtin.shell.cwd_roots.allowlist_miss",
        ),
        (".", "builtin.shell.cwd_roots.allowlist_miss"),
    ] {
        let mut args = base.clone();
        args["cwd"] = json!(cwd);
        run(&f, args, &tools, false).await.denied(id);
    }
    let mut args = base.clone();
    args["env"] = json!({"PABLO_TASK_OTHER":"x"});
    run(&f, args, &tools, false)
        .await
        .denied("builtin.shell.environment.names");
    let mut args = base.clone();
    args["env"] = json!({"PABLO_TASK_VALUE":"é".repeat(4097)});
    run(&f, args, &tools, false)
        .await
        .denied("builtin.shell.environment.value");
    let mut args = base.clone();
    args["env"] = json!({"AI_GATEWAY_API_KEY":"private-key"});
    let result = run(&f, args, &tools, false).await;
    assert_eq!(
        result.outcome,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::Environment
        }
    );
    assert!(!result.native.contains("private-key"));
    let mut args = base;
    args["cwd"] = json!(std::env::temp_dir());
    assert_eq!(
        run(&f, args, &tools, false).await.outcome,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::Workspace
        }
    );
}

#[tokio::test]
async fn restricted_cancellation_reaps_process_before_terminal_and_next_run_is_clean() {
    let f = Fixture::new();
    let tools = registry(commands("allow", vec![], vec![]), vec![]);
    let result = run(&f, json!({"command":"sleep 60","cwd":"."}), &tools, true).await;
    assert_eq!(result.outcome, RunOutcome::Cancelled);
    for event in &result.events {
        if let EventKind::ShellStarted { process_id, .. } = event.kind {
            assert!(
                rustix::process::test_kill_process(
                    rustix::process::Pid::from_raw(process_id as i32).unwrap()
                )
                .is_err()
            );
        }
    }
    command(&f, "printf clean", &tools)
        .await
        .allowed("clean", &[]);
}

#[tokio::test]
async fn allowed_program_descendants_are_joined_on_restricted_cancellation() {
    let f = Fixture::new();
    let executable=f.script("children", "trap '' TERM; /bin/sh -c 'trap \"\" TERM; while :; do sleep 1; done' & echo $! > child.pid; wait");
    let tools = registry(
        commands(
            "deny",
            vec![rule("children", executable.to_str().unwrap(), &[], "exact")],
            vec![],
        ),
        vec![],
    );
    let provider = Loop {
        args: json!({"command":format!("'{}'",executable.display()),"cwd":"."}),
        calls: AtomicUsize::new(0),
    };
    let sdk = SdkTracerProvider::builder().build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let cancel = CancellationToken::new();
    let spec = RunSpec::new("synthetic cleanup", f.0.clone(), "fixture/q01");
    let mut events = Vec::new();
    let mut sink = |e: &RunEvent| {
        events.push(e.clone());
        Ok(())
    };
    let cancel_task = async {
        let pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(text) = fs::read_to_string(f.0.join("child.pid"))
                    && let Ok(pid) = text.trim().parse::<i32>()
                {
                    break pid;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        pid
    };
    let (outcome, pid) = tokio::join!(
        runtime.run_with_tools(&spec, &provider, &tools, &cancel, &mut sink),
        cancel_task
    );
    assert_eq!(outcome.unwrap(), RunOutcome::Cancelled);
    assert!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()).is_err()
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
    command(
        &f,
        "printf clean",
        &registry(commands("allow", vec![], vec![]), vec![]),
    )
    .await
    .allowed("clean", &[]);
}
