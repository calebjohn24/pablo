#![cfg(unix)]
use pablo_core::{
    children::SpawnRequest,
    deployment::{
        self, ConfigInput, CredentialInputs, CredentialReadError, PreparedRun, ResolveRequest,
        RunInput,
    },
    provider::ProviderEvent,
    *,
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Mutex, time::Duration};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("pablo-child-admission-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn prepared(&self, mut document: Value) -> PreparedRun {
        document["schema_version"] = 1.into();
        if document.get("credentials").is_none() {
            document["credentials"] = json!({"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"gateway"}]}});
        }
        let mut request = ResolveRequest::new(self.0.clone(), "unused");
        request.entry = ConfigInput::Document(document);
        request
            .path_bindings
            .insert("workspace".into(), self.0.clone());
        deployment::resolve(request)
            .unwrap()
            .prepare_run(RunInput {
                input: "PARENT_PRIVATE_HISTORY".into(),
                session_id: Some("parent-session".into()),
                workspace: None,
            })
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn spawn(mut value: Value) -> SpawnRequest {
    if value.get("input").is_none() {
        value["input"] = "selected child task".into();
    }
    serde_json::from_value(value).unwrap()
}
#[derive(Default)]
struct Credentials(Mutex<Vec<String>>);
impl CredentialInputs for Credentials {
    fn environment(&self, _: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        panic!("no ambient credentials")
    }
    fn host(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        self.0.lock().unwrap().push(name.into());
        Ok(Some(format!("synthetic-{name}").into_bytes()))
    }
}

#[test]
fn child_projection_preserves_authority_but_only_contains_selected_task_context() {
    let f = Fixture::new();
    let parent = f.prepared(json!({"options":{"run":{"instructions":"TRUSTED_SHARED_PREFIX"},"filesystem":{"write":true},"trace":{"path":{"base":"workspace","path":"trace.jsonl"}}}}));
    let original = serde_json::to_value(parent.spec()).unwrap();
    let parent_tools = parent.tools().unwrap();
    let request = spawn(
        json!({"overlay":"ordinary child request","context":["selected fact"],"capabilities":{"tools":["fs.read","fs.search"]},"ceilings":{"max_model_calls":2}}),
    );
    let child = parent.prepare_child(&request, &parent_tools).unwrap();
    assert_eq!(child.spec().instructions, "TRUSTED_SHARED_PREFIX");
    assert_eq!(child.spec().workspace, parent.spec().workspace);
    assert_eq!(child.spec().session_id, None);
    assert!(!child.spec().input.contains("PARENT_PRIVATE_HISTORY"));
    let input: Value = serde_json::from_str(&child.spec().input).unwrap();
    assert_eq!(input["task"], request.input);
    assert_eq!(input["overlay"], request.overlay.as_ref().unwrap().as_str());
    assert_eq!(input["context"], json!(["selected fact"]));
    assert_eq!(child.spec().limits.max_model_calls, Some(2));
    assert_eq!(child.spec().limits.filesystem.max_scan_bytes, None);
    assert!(child.trace_path().is_none());
    assert!(child.create_trace_file().unwrap().is_none());
    assert!(!f.0.join("trace.jsonl").exists());
    assert_eq!(
        child
            .tools()
            .unwrap()
            .descriptors()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["fs.read", "fs.search"]
    );
    assert_ne!(child.bindings_fingerprint(), parent.bindings_fingerprint());
    assert_eq!(serde_json::to_value(parent.spec()).unwrap(), original);
    assert!(child.prepare_child(&request, &parent_tools).is_err());
    let empty = parent
        .prepare_child(
            &spawn(json!({"capabilities":{"tools":[],"skills":[],"mcp_servers":[]}})),
            &parent_tools,
        )
        .unwrap();
    assert!(empty.tools().unwrap().descriptors().is_empty());
    assert_ne!(empty.bindings_fingerprint(), child.bindings_fingerprint());
}

#[tokio::test]
async fn removed_native_tool_cannot_execute_even_when_the_model_guesses_its_name() {
    let f = Fixture::new();
    let parent = f.prepared(json!({"options":{"filesystem":{"write":true}}}));
    let child = parent
        .prepare_child(
            &spawn(json!({"capabilities":{"tools":["fs.read"]}})),
            &parent.tools().unwrap(),
        )
        .unwrap();
    let provider = ScriptedProvider::new(vec![
        (
            Duration::ZERO,
            Ok(ProviderEvent::ToolCallStart {
                id: "write".into(),
                name: "fs.write".into(),
            }),
        ),
        (
            Duration::ZERO,
            Ok(ProviderEvent::ToolCallArgumentsDelta {
                id: "write".into(),
                delta: json!({"path":"forbidden.txt","text":"must not write","mode":"create"})
                    .to_string(),
            }),
        ),
        (
            Duration::ZERO,
            Ok(ProviderEvent::Finished {
                reason: FinishReason::ToolCalls,
                usage: Usage::default(),
            }),
        ),
    ]);
    let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let result = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            child.spec(),
            &provider,
            &child.tools().unwrap(),
            &CancellationToken::new(),
            &mut |_: &RunEvent| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::ToolUnavailable
        }
    );
    assert!(!f.0.join("forbidden.txt").exists());
}

#[test]
fn denied_unknown_and_escaped_selections_fail_before_any_factory_runs() {
    let f = Fixture::new();
    let parent = f.prepared(json!({"options":{"filesystem":{"write":true}},"authority":[{"id":"host","policy":{"tools":{"default":"allow","deny":[{"id":"host.no-write","value":"fs.write"}]}}}]}));
    let tools = parent.tools().unwrap();
    for request in [
        json!({"capabilities":{"tools":["fs.write"]}}),
        json!({"capabilities":{"tools":["invented"]}}),
        json!({"capabilities":{"mcp_servers":["invented"]}}),
        json!({"capabilities":{"skills":["ambient/secret"]}}),
        json!({"capabilities":{"model_route":["invented"]}}),
        json!({"ceilings":{"max_context_bytes":8388609}}),
        json!({"output_schema":{"$ref":"https://never-fetch.invalid/schema"}}),
        json!({"input":"\"".repeat(600_000),"overlay":"forces serialized attachment framing"}),
    ] {
        assert!(parent.prepare_child(&spawn(request), &tools).is_err());
    }
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 0);
}

#[test]
fn child_route_and_credential_resolution_use_only_exact_inherited_entries() {
    let f = Fixture::new();
    let parent = f.prepared(json!({"options":{
        "models":{"first":{"provider":"vercel","id":"zai/glm-5.3-flash","credential":"a"},"second":{"provider":"openrouter","id":"z-ai/glm-5.3-flash","credential":"b"}},
        "routes":{"main":{"entries":[{"model":"first"},{"model":"second"}]}},"model_route":"main"
    },"credentials":{"a":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"first"}]},"b":{"consumer":"provider.openrouter","sources":[{"kind":"host","name":"second"}]}}}));
    let tools = parent.tools().unwrap();
    let child = parent
        .prepare_child(
            &spawn(json!({"capabilities":{"model_route":["second"]}})),
            &tools,
        )
        .unwrap();
    assert_eq!(
        child.model_route().unwrap().entries(),
        &parent.model_route().unwrap().entries()[1..]
    );
    assert_eq!(child.model_route().unwrap().policy().max_attempts, 1);
    assert_eq!(child.spec().model, "z-ai/glm-5.3-flash");
    assert_eq!(
        child.model_profile().unwrap(),
        *parent.model_route().unwrap().entries()[1].profile()
    );
    let credentials = Credentials::default();
    assert!(child.route_credential(0, &credentials).is_ok());
    assert!(child.route_credential(1, &credentials).is_err());
    assert!(
        child
            .credential(deployment::CredentialConsumer::OpenRouter, &credentials)
            .is_ok()
    );
    assert_eq!(*credentials.0.lock().unwrap(), ["second", "second"]);
    for names in [
        json!([]),
        json!(["second", "first"]),
        json!(["second", "second"]),
        json!(["third"]),
    ] {
        assert!(
            parent
                .prepare_child(
                    &spawn(json!({"capabilities":{"model_route":names}})),
                    &tools
                )
                .is_err()
        );
    }
}

#[test]
fn child_mcp_client_selection_cannot_restore_an_excluded_host_server() {
    let f = Fixture::new();
    let mut parent = f.prepared(json!({"options":{"mcp":{"servers":{
        "one":{"transport":"http","url":"https://one.example.test/mcp"},
        "two":{"transport":"http","url":"https://two.example.test/mcp"}
    }}}}));
    let selection = |name: &str| mcp::ClientServer {
        name: name.into(),
        transport: mcp::ClientTransport::Http {
            url: format!("https://{name}.example.test/mcp"),
            headers: Default::default(),
        },
    };
    parent.select_mcp_client(&[selection("one")]).unwrap();
    let native = ToolRegistry::configured(true, true, Default::default()).unwrap();
    assert!(
        parent
            .prepare_child(
                &spawn(json!({"capabilities":{"mcp_servers":["two"]}})),
                &native
            )
            .is_err()
    );
    let mut child = parent.prepare_child(&spawn(json!({})), &native).unwrap();
    assert!(child.select_mcp_client(&[selection("two")]).is_err());
    child.select_mcp_client(&[]).unwrap();
    assert!(child.select_mcp_client(&[selection("two")]).is_err());
    child.select_mcp_client(&[selection("one")]).unwrap();
    assert!(child.has_mcp());
}

#[tokio::test]
async fn child_can_activate_an_approved_skill_without_loading_it_into_the_parent() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("skills/read")).unwrap();
    fs::write(
        f.0.join("skills/read/SKILL.md"),
        "---\nname: read\ndescription: Read selected evidence.\n---\nCHILD_ONLY_INSTRUCTIONS\n",
    )
    .unwrap();
    let parent = f.prepared(
        json!({"options":{"skills":{"roots":{"host":{"base":"config","path":"skills"}}}}}),
    );
    fs::write(f.0.join("skills/read/secret.txt"), "UNSELECTED_RESOURCE").unwrap();
    let parent_tools = parent.tools().unwrap();
    assert!(!parent.has_skills());
    assert!(parent_tools.activated_skills().is_none());
    let child = parent
        .prepare_child(
            &spawn(json!({"capabilities":{"skills":["host/read"],"tools":[]},"ceilings":{"max_model_calls":2}})),
            &parent_tools,
        )
        .unwrap();
    let credentials = Credentials::default();
    let tools = child
        .tools_with_capabilities(
            &credentials,
            tokio::time::Instant::now() + Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(tools.descriptors().is_empty());
    assert!(
        tools.activated_skills().unwrap().instructions()[0]
            .1
            .contains("CHILD_ONLY_INSTRUCTIONS")
    );
    assert!(parent_tools.activated_skills().is_none());
    assert!(credentials.0.lock().unwrap().is_empty());
    let provider = ScriptedProvider::new(vec![
        (
            Duration::ZERO,
            Ok(ProviderEvent::ToolCallStart {
                id: "read".into(),
                name: "skill.read".into(),
            }),
        ),
        (
            Duration::ZERO,
            Ok(ProviderEvent::ToolCallArgumentsDelta {
                id: "read".into(),
                delta: json!({"skill":"host/read","path":"secret.txt"}).to_string(),
            }),
        ),
        (
            Duration::ZERO,
            Ok(ProviderEvent::Finished {
                reason: FinishReason::ToolCalls,
                usage: Usage::default(),
            }),
        ),
    ]);
    let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let result = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            child.spec(),
            &provider,
            &tools,
            &CancellationToken::new(),
            &mut |_: &RunEvent| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        RunOutcome::PolicyDenied {
            rule: PolicyRule::ToolUnavailable
        }
    );
    tools.close().await.unwrap();
    for name in ["read", "host/../read", "other/read"] {
        assert!(
            parent
                .prepare_child(
                    &spawn(json!({"capabilities":{"skills":[name]}})),
                    &parent_tools
                )
                .is_err()
        );
    }
    let restricted = f.prepared(
        json!({"options":{"skills":{"roots":{"host":{"base":"config","path":"skills"}}}},
        "authority":[{"id":"root","tool_names":["shell.run","fs.read","fs.list","fs.search"]}]}),
    );
    assert!(
        restricted
            .prepare_child(
                &spawn(json!({"capabilities":{"skills":["host/read"],"tools":["skill.read"]}})),
                &restricted.tools().unwrap()
            )
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires the isolated .pablo/mcp-fixture-venv/bin/python fixture interpreter"]
async fn child_mcp_catalogs_are_fresh_narrowed_and_joined_on_definition_change() {
    let f = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let definition = json!({"transport":"stdio","command":root.join(".pablo/mcp-fixture-venv/bin/python"),
        "args":[root.join("tests/fixtures/mcp/adversarial.py"),"child_catalog"],"required":false});
    let parent = f.prepared(json!({"options":{"shell":{"enabled":false},"filesystem":{"enabled":false},"mcp":{"servers":{"local":definition}}}}));
    fs::write(f.0.join("catalog-version"), "original").unwrap();
    let credentials = Credentials::default();
    let cancellation = CancellationToken::new();
    let deadline = || tokio::time::Instant::now() + Duration::from_secs(10);
    let parent_tools = parent
        .tools_with_capabilities(&credentials, deadline(), &cancellation)
        .await
        .unwrap();
    assert_eq!(parent_tools.descriptors().len(), 1);
    let pid = || {
        fs::read_to_string(f.0.join("pid"))
            .unwrap()
            .parse::<i32>()
            .unwrap()
    };
    let root_pid = pid();
    let child = parent
        .prepare_child(
            &spawn(json!({"capabilities":{"tools":["mcp/local/read"]}})),
            &parent_tools,
        )
        .unwrap();
    let removed = parent
        .prepare_child(
            &spawn(json!({"capabilities":{"mcp_servers":[]}})),
            &parent_tools,
        )
        .unwrap();
    assert!(!removed.has_mcp());
    assert!(removed.tools().unwrap().descriptors().is_empty());
    assert!(
        parent
            .prepare_child(
                &spawn(json!({"capabilities":{"mcp_servers":[],"tools":["mcp/local/read"]}})),
                &parent_tools
            )
            .is_err()
    );
    for variant in ["original", "extra", "changed", "removed"] {
        fs::write(f.0.join("catalog-version"), variant).unwrap();
        let result = child
            .tools_with_capabilities(&credentials, deadline(), &cancellation)
            .await;
        let child_pid = pid();
        assert_ne!(child_pid, root_pid);
        if matches!(variant, "changed" | "removed") {
            assert_eq!(
                result.err().unwrap().code,
                "config_child_capability_changed"
            );
        } else {
            let tools = result.unwrap();
            assert_eq!(
                tools
                    .descriptors()
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                ["mcp/local/read"]
            );
            tools.close().await.unwrap();
        }
        assert_eq!(
            rustix::process::test_kill_process_group(
                rustix::process::Pid::from_raw(child_pid).unwrap()
            ),
            Err(rustix::io::Errno::SRCH)
        );
        assert!(
            rustix::process::test_kill_process_group(
                rustix::process::Pid::from_raw(root_pid).unwrap()
            )
            .is_ok()
        );
    }
    assert!(!f.0.join("called").exists());
    parent_tools.close().await.unwrap();
    assert_eq!(
        rustix::process::test_kill_process_group(rustix::process::Pid::from_raw(root_pid).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
    assert!(credentials.0.lock().unwrap().is_empty());
}
