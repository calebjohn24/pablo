use pablo_core::deployment::{self, ConfigInput, ResolveRequest, ResolvedDeployment};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture(PathBuf);

#[test]
fn skill_root_authority_rechecks_task_workspace_and_intersects_every_layer() {
    let f = Fixture::new();
    let value = json!({"options":{"skills":{"roots":{"local":{"base":"workspace","path":".agents/skills"}}}},"authority":[{"id":"host","skill_roots":[{"base":"config","path":"."}]}]});
    let resolved = f.resolve(value.clone());
    assert!(resolved.skill_roots(None).is_ok());
    let outside = Fixture::new();
    let error = resolved.skill_roots(Some(&outside.0)).unwrap_err();
    assert_eq!(error.code, "config_authority_violation");
    assert_eq!(error.authority_id.as_deref(), Some("host"));
    assert!(resolved.skill_roots(Some(&f.0.join("child"))).is_ok());
    let mut denied = value;
    denied["authority"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"narrow","skill_roots":[]}));
    assert_eq!(
        deployment::resolve(f.document(denied))
            .unwrap_err()
            .authority_id
            .as_deref(),
        Some("narrow")
    );
}

#[derive(Default)]
struct PrivateInputs {
    environment: Option<Vec<u8>>,
    host: Option<Vec<u8>>,
    host_calls: std::sync::atomic::AtomicUsize,
}
impl deployment::CredentialInputs for PrivateInputs {
    fn environment(&self, _: &str) -> Result<Option<Vec<u8>>, deployment::CredentialReadError> {
        Ok(self.environment.clone())
    }
    fn host(&self, _: &str) -> Result<Option<Vec<u8>>, deployment::CredentialReadError> {
        self.host_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.host.clone())
    }
}

fn prepared_credential(f: &Fixture, sources: Value) -> deployment::PreparedRun {
    f.resolve(json!({"credentials":{"gateway":{"consumer":"provider.vercel","sources":sources}}}))
        .prepare_run(deployment::RunInput {
            input: "synthetic task".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap()
}

#[test]
fn openrouter_credentials_resolve_privately_and_cannot_cross_consumers_or_destinations() {
    use deployment::CredentialConsumer::{OpenRouter, OtelHeaders, Vercel};
    use pablo_core::gateway::{OPENROUTER_ENDPOINT, VERCEL_ENDPOINT};
    let f = Fixture::new();
    let prepared = f.resolve(json!({
        "options":{"model":{"provider":"openrouter"}},
        "credentials":{"gateway":{"consumer":"provider.openrouter","sources":[
            {"kind":"environment","name":"OPENROUTER_API_KEY"},
            {"kind":"file","path":{"base":"config","path":"synthetic.env"},"encoding":"dotenv","key":"OPENROUTER_API_KEY"},
            {"kind":"host","name":"router"}
        ]}}
    })).prepare_run(deployment::RunInput{input:"synthetic".into(),session_id:None,workspace:None}).unwrap();
    let mut inputs = PrivateInputs {
        host: Some(b"private-router-host".to_vec()),
        ..Default::default()
    };
    for (environment, file, expected) in [
        (
            Some("private-router-env"),
            Some("OPENROUTER_API_KEY=private-router-file\n"),
            "private-router-env",
        ),
        (
            None,
            Some("OPENROUTER_API_KEY=private-router-file\nAI_GATEWAY_API_KEY=private-vercel\n"),
            "private-router-file",
        ),
        (None, None, "private-router-host"),
    ] {
        inputs.environment = environment.map(|v| v.as_bytes().to_vec());
        if let Some(file) = file {
            f.write("synthetic.env", file);
        } else {
            fs::remove_file(f.0.join("synthetic.env")).unwrap();
        }
        let credential = prepared.credential(OpenRouter, &inputs).unwrap().unwrap();
        assert_eq!(
            credential
                .expose_for(OpenRouter, OPENROUTER_ENDPOINT)
                .unwrap(),
            expected
        );
        for (consumer, endpoint) in [
            (Vercel, VERCEL_ENDPOINT),
            (Vercel, OPENROUTER_ENDPOINT),
            (OtelHeaders, OPENROUTER_ENDPOINT),
            (OpenRouter, VERCEL_ENDPOINT),
            (OpenRouter, "http://127.0.0.1:1/fixture"),
        ] {
            assert_eq!(
                credential.expose_for(consumer, endpoint).unwrap_err().code,
                "config_credential_scope"
            );
        }
        let surfaces = format!(
            "{credential:?} {prepared:?} {} {}",
            serde_json::to_string(prepared.deployment()).unwrap(),
            prepared.deployment().render().unwrap()
        );
        assert!(!surfaces.contains("private-router"));
    }
    assert_eq!(
        prepared.credential(Vercel, &inputs).unwrap_err().code,
        "config_credential_scope"
    );
    for invalid in [
        vec![],
        b"private invalid".to_vec(),
        vec![255],
        vec![b'x'; 8193],
    ] {
        inputs.environment = Some(invalid);
        let before = inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst);
        let error = prepared.credential(OpenRouter, &inputs).unwrap_err();
        assert_eq!(error.code, "config_credential_invalid");
        assert!(!format!("{error:?}").contains("private"));
        assert_eq!(
            inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst),
            before
        );
    }
}

#[test]
fn open_responses_profile_credentials_and_render_preserve_exact_destination_scope() {
    use deployment::CredentialConsumer::{OpenResponses, OpenRouter, Vercel};
    let f = Fixture::new();
    let endpoint = "https://responses.example.test/v1/responses";
    let config = json!({"options":{"model":{"provider":"open_responses","id":"operator/model","endpoint":endpoint,
        "capability_profile":"open-responses-text-tools-v1","auth_header":"X-Api-Key","auth_scheme":"raw"}},
        "credentials":{"gateway":{"consumer":"provider.open_responses","sources":[{"kind":"environment","name":"CUSTOM_RESPONSES_TOKEN"},{"kind":"host","name":"responses"}]}}});
    let resolved = f.resolve(config.clone());
    assert_eq!(resolved.model_profile().unwrap().endpoint, endpoint);
    let prepared = resolved
        .prepare_run(deployment::RunInput {
            input: "task".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap();
    let inputs = PrivateInputs {
        environment: Some(b"private-responses-token".to_vec()),
        ..Default::default()
    };
    let credential = prepared
        .credential(OpenResponses, &inputs)
        .unwrap()
        .unwrap();
    assert_eq!(
        credential.expose_for(OpenResponses, endpoint).unwrap(),
        "private-responses-token"
    );
    for (consumer, destination) in [
        (Vercel, endpoint),
        (OpenRouter, endpoint),
        (OpenResponses, "https://responses.example.test/other"),
        (OpenResponses, "https://other.example.test/v1/responses"),
    ] {
        assert_eq!(
            credential
                .expose_for(consumer, destination)
                .unwrap_err()
                .code,
            "config_credential_scope"
        );
    }
    assert!(
        !format!(
            "{credential:?} {prepared:?} {}",
            prepared.deployment().render().unwrap()
        )
        .contains("private-responses-token")
    );
    for (field, value) in [
        (
            "endpoint",
            json!("http://responses.example.test/v1/responses"),
        ),
        ("capability_profile", json!("unknown")),
        ("auth_header", json!("Host")),
        ("auth_scheme", json!("unknown")),
    ] {
        let mut bad = config.clone();
        bad["options"]["model"][field] = value;
        assert!(deployment::resolve(f.document(bad)).is_err());
    }
    let mut small = config.clone();
    small["options"]["limits"] = json!({"max_output_tokens":15});
    assert_eq!(
        deployment::resolve(f.document(small)).unwrap_err().code,
        "config_invalid_value"
    );
    let mut defaults = config.clone();
    defaults["options"]["model"]
        .as_object_mut()
        .unwrap()
        .remove("auth_header");
    defaults["options"]["model"]
        .as_object_mut()
        .unwrap()
        .remove("auth_scheme");
    let resolved = f.resolve(defaults);
    assert_eq!(resolved.options()["model"]["auth_header"], "Authorization");
    assert_eq!(resolved.options()["model"]["auth_scheme"], "bearer");
    assert!(!resolved.provenance()["/config/options/model/auth_header"].is_empty());
}

#[test]
fn credential_fallback_only_uses_absent_sources_and_never_hashes_or_serializes_values() {
    use deployment::CredentialConsumer::Vercel;
    let f = Fixture::new();
    let prepared = prepared_credential(
        &f,
        json!([
            {"kind":"environment","name":"SYNTHETIC_PRIVATE"},
            {"kind":"file","path":{"base":"config","path":"missing.token"},"encoding":"utf8"},
            {"kind":"host","name":"synthetic-host"}
        ]),
    );
    let mut inputs = PrivateInputs {
        host: Some(b"synthetic-private-first".to_vec()),
        ..Default::default()
    };
    let first = prepared.credential(Vercel, &inputs).unwrap().unwrap();
    assert_eq!(
        inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        first
            .expose_for(Vercel, pablo_core::gateway::VERCEL_ENDPOINT)
            .unwrap(),
        "synthetic-private-first"
    );
    assert!(
        first
            .expose_for(
                deployment::CredentialConsumer::OtelHeaders,
                pablo_core::gateway::VERCEL_ENDPOINT
            )
            .is_err()
    );
    assert!(
        first
            .expose_for(Vercel, "http://127.0.0.1:1234/fixture")
            .is_err()
    );
    inputs.host = Some(b"synthetic-private-second".to_vec());
    let second = prepared.credential(Vercel, &inputs).unwrap().unwrap();
    assert!(!first.same_private_value(&second));
    assert!(second.same_private_value(&second));
    assert!(!format!("{first:?} {prepared:?}").contains("synthetic-private"));
    assert!(
        !serde_json::to_string(prepared.deployment())
            .unwrap()
            .contains("synthetic-private")
    );
    assert!(
        !prepared
            .deployment()
            .render()
            .unwrap()
            .contains("synthetic-private")
    );
    for bad in [
        vec![],
        b" \t".to_vec(),
        vec![255],
        b"private\n".to_vec(),
        vec![b'x'; 65_537],
        vec![b'x'; 8193],
    ] {
        inputs.environment = Some(bad);
        let before = inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst);
        let error = prepared.credential(Vercel, &inputs).unwrap_err();
        assert_eq!(error.code, "config_credential_invalid");
        assert!(!format!("{error:?}").contains("private"));
        assert_eq!(
            inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst),
            before
        );
    }
    inputs.environment = None;
    inputs.host = None;
    assert_eq!(
        prepared.credential(Vercel, &inputs).unwrap_err().code,
        "config_credential_missing"
    );
}

#[test]
fn credential_token_and_dotenv_files_are_bounded_literal_and_private() {
    use deployment::CredentialConsumer::Vercel;
    let f = Fixture::new();
    let inputs = PrivateInputs {
        host: Some(b"fallback-must-not-run".to_vec()),
        ..Default::default()
    };
    for (encoding, content, expected) in [
        ("utf8", "synthetic-token\r\n", "synthetic-token"),
        (
            "dotenv",
            "# comment\r\nexport TOKEN = 'literal-${NOT_EXPANDED}' # comment\r\nOTHER=ignored\n",
            "literal-${NOT_EXPANDED}",
        ),
        (
            "dotenv",
            "TOKEN=literal-$NOT_EXPANDED # comment\n",
            "literal-$NOT_EXPANDED",
        ),
        (
            "dotenv",
            "TOKEN=\"literal-\\$NOT_EXPANDED\"\n",
            "literal-$NOT_EXPANDED",
        ),
    ] {
        f.write("private.token", content);
        let mut source = json!({"kind":"file","path":{"base":"config","path":"private.token"},"encoding":encoding});
        if encoding == "dotenv" {
            source["key"] = "TOKEN".into();
        }
        let prepared = prepared_credential(&f, json!([source]));
        let credential = prepared.credential(Vercel, &inputs).unwrap().unwrap();
        assert_eq!(
            credential
                .expose_for(Vercel, pablo_core::gateway::VERCEL_ENDPOINT)
                .unwrap(),
            expected
        );
    }
    for content in [
        "TOKEN=one\nTOKEN=two\n",
        "OTHER=bad\u{7}value\nTOKEN=valid\n",
        "OTHER=bad\rvalue\nTOKEN=valid\n",
        "TOKEN=\"unclosed",
        "OTHER=value\n",
        "TOKEN=\n",
        "malformed private bytes",
        "TOKEN=\"escaped\\nnewline\"\n",
        "TOKEN=valid\nOTHER=one\nOTHER=two\n",
    ] {
        f.write("private.token", content);
        let prepared = prepared_credential(
            &f,
            json!([
                {"kind":"file","path":{"base":"config","path":"private.token"},"encoding":"dotenv","key":"TOKEN"},
                {"kind":"host","name":"fallback"}
            ]),
        );
        assert_eq!(
            prepared.credential(Vercel, &inputs).unwrap_err().code,
            "config_credential_invalid"
        );
    }
    let prepared = prepared_credential(
        &f,
        json!([
            {"kind":"file","path":{"base":"config","path":"private.token"},"encoding":"utf8"},
            {"kind":"host","name":"fallback"}
        ]),
    );
    for bytes in [
        vec![0xff],
        vec![b'x'; 65_537],
        b"two\nnewlines\n".to_vec(),
        vec![],
    ] {
        fs::write(f.0.join("private.token"), bytes).unwrap();
        assert_eq!(
            prepared.credential(Vercel, &inputs).unwrap_err().code,
            "config_credential_invalid"
        );
    }
    assert_eq!(
        inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[cfg(unix)]
#[test]
fn credential_files_reject_symlinks_directories_fifos_and_unreadable_sources() {
    use deployment::CredentialConsumer::Vercel;
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    f.write("actual.token", "synthetic-private-token");
    symlink("actual.token", f.0.join("symlink.token")).unwrap();
    symlink("absent.token", f.0.join("dangling.token")).unwrap();
    fs::create_dir(f.0.join("directory.token")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(f.0.join("fifo.token"))
            .status()
            .unwrap()
            .success()
    );
    fs::set_permissions(f.0.join("actual.token"), fs::Permissions::from_mode(0o0)).unwrap();
    let inputs = PrivateInputs {
        host: Some(b"fallback".to_vec()),
        ..Default::default()
    };
    for name in [
        "symlink.token",
        "dangling.token",
        "directory.token",
        "fifo.token",
        "actual.token",
    ] {
        let prepared = prepared_credential(
            &f,
            json!([
                {"kind":"file","path":{"base":"config","path":name},"encoding":"utf8"},
                {"kind":"host","name":"fallback"}
            ]),
        );
        assert_eq!(
            prepared.credential(Vercel, &inputs).unwrap_err().code,
            "config_credential_invalid",
            "{name}"
        );
    }
    assert_eq!(
        inputs.host_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    fs::set_permissions(f.0.join("actual.token"), fs::Permissions::from_mode(0o600)).unwrap();
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pablo-deployment-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, name: &str, content: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    fn corpus(&self) {
        for (name, text) in [
            (
                "modules/base.toml",
                include_str!("../../../docs/project/fixtures/c3-deployment/modules/base.toml"),
            ),
            (
                "development.toml",
                include_str!("../../../docs/project/fixtures/c3-deployment/development.toml"),
            ),
            (
                "production.toml",
                include_str!("../../../docs/project/fixtures/c3-deployment/production.toml"),
            ),
            (
                "credential-sources.toml",
                include_str!(
                    "../../../docs/project/fixtures/c3-deployment/credential-sources.toml"
                ),
            ),
        ] {
            self.write(name, text);
        }
    }
    fn request(&self, entry: &str) -> ResolveRequest {
        let mut request = ResolveRequest::new(self.0.clone(), entry);
        request
            .path_bindings
            .insert("workspace".into(), self.0.clone());
        request
    }
    fn document(&self, mut document: Value) -> ResolveRequest {
        document["schema_version"] = 1.into();
        if document.get("credentials").is_none() {
            document["credentials"] = json!({"gateway":{"consumer":"provider.vercel","sources":[{"kind":"environment","name":"SYNTHETIC_KEY"}]}});
        }
        let mut request = self.request("unused.toml");
        request.entry = ConfigInput::Document(document);
        request
    }
    fn resolve(&self, document: Value) -> ResolvedDeployment {
        deployment::resolve(self.document(document)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn error(request: ResolveRequest, code: &str) {
    let e = deployment::resolve(request).unwrap_err();
    assert_eq!(e.code, code, "{e}");
}

#[test]
fn rendered_presets_reload_without_profiles_environment_or_secret_access() {
    let f = Fixture::new();
    f.corpus();
    for entry in [
        "production.toml",
        "development.toml",
        "credential-sources.toml",
    ] {
        let mut request = f.request(entry);
        request.path_bindings.insert("secrets".into(), f.0.clone());
        request
            .environment
            .insert("PABLO_DEV_MAX_TOOL_CALLS".into(), "3".into());
        let original = deployment::resolve(request).unwrap();
        let rendered = original.render().unwrap();
        let document: toml::Value = toml::from_str(&rendered).unwrap();
        for absent in ["imports", "profiles", "profile", "environment"] {
            assert!(document.get(absent).is_none());
        }
        f.write("rendered.toml", &rendered);
        let mut request = f.request("rendered.toml");
        request.path_bindings.insert("secrets".into(), f.0.clone());
        let reloaded = deployment::resolve(request).unwrap();
        assert_eq!(original.config(), reloaded.config());
        assert_eq!(original.fingerprint(), reloaded.fingerprint());
        assert_ne!(original.input_fingerprint(), reloaded.input_fingerprint());
        assert_eq!(rendered, reloaded.render().unwrap());
    }
}

#[test]
fn render_preserves_quoted_keys_controls_unicode_and_full_u64_strings() {
    let f = Fixture::new();
    let original = f.resolve(json!({"options": {
        "run": {"instructions": "quote\" slash\\ newline\n tab\t DEL\u{7f} é🙂"},
        "otel": {"resource_attributes": {"a.b\"c": "value"}},
        "limits": {"max_total_tokens": "18446744073709551615"}
    }}));
    f.write("rendered.toml", &original.render().unwrap());
    let reloaded = deployment::resolve(f.request("rendered.toml")).unwrap();
    assert_eq!(original.fingerprint(), reloaded.fingerprint());
}

#[test]
fn render_rejects_an_entry_that_cannot_reload_within_the_file_bound() {
    let f = Fixture::new();
    let result = f.resolve(
        json!({"options":{"run":{"instructions":"x".repeat(deployment::MAX_FILE_BYTES - 300)}}}),
    );
    assert_eq!(result.render().unwrap_err().code, "config_limit");
}

#[test]
fn environment_capture_finishes_the_same_loaded_files() {
    let f = Fixture::new();
    f.corpus();
    let loaded = deployment::load(f.request("development.toml")).unwrap();
    let names: Vec<_> = loaded.environment_names().map(str::to_owned).collect();
    assert_eq!(names.len(), 1);
    f.write("development.toml", "invalid replacement");
    let result = loaded
        .with_environment([(names[0].clone(), "3".into())].into())
        .unwrap()
        .resolve()
        .unwrap();
    assert_eq!(result.options()["limits"]["max_tool_calls"], 3);
    assert_eq!(
        deployment::resolve(f.request("development.toml"))
            .unwrap_err()
            .code,
        "config_parse"
    );
    let loaded = deployment::load(f.request("production.toml")).unwrap();
    assert!(
        loaded
            .with_environment([("UNDECLARED".into(), "private".into())].into())
            .is_err()
    );
}

#[test]
fn secret_environment_names_cannot_be_reclassified_as_inspection_options() {
    let f = Fixture::new();
    for name in [
        "SYNTHETIC_KEY",
        "AI_GATEWAY_API_KEY",
        "VERCEL_AI_GATEWAY",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    ] {
        let request = f.document(json!({"environment":{name:{"option":"otel.service_name"}}}));
        assert!(matches!(deployment::load(request), Err(e) if e.code == "config_invalid_value"));
    }
}

#[test]
fn filesystem_work_quotas_are_optional_but_explicit_authority_still_applies() {
    let f = Fixture::new();
    for field in [
        "max_file_bytes",
        "max_entries",
        "max_depth",
        "max_scan_bytes",
    ] {
        for value in [json!("unlimited"), json!(17)] {
            let resolved = deployment::resolve(f.document(json!({
                "options":{"limits":{"filesystem":{field:value}}}
            })))
            .unwrap();
            let prepared = resolved
                .prepare_run(deployment::RunInput {
                    input: "synthetic".into(),
                    workspace: Some(f.0.clone()),
                    session_id: None,
                })
                .unwrap();
            let limits = serde_json::to_value(&prepared.spec().limits.filesystem).unwrap();
            assert_eq!(
                limits[field],
                if value.is_string() {
                    Value::Null
                } else {
                    value
                }
            );
        }
        let error = deployment::resolve(f.document(json!({
            "options":{"limits":{"filesystem":{field:"unlimited"}}},
            "authority":[{"id":"host","limits":{"filesystem":{field:17}}}]
        })))
        .unwrap_err();
        assert_eq!(error.code, "config_authority_violation");
    }
}

#[test]
fn prepared_runs_project_limits_catalog_and_private_binding_identity() {
    let f = Fixture::new();
    f.corpus();
    let deployment = deployment::resolve(f.request("production.toml")).unwrap();
    let prepared = deployment
        .prepare_run(deployment::RunInput {
            input: "synthetic task".into(),
            session_id: Some("session-one".into()),
            workspace: Some(f.0.clone()),
        })
        .unwrap();
    assert_eq!(prepared.spec().workspace, f.0);
    let filesystem = &prepared.spec().limits.filesystem;
    assert_eq!(filesystem.max_file_bytes, None);
    assert_eq!(filesystem.max_entries, None);
    assert_eq!(filesystem.max_depth, None);
    assert_eq!(filesystem.max_scan_bytes, None);
    assert_eq!(prepared.spec().limits.max_model_calls, Some(4));
    assert_eq!(prepared.spec().limits.max_tool_calls, Some(8));
    assert_eq!(prepared.spec().limits.max_run_duration_ms, 600000);
    assert!(!prepared.spec().trace.capture_content);
    assert_eq!(
        prepared
            .tools()
            .unwrap()
            .descriptors()
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["fs.read", "fs.list", "fs.search"]
    );
    assert_eq!(
        prepared.deployment().fingerprint(),
        deployment.fingerprint()
    );
    assert!(!format!("{prepared:?}").contains("synthetic task"));
    let other = Fixture::new();
    let mut request = f.request("production.toml");
    request
        .path_bindings
        .insert("workspace".into(), other.0.clone());
    let alternate = deployment::resolve(request)
        .unwrap()
        .prepare_run(deployment::RunInput {
            input: "other".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap();
    assert_eq!(
        prepared.deployment().fingerprint(),
        alternate.deployment().fingerprint()
    );
    assert_ne!(
        prepared.bindings_fingerprint(),
        alternate.bindings_fingerprint()
    );
    assert!(
        !serde_json::to_string(prepared.deployment())
            .unwrap()
            .contains(f.0.to_str().unwrap())
    );
}

#[test]
fn child_workspace_admission_keeps_ceilings_and_updates_effective_identity() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("child")).unwrap();
    let original = f.resolve(json!({"deployment":{"locked":true,"allowed_run_overrides":["input","run.workspace"]},
        "authority":[{"id":"root.ceiling","workspace_roots":[{"base":"binding","name":"workspace","path":"."}]}]}));
    let child = original
        .prepare_run(deployment::RunInput {
            input: "task".into(),
            session_id: None,
            workspace: Some(f.0.join("child")),
        })
        .unwrap();
    assert_eq!(child.spec().workspace, f.0.join("child"));
    assert_ne!(original.fingerprint(), child.deployment().fingerprint());
    let direct = original
        .with_overrides(
            [(
                "run.workspace".into(),
                json!({"base":"binding","name":"workspace","path":"child"}),
            )]
            .into_iter()
            .collect(),
        )
        .unwrap();
    assert_eq!(child.deployment().fingerprint(), direct.fingerprint());
    assert_eq!(
        child.deployment().config()["authority"],
        original.config()["authority"]
    );
    let other = Fixture::new();
    let error = original
        .prepare_run(deployment::RunInput {
            input: "task".into(),
            session_id: None,
            workspace: Some(other.0.clone()),
        })
        .unwrap_err();
    assert_eq!(error.code, "config_authority_violation");
    let locked = f.resolve(json!({"deployment":{"locked":true}}));
    assert_eq!(
        locked
            .prepare_run(deployment::RunInput {
                input: "task".into(),
                session_id: None,
                workspace: Some(f.0.join("child"))
            })
            .unwrap_err()
            .code,
        "config_override_forbidden"
    );
}

#[test]
fn prepared_run_rejects_endpoint_and_unavailable_paths_without_creating_trace() {
    let f = Fixture::new();
    let input = || deployment::RunInput {
        input: "task".into(),
        session_id: Some("safe-session".into()),
        workspace: None,
    };
    let trace = f.resolve(
        json!({"options":{"trace":{"path":{"base":"workspace","path":"{session_id}.jsonl"}}}}),
    );
    let prepared = trace.prepare_run(input()).unwrap();
    assert_eq!(
        prepared.trace_path(),
        Some(f.0.join("safe-session.jsonl").as_path())
    );
    assert!(!f.0.join("safe-session.jsonl").exists());
    f.write("safe-session.jsonl", "existing sentinel");
    assert_eq!(
        trace.prepare_run(input()).unwrap_err().code,
        "config_path_unavailable"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("safe-session.jsonl")).unwrap(),
        "existing sentinel"
    );
    let wrong_endpoint = deployment::resolve(
        f.document(json!({"options":{"model":{"endpoint":"https://example.invalid/steal"}}})),
    );
    assert_eq!(wrong_endpoint.unwrap_err().code, "config_invalid_value");
    let missing = f.resolve(json!({"options":{"run":{"workspace":{"base":"binding","name":"workspace","path":"missing"}}}}));
    assert_eq!(
        missing.prepare_run(input()).unwrap_err().code,
        "config_path_unavailable"
    );
}

#[cfg(unix)]
#[test]
fn workspace_symlinks_cannot_widen_physical_roots() {
    let f = Fixture::new();
    let outside = Fixture::new();
    std::os::unix::fs::symlink(&outside.0, f.0.join("escape")).unwrap();
    let config = f.resolve(json!({"options":{"run":{"workspace":{"base":"binding","name":"workspace","path":"escape"}}}}));
    let e = config
        .prepare_run(deployment::RunInput {
            input: "task".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap_err();
    assert_eq!(e.code, "config_authority_violation");
}

#[test]
fn production_matches_frozen_config_sources_and_fingerprints() {
    let f = Fixture::new();
    f.corpus();
    let result = deployment::resolve(f.request("production.toml")).unwrap();
    let golden: Value = serde_json::from_str(include_str!(
        "../../../docs/project/fixtures/c3-deployment/production.resolved.json"
    ))
    .unwrap();
    assert_eq!(result.config(), &golden["config"]);
    assert_eq!(result.fingerprint(), golden["fingerprint"]);
    assert_eq!(result.input_fingerprint(), golden["input_fingerprint"]);
    assert_eq!(
        serde_json::to_value(result.sources()).unwrap(),
        golden["sources"]
    );
    assert_eq!(serde_json::to_value(&result).unwrap(), golden);
    for (path, origins) in result.provenance() {
        assert!(
            result
                .config()
                .pointer(path.strip_prefix("/config").unwrap())
                .is_some(),
            "{path}"
        );
        for origin in origins {
            assert!(result.sources().iter().any(|s| s.id == origin.source));
        }
    }
    for path in golden["provenance"].as_object().unwrap().keys() {
        assert!(result.provenance().contains_key(path), "{path}");
    }
    let again = deployment::resolve(f.request("production.toml")).unwrap();
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::to_value(again).unwrap()
    );
}

#[test]
fn modules_profiles_lists_and_environment_keep_order_and_origins() {
    let f = Fixture::new();
    f.corpus();
    let result = deployment::resolve(f.request("development.toml")).unwrap();
    assert_eq!(result.options()["limits"]["max_model_calls"], "unlimited");
    assert_eq!(result.options()["limits"]["max_tool_calls"], 20);
    assert_eq!(result.options()["filesystem"]["write"], true);
    let rules = result.options()["policy"]["read_roots"]["allow"]
        .as_array()
        .unwrap();
    assert_eq!(
        rules
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["read.workspace", "read.shared", "read.docs"]
    );
    let origins = result.provenance();
    for (index, operation) in [(0, "prepend"), (1, "replace"), (2, "append")] {
        assert_eq!(
            origins[&format!("/config/options/policy/read_roots/allow/{index}/id")]
                .last()
                .unwrap()
                .operation,
            operation
        );
    }
    let mut request = f.request("development.toml");
    request
        .environment
        .insert("PABLO_CONFIG_TOOL_CALLS".into(), "7".into());
    request
        .overrides
        .insert("limits.max_tool_calls".into(), 5.into());
    let result = deployment::resolve(request).unwrap();
    assert_eq!(result.options()["limits"]["max_tool_calls"], 5);
    let origins = &result.provenance()["/config/options/limits/max_tool_calls"];
    assert_eq!(origins.len(), 4);
    assert_eq!(result.sources().last().unwrap().kind, "override");
}

#[test]
fn ordinary_user_workspace_entry_environment_and_host_precedence() {
    let f = Fixture::new();
    f.corpus();
    f.write(
        "user.toml",
        "schema_version=1\n[options.limits]\nmax_tool_calls=100\n",
    );
    f.write(
        "workspace.toml",
        "schema_version=1\n[options.limits]\nmax_tool_calls=50\n",
    );
    let mut request = f.request("development.toml");
    request.user_config = Some(ConfigInput::File("user.toml".into()));
    request.workspace_config = Some(ConfigInput::File("workspace.toml".into()));
    request
        .environment
        .insert("PABLO_CONFIG_TOOL_CALLS".into(), "7".into());
    request
        .overrides
        .insert("limits.max_tool_calls".into(), 5.into());
    let result = deployment::resolve(request).unwrap();
    assert_eq!(result.options()["limits"]["max_tool_calls"], 5);
    assert_eq!(
        result.provenance()["/config/options/limits/max_tool_calls"].len(),
        6
    );
}

#[test]
fn metadata_with_operation_keys_is_data_and_maps_merge() {
    let f = Fixture::new();
    f.write("base.toml","schema_version=1\n[options.otel.resource_attributes]\nstage='base'\nregion='fixture'\nmode='append'\nitems='literal'\nbase='source'\npath='literal'\n");
    let result=f.resolve(json!({"imports":["base.toml"],"options":{"otel":{"resource_attributes":{"stage":"entry"}}}}));
    assert_eq!(
        result.options()["otel"]["resource_attributes"],
        json!({"stage":"entry","region":"fixture","mode":"append","items":"literal","base":"source","path":"literal"})
    );
}

#[test]
fn resource_attribute_limits_apply_after_map_composition() {
    let f = Fixture::new();
    let attrs: serde_json::Map<_, _> = (0..64)
        .map(|i| (format!("key{i}"), json!("value")))
        .collect();
    assert!(
        deployment::resolve(f.document(json!({"options":{"otel":{"resource_attributes":attrs}}})))
            .is_ok()
    );
    let mut request = f.document(json!({"options":{"otel":{"resource_attributes":attrs}}}));
    request
        .overrides
        .insert("otel.resource_attributes".into(), json!({"extra":"value"}));
    error(request, "config_limit");
    error(
        f.document(
            json!({"options":{"otel":{"resource_attributes":{"multibyte":"é".repeat(2049)}}}}),
        ),
        "config_invalid_value",
    );
}

#[test]
fn lists_clear_optionals_unset_and_trace_creation_is_deferred() {
    let f = Fixture::new();
    f.corpus();
    let mut request = f.request("development.toml");
    request
        .overrides
        .insert("trace.path".into(), json!({"unset":true}));
    request.overrides.insert(
        "policy.read_roots".into(),
        json!({"default":"allow","allow":[]}),
    );
    let result = deployment::resolve(request).unwrap();
    assert_eq!(result.options()["trace"]["path"], json!({"unset":true}));
    assert_eq!(result.options()["policy"]["read_roots"]["allow"], json!([]));
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 4);
    assert!(
        result
            .provenance()
            .keys()
            .all(|p| !p.ends_with("/trace/path/base"))
    );
}

#[test]
fn source_changes_and_effective_changes_have_distinct_identities() {
    let f = Fixture::new();
    f.corpus();
    let before = deployment::resolve(f.request("production.toml")).unwrap();
    let path = f.0.join("modules/base.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n# operator comment\n");
    fs::write(path, text).unwrap();
    let comment = deployment::resolve(f.request("production.toml")).unwrap();
    assert_eq!(before.fingerprint(), comment.fingerprint());
    assert_ne!(before.input_fingerprint(), comment.input_fingerprint());
    let path = f.0.join("production.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("max_tool_calls = 8", "max_tool_calls = 7"),
    )
    .unwrap();
    let changed = deployment::resolve(f.request("production.toml")).unwrap();
    assert_ne!(comment.fingerprint(), changed.fingerprint());
}

#[test]
fn relative_import_and_source_paths_use_declaring_file_and_explicit_root() {
    let f = Fixture::new();
    f.corpus();
    f.write(
        "nested/entry.toml",
        "schema_version=1\nimports=['../modules/base.toml','../modules/trace.toml']\n",
    );
    f.write(
        "modules/trace.toml",
        "schema_version=1\n[options.trace]\npath={base='source',path='output/trace.jsonl'}\n",
    );
    let result = deployment::resolve(f.request("nested/entry.toml")).unwrap();
    assert_eq!(
        result.options()["trace"]["path"],
        json!({"base":"config","path":"modules/output/trace.jsonl"})
    );
    assert!(!f.0.join("modules/output").exists());
}

#[test]
fn import_cycles_repeated_files_and_diamonds_reject() {
    let f = Fixture::new();
    for (entry, a, b, expected) in [
        ("['a.toml']", "['entry.toml']", "[]", "config_import_cycle"),
        ("['a.toml','a.toml']", "[]", "[]", "config_duplicate_import"),
        (
            "['a.toml','b.toml']",
            "['shared.toml']",
            "['shared.toml']",
            "config_duplicate_import",
        ),
    ] {
        for (name, imports) in [
            ("entry.toml", entry),
            ("a.toml", a),
            ("b.toml", b),
            ("shared.toml", "[]"),
        ] {
            f.write(name, &format!("schema_version=1\nimports={imports}\n"));
        }
        error(f.request("entry.toml"), expected);
    }
}

#[test]
fn profile_cycles_missing_parents_and_diamond_ancestors_reject() {
    let f = Fixture::new();
    for (profiles, code) in [
        (
            json!({"a":{"extends":["b"]},"b":{"extends":["a"]}}),
            "config_profile_cycle",
        ),
        (
            json!({"a":{"extends":["missing"]}}),
            "config_unknown_profile",
        ),
        (
            json!({"a":{"extends":["b","c"]},"b":{"extends":["d"]},"c":{"extends":["d"]},"d":{}}),
            "config_duplicate_profile_ancestor",
        ),
    ] {
        error(f.document(json!({"profile":"a","profiles":profiles})), code);
    }
}

#[test]
fn named_definitions_conflict_and_equal_definitions_coalesce() {
    let f = Fixture::new();
    f.corpus();
    let base = fs::read_to_string(f.0.join("modules/base.toml")).unwrap();
    f.write("same.toml", &base);
    assert!(
        deployment::resolve(
            f.document(json!({"imports":["modules/base.toml","same.toml"],"credentials":{}}))
        )
        .is_ok()
    );
    f.write(
        "same.toml",
        &base.replace("AI_GATEWAY_API_KEY", "DIFFERENT_KEY"),
    );
    error(
        f.document(json!({"imports":["modules/base.toml","same.toml"],"credentials":{}})),
        "config_conflict",
    );
    f.write(
        "same.toml",
        "schema_version=1\n[profiles.inspection.options.filesystem]\nwrite=true\n",
    );
    error(
        f.document(json!({"imports":["modules/base.toml","same.toml"],"credentials":{}})),
        "config_conflict",
    );
    f.write(
        "env_a.toml",
        "schema_version=1\n[environment.CAP]\noption='limits.max_tool_calls'\n",
    );
    f.write(
        "env_b.toml",
        "schema_version=1\n[environment.CAP]\noption='limits.max_model_calls'\n",
    );
    error(
        f.document(json!({"imports":["env_a.toml","env_b.toml"]})),
        "config_conflict",
    );
}

#[test]
fn coalesced_profiles_apply_ordered_lists_once_and_keep_each_origin() {
    let f = Fixture::new();
    let profile = "schema_version=1\n[profiles.shared.options.policy.tools]\ndefault='allow'\nallow={mode='append',items=[{id='one.rule',value='fs.read'}]}\n";
    f.write("a.toml", profile);
    f.write("b.toml", profile);
    let result = f.resolve(json!({"imports":["a.toml","b.toml"],"profile":"shared"}));
    assert_eq!(
        result.options()["policy"]["tools"]["allow"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let origins = &result.provenance()["/config/options/policy/tools/allow/0/id"];
    assert_eq!(origins.len(), 2);
    let locators: Vec<_> = origins
        .iter()
        .map(|origin| {
            result
                .sources()
                .iter()
                .find(|s| s.id == origin.source)
                .unwrap()
                .locator
                .as_str()
        })
        .collect();
    assert_eq!(locators, ["a.toml#shared", "b.toml#shared"]);
}

#[test]
fn policy_identity_conflicts_survive_list_operations_and_authority_layers() {
    let f = Fixture::new();
    let rule = json!({"id":"same","value":"fs.read"});
    let options = json!({"policy":{"tools":{"default":"allow","allow":[rule]}}});
    error(f.document(json!({"options":options,"authority":[{"id":"host","policy":{"tools":{"default":"deny","allow":[rule]}}}]})),"config_conflict");
    error(f.document(json!({"options":{"policy":{"tools":{"default":"allow","allow":[rule],"deny":[rule]}}}})), "config_conflict");
    f.write("policy.toml", "schema_version=1\n[options.policy.tools]\ndefault='allow'\nallow=[{id='same',value='fs.read'}]\n");
    error(f.document(json!({"imports":["policy.toml"],"options":{"policy":{"tools":{"default":"allow","allow":{"mode":"append","items":[rule]}}}}})), "config_conflict");
    let allows: Vec<_> = (0..65)
        .map(|i| json!({"id":format!("allow.{i}"),"value":"fs.read"}))
        .collect();
    let denies: Vec<_> = (0..64)
        .map(|i| json!({"id":format!("deny.{i}"),"value":"fs.write"}))
        .collect();
    error(f.document(json!({"options":{"policy":{"tools":{"default":"allow","allow":allows,"deny":denies}}}})),"config_limit");
}

#[test]
fn locked_resolution_ignores_untrusted_files_and_undeclared_values() {
    let f = Fixture::new();
    f.corpus();
    let before = deployment::resolve(f.request("production.toml")).unwrap();
    let mut request = f.request("production.toml");
    request.user_config = Some(ConfigInput::File("/not-an-approved-root/user.toml".into()));
    request.workspace_config = Some(ConfigInput::Document(
        json!({"options":{"filesystem":{"write":true}}}),
    ));
    for name in [
        "OTEL_TRACES_EXPORTER",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "AI_GATEWAY_API_KEY",
        "HOME",
        "XDG_CONFIG_HOME",
        "PABLO_TASK_SECRET",
    ] {
        request
            .environment
            .insert(name.into(), "PRIVATE-SENTINEL".into());
    }
    let after = deployment::resolve(request).unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert!(!format!("{after:?}").contains("PRIVATE-SENTINEL"));
}

#[test]
fn locked_overrides_narrow_and_forbidden_equal_assignments_reject() {
    let f = Fixture::new();
    f.corpus();
    for (option, value, expected) in [
        ("limits.max_run_duration_ms", json!(300000), None),
        ("input", json!("DYNAMIC-TASK-SENTINEL"), None),
        (
            "limits.max_run_duration_ms",
            json!(600001),
            Some("config_authority_violation"),
        ),
        (
            "shell.enabled",
            json!(false),
            Some("config_override_forbidden"),
        ),
        (
            "profile",
            json!("production"),
            Some("config_override_forbidden"),
        ),
        (
            "profile",
            json!("development"),
            Some("config_override_forbidden"),
        ),
        (
            "credentials.gateway",
            json!("other"),
            Some("config_override_forbidden"),
        ),
    ] {
        let mut request = f.request("production.toml");
        request.overrides.insert(option.into(), value);
        if let Some(code) = expected {
            error(request, code);
        } else {
            let result = deployment::resolve(request).unwrap();
            assert!(
                !serde_json::to_string(&result)
                    .unwrap()
                    .contains("DYNAMIC-TASK-SENTINEL")
            );
        }
    }
}

#[test]
fn authority_ceiling_cannot_be_widened_by_profile_or_environment() {
    let f = Fixture::new();
    let document = json!({"profile":"selected","profiles":{"selected":{"options":{"limits":{"max_tool_calls":9}}}},"environment":{"TOOL_CAP":{"option":"limits.max_tool_calls"}},"authority":[{"id":"ceiling","limits":{"max_tool_calls":8}}]});
    let mut request = f.document(document);
    request.environment.insert("TOOL_CAP".into(), "10".into());
    let e = deployment::resolve(request).unwrap_err();
    assert_eq!(e.code, "config_authority_violation");
    assert_eq!(e.authority_id.as_deref(), Some("ceiling"));
}

#[test]
fn workspace_authority_stays_anchored_when_later_layers_change_workspace() {
    let f = Fixture::new();
    let document = json!({"options":{"run":{"workspace":{"base":"binding","name":"workspace","path":"approved"}}},"authority":[{"id":"fixed","workspace_roots":[{"base":"workspace","path":"."}]}]});
    let result = f.resolve(document.clone());
    assert_eq!(
        result.config()["authority"][0]["workspace_roots"][0],
        json!({"base":"binding","name":"workspace","path":"approved"})
    );
    for path in ["approved/child", "sibling"] {
        let mut request = f.document(document.clone());
        request.overrides.insert(
            "run.workspace".into(),
            json!({"base":"binding","name":"workspace","path":path}),
        );
        if path == "sibling" {
            let e = deployment::resolve(request).unwrap_err();
            assert_eq!(e.code, "config_authority_violation");
            assert_eq!(e.authority_id.as_deref(), Some("fixed"));
        } else {
            assert!(deployment::resolve(request).is_ok());
        }
    }
    let mut request = f.document(
        json!({"options":{"run":{"workspace":{"base":"binding","name":"elsewhere","path":"."}}}}),
    );
    request
        .path_bindings
        .insert("elsewhere".into(), f.0.with_file_name("other-workspace"));
    request
        .host_authority
        .push(json!({"id":"host","workspace_roots":[{"base":"workspace","path":"."}]}));
    error(request, "config_authority_violation");
}

#[test]
fn inactive_and_overridden_invalid_declarations_are_not_hidden() {
    let f = Fixture::new();
    for body in [
        json!({"options":{"limits":{"max_total_tokens":"18446744073709551616"}}}),
        json!({"options":{"limits":{"max_events":"3"}}}),
        json!({"options":{"run":{"workspace":{"base":"workspace","path":"."}}}}),
        json!({"options":{"trace":{"path":{"base":"binding","name":"workspace","path":"../escape"}}}}),
    ] {
        error(
            f.document(json!({"profiles":{"unused":body}})),
            "config_invalid_value",
        );
    }
    let mut request =
        f.document(json!({"options":{"limits":{"max_total_tokens":"18446744073709551616"}}}));
    request
        .overrides
        .insert("limits.max_total_tokens".into(), "4".into());
    error(request, "config_invalid_value");
}

#[test]
fn private_file_and_host_sources_are_references_only() {
    let f = Fixture::new();
    f.corpus();
    fs::create_dir(f.0.join("secrets")).unwrap();
    let mut request = f.request("credential-sources.toml");
    request
        .path_bindings
        .insert("secrets".into(), f.0.join("secrets"));
    let before = deployment::resolve(request).unwrap();
    f.write("secrets/gateway.token", "PRIVATE-GATEWAY-SENTINEL");
    f.write("secrets/collector.env", "PRIVATE-EXPORTER-SENTINEL");
    let mut request = f.request("credential-sources.toml");
    request
        .path_bindings
        .insert("secrets".into(), f.0.join("secrets"));
    request.environment.insert(
        "SYNTHETIC_GATEWAY_KEY".into(),
        "PRIVATE-ENV-SENTINEL".into(),
    );
    let after = deployment::resolve(request).unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    let serialized = serde_json::to_string(&after).unwrap();
    for sentinel in [
        "PRIVATE-GATEWAY-SENTINEL",
        "PRIVATE-EXPORTER-SENTINEL",
        "PRIVATE-ENV-SENTINEL",
    ] {
        assert!(!serialized.contains(sentinel));
    }
}

#[test]
fn environment_scalar_parsing_preserves_zero_unlimited_and_errors() {
    let f = Fixture::new();
    f.corpus();
    for (text, result) in [
        ("0", Some(json!(0))),
        ("unlimited", Some(json!("unlimited"))),
        ("7", Some(json!(7))),
        ("", None),
        ("1.0", None),
        ("-1", None),
        ("4294967296", None),
    ] {
        let mut request = f.request("development.toml");
        request
            .environment
            .insert("PABLO_CONFIG_TOOL_CALLS".into(), text.into());
        match result {
            Some(value) => assert_eq!(
                deployment::resolve(request).unwrap().options()["limits"]["max_tool_calls"],
                value
            ),
            None => error(request, "config_environment_value"),
        };
    }
    let request = f.document(
        json!({"environment":{"REQUIRED":{"option":"limits.max_tool_calls","required":true}}}),
    );
    error(request, "config_environment_missing");
    error(f.document(json!({"environment":{"A":{"option":"limits.max_tool_calls"},"B":{"option":"limits.max_tool_calls"}}})),"config_conflict");
}

#[test]
fn exact_u64_ceiling_and_event_ranges() {
    let f = Fixture::new();
    assert_eq!(
        f.resolve(json!({"options":{"limits":{"max_total_tokens":"18446744073709551615"}}}))
            .options()["limits"]["max_total_tokens"],
        "18446744073709551615"
    );
    for value in ["18446744073709551616", "01", "-1"] {
        error(
            f.document(json!({"options":{"limits":{"max_total_tokens":value}}})),
            "config_invalid_value",
        );
    }
    assert_eq!(
        f.resolve(json!({"options":{"limits":{"max_events":"4"}}}))
            .options()["limits"]["max_events"],
        "4"
    );
    error(
        f.document(json!({"options":{"limits":{"max_events":"3"}}})),
        "config_invalid_value",
    );
}

#[test]
fn unknown_unsupported_and_schema_versions_are_explicit_and_redacted() {
    let f = Fixture::new();
    error(
        f.document(json!({"options":{"filesystem":{"wrtie":true}}})),
        "config_unknown_option",
    );
    let e = deployment::resolve(
        f.document(json!({"profiles":{"unused":{"options":{"children":{"enabled":false}}}}})),
    )
    .unwrap_err();
    assert_eq!(e.code, "config_unsupported_feature");
    assert_eq!(e.owner, Some("C3.22"));
    error(
        f.document(json!({"options":{"model":{"provider":"open_responses"}}})),
        "config_invalid_value",
    );
    for version in [Value::Null, json!(0), json!(2), json!("1")] {
        let mut request = f.document(json!({}));
        if let ConfigInput::Document(document) = &mut request.entry {
            document["schema_version"] = version;
            document["imports"] = json!(["never-opened.toml"]);
        }
        error(request, "config_schema_version");
    }
    f.write(
        "private.toml",
        "schema_version=1\npassword = \"PRIVATE-SENTINEL\n",
    );
    let e = deployment::resolve(f.request("private.toml")).unwrap_err();
    assert_eq!(e.code, "config_parse");
    assert!(e.line.is_some());
    assert!(!format!("{e:?}").contains("PRIVATE-SENTINEL"));
    assert!(e.to_string().len() < 1024);
}

#[test]
fn frozen_toml_corpus_is_accepted_or_rejected_by_the_real_loader() {
    let f = Fixture::new();
    f.corpus();
    let corpus = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/project/fixtures/c3-deployment"
    ));
    let cases: Value = serde_json::from_str(include_str!(
        "../../../docs/project/fixtures/c3-deployment/cases.json"
    ))
    .unwrap();
    for case in cases["shape_cases"].as_array().unwrap() {
        let file = case["file"].as_str().unwrap();
        f.write(file, &fs::read_to_string(corpus.join(file)).unwrap());
        let mut request = f.request(file);
        request
            .path_bindings
            .insert("secrets".into(), f.0.join("unopened-secrets"));
        let result = deployment::resolve(request);
        if let Some(expected) = case["resolution_error"].as_str() {
            assert_eq!(result.unwrap_err().code, expected, "{file}");
            continue;
        }
        match case["expected"].as_str().unwrap() {
            "accepted_shape" => {
                result.unwrap();
            }
            "parse_error" => assert_eq!(result.unwrap_err().code, "config_parse", "{file}"),
            "type_error" => assert_eq!(result.unwrap_err().code, "config_invalid_value", "{file}"),
            "schema_error" => {
                assert!(result.is_err(), "{file}");
            }
            other => panic!("unknown corpus expectation {other}"),
        }
    }
}

#[test]
fn semantic_combinations_and_references_reject_before_activation() {
    let f = Fixture::new();
    for options in [
        json!({"model":{"credential":"missing"}}),
        json!({"filesystem":{"enabled":false,"write":true}}),
        json!({"trace":{"capture_content":true}}),
        json!({"otel":{"max_queue_size":1,"max_export_batch_size":2}}),
        json!({"run":{"workspace":{"base":"workspace","path":"."}}}),
        json!({"trace":{"path":{"base":"binding","name":"unknown","path":"."}}}),
        json!({"trace":{"path":{"base":"binding","name":"workspace","path":"../escape"}}}),
        json!({"otel":{"endpoint":"http://[invalid"}}),
    ] {
        error(
            f.document(json!({"options":options})),
            "config_invalid_value",
        );
    }
    error(f.document(json!({"credentials":{"gateway":{"consumer":"otel.headers","sources":[{"kind":"host","name":"synthetic"}]}}})), "config_invalid_value");
}

#[test]
fn canonical_encoding_matches_independent_frozen_vectors() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../docs/project/fixtures/c3-deployment/canonical-vectors.json"
    ))
    .unwrap();
    for vector in vectors {
        assert_eq!(
            deployment::fingerprint(&vector["value"]).unwrap(),
            vector["sha256"]
        );
    }
}

#[cfg(unix)]
#[test]
fn imports_cannot_escape_or_follow_symlinks_or_block_on_fifo() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = Fixture::new();
    outside.write("outside.toml", "schema_version=1\n");
    symlink(outside.0.join("outside.toml"), f.0.join("link.toml")).unwrap();
    symlink(&outside.0, f.0.join("linked-dir")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(f.0.join("fifo.toml"))
            .status()
            .unwrap()
            .success()
    );
    for name in [
        "https://example.invalid/a.toml",
        "/outside/a.toml",
        "../outside.toml",
        "*.toml",
        "link.toml",
        "linked-dir/outside.toml",
        "fifo.toml",
    ] {
        error(f.document(json!({"imports":[name]})), "config_import_path");
    }
    let mut request = f.request("link.toml");
    request
        .path_bindings
        .insert("workspace".into(), f.0.clone());
    error(request, "config_import_path");
}

#[test]
fn limits_on_file_bytes_depth_and_profile_expansion_are_real() {
    let f = Fixture::new();
    let mut content = "schema_version=1\n#".to_owned();
    content.push_str(&"x".repeat(deployment::MAX_FILE_BYTES - content.len()));
    f.write("max.toml", &content);
    assert!(deployment::resolve(f.document(json!({"imports":["max.toml"]}))).is_ok());
    content.push('x');
    f.write("max.toml", &content);
    error(f.document(json!({"imports":["max.toml"]})), "config_limit");
    for i in 0..17 {
        f.write(
            &format!("depth{i}.toml"),
            &format!(
                "schema_version=1\n{}",
                if i < 16 {
                    format!("imports=['depth{}.toml']\n", i + 1)
                } else {
                    String::new()
                }
            ),
        );
    }
    error(f.request("depth0.toml"), "config_limit");
    let mut profiles = serde_json::Map::new();
    for i in 0..17 {
        profiles.insert(
            format!("p{i}"),
            if i < 16 {
                json!({"extends":[format!("p{}",i+1)]})
            } else {
                json!({})
            },
        );
    }
    error(
        f.document(json!({"profile":"p0","profiles":profiles})),
        "config_limit",
    );
}

#[test]
fn aggregate_file_bytes_and_file_count_accept_exact_bounds() {
    let f = Fixture::new();
    let credentials = "[credentials.gateway]\nconsumer='provider.vercel'\nsources=[{kind='host',name='synthetic'}]\n";
    // Nine individually smaller files isolate the aggregate byte bound.
    let imports: Vec<_> = (1..9).map(|i| format!("bytes{i}.toml")).collect();
    for i in 0..9 {
        let mut text = if i == 0 {
            format!(
                "schema_version=1\nimports={}\n{credentials}#",
                serde_json::to_string(&imports).unwrap()
            )
        } else {
            "schema_version=1\n#".into()
        };
        let size =
            deployment::MAX_INPUT_BYTES / 9 + usize::from(i < deployment::MAX_INPUT_BYTES % 9);
        text.push_str(&"x".repeat(size - text.len()));
        f.write(&format!("bytes{i}.toml"), &text);
    }
    assert!(deployment::resolve(f.request("bytes0.toml")).is_ok());
    let path = f.0.join("bytes8.toml");
    fs::write(&path, format!("{}x", fs::read_to_string(&path).unwrap())).unwrap();
    error(f.request("bytes0.toml"), "config_limit");

    for i in 0..62 {
        f.write(&format!("leaf{i}.toml"), "schema_version=1\n");
    }
    for (group, range) in [(0, 0..31), (1, 31..61)] {
        let imports: Vec<_> = range.map(|i| format!("leaf{i}.toml")).collect();
        f.write(
            &format!("group{group}.toml"),
            &format!(
                "schema_version=1\nimports={}\n",
                serde_json::to_string(&imports).unwrap()
            ),
        );
    }
    f.write(
        "files.toml",
        &format!("schema_version=1\nimports=['group0.toml','group1.toml']\n{credentials}"),
    );
    assert_eq!(
        deployment::resolve(f.request("files.toml"))
            .unwrap()
            .sources()
            .iter()
            .filter(|s| s.kind == "file")
            .count(),
        64
    );
    let imports: Vec<_> = (31..62).map(|i| format!("leaf{i}.toml")).collect();
    f.write(
        "group1.toml",
        &format!(
            "schema_version=1\nimports={}\n",
            serde_json::to_string(&imports).unwrap()
        ),
    );
    error(f.request("files.toml"), "config_limit");
    for count in [32, 33] {
        let imports: Vec<_> = (0..count).map(|i| format!("leaf{i}.toml")).collect();
        let request = f.document(json!({"imports":imports}));
        if count == 32 {
            assert!(deployment::resolve(request).is_ok());
        } else {
            error(request, "config_limit");
        }
    }
}

#[test]
fn profile_depth_and_total_expansion_accept_exact_bounds() {
    let f = Fixture::new();
    for depth in [16, 17] {
        let profiles: serde_json::Map<_, _> = (0..depth)
            .map(|i| {
                (
                    format!("p{i}"),
                    if i + 1 < depth {
                        json!({"extends":[format!("p{}",i+1)]})
                    } else {
                        json!({})
                    },
                )
            })
            .collect();
        let request = f.document(json!({"profile":"p0","profiles":profiles}));
        if depth == 16 {
            assert!(deployment::resolve(request).is_ok());
        } else {
            error(request, "config_limit");
        }
    }
    let mut profiles = serde_json::Map::new();
    let branches: Vec<_> = (0..7).map(|i| format!("branch{i}")).collect();
    profiles.insert("selected".into(), json!({"extends":branches}));
    for i in 0..7 {
        let leaves: Vec<_> = (0..8).map(|j| format!("leaf{i}_{j}")).collect();
        profiles.insert(format!("branch{i}"), json!({"extends":leaves}));
        for name in leaves {
            profiles.insert(name, json!({}));
        }
    }
    assert_eq!(
        deployment::resolve(f.document(json!({"profile":"selected","profiles":profiles})))
            .unwrap()
            .sources()
            .iter()
            .filter(|s| s.kind == "profile")
            .count(),
        64
    );
    profiles.insert("one_more".into(), json!({}));
    error(f.document(json!({"profiles":profiles})), "config_limit");
}

#[test]
fn credential_binding_authority_and_rule_budgets_are_enforced() {
    let f = Fixture::new();
    for count in [64, 65] {
        let credentials: serde_json::Map<_, _> = (0..count).map(|i| (if i == 0 {"gateway".into()} else {format!("key{i}")}, json!({"consumer":"provider.vercel","sources":[{"kind":"host","name":"synthetic"}]}))).collect();
        let request = f.document(json!({"credentials":credentials}));
        if count == 64 {
            assert!(deployment::resolve(request).is_ok());
        } else {
            error(request, "config_limit");
        }
        let mut request = f.document(json!({}));
        for i in 1..count {
            request
                .path_bindings
                .insert(format!("binding{i}"), f.0.clone());
        }
        if count == 64 {
            assert!(deployment::resolve(request).is_ok());
        } else {
            error(request, "config_limit");
        }
        let authority: Vec<_> = (0..count)
            .map(|i| json!({"id":format!("ceiling.{i}")}))
            .collect();
        let request = f.document(json!({"authority":authority}));
        if count == 64 {
            assert!(deployment::resolve(request).is_ok());
        } else {
            error(request, "config_limit");
        }
    }
    for count in [8, 9] {
        let sources: Vec<_> = (0..count)
            .map(|i| json!({"kind":"host","name":format!("source{i}")}))
            .collect();
        let request = f.document(
            json!({"credentials":{"gateway":{"consumer":"provider.vercel","sources":sources}}}),
        );
        if count == 8 {
            assert!(deployment::resolve(request).is_ok());
        } else {
            error(request, "config_limit");
        }
    }
    let mut authority: Vec<_> = (0..8)
        .map(|i| {
            let deny: Vec<_> = (0..128)
                .map(|j| json!({"id":format!("rule.{i}.{j}"),"value":"fs.write"}))
                .collect();
            json!({"id":format!("ceiling.{i}"),"policy":{"tools":{"default":"allow","deny":deny}}})
        })
        .collect();
    assert!(deployment::resolve(f.document(json!({"authority":authority}))).is_ok());
    authority.push(json!({"id":"extra","policy":{"tools":{"default":"allow","deny":[{"id":"extra.rule","value":"fs.write"}]}}}));
    error(f.document(json!({"authority":authority})), "config_limit");
}

#[test]
fn file_depth_hardlink_and_format_boundaries() {
    let f = Fixture::new();
    f.corpus();
    for depth in [16, 17] {
        for i in 0..depth {
            f.write(&format!("d{i}.toml"), &format!("schema_version=1\n{}{}", if i + 1 < depth {format!("imports=['d{}.toml']\n",i+1)} else {String::new()}, if i == 0 {"[credentials.gateway]\nconsumer='provider.vercel'\nsources=[{kind='host',name='synthetic'}]\n"} else {""}));
        }
        if depth == 16 {
            assert!(deployment::resolve(f.request("d0.toml")).is_ok());
        } else {
            error(f.request("d0.toml"), "config_limit");
        }
    }
    fs::hard_link(f.0.join("modules/base.toml"), f.0.join("hard.toml")).unwrap();
    error(
        f.document(json!({"imports":["modules/base.toml","hard.toml"],"credentials":{}})),
        "config_duplicate_import",
    );
    let lf = deployment::resolve(f.request("production.toml")).unwrap();
    let text = fs::read_to_string(f.0.join("production.toml")).unwrap();
    f.write("production.toml", &text.replace('\n', "\r\n"));
    let crlf = deployment::resolve(f.request("production.toml")).unwrap();
    assert_eq!(lf.fingerprint(), crlf.fingerprint());
    assert_ne!(lf.input_fingerprint(), crlf.input_fingerprint());
    for text in ["\u{feff}schema_version=1", "schema_version=1\n#\0"] {
        f.write("invalid.toml", text);
        error(f.request("invalid.toml"), "config_parse");
    }
    fs::write(f.0.join("invalid.toml"), [0xff, 0xfe]).unwrap();
    error(f.request("invalid.toml"), "config_parse");
}

#[test]
fn typed_and_file_inputs_have_equal_effective_identity() {
    let f = Fixture::new();
    f.corpus();
    let file = deployment::resolve(f.request("production.toml")).unwrap();
    let mut request=f.document(json!({"deployment":file.config()["deployment"],"options":file.config()["options"],"authority":file.config()["authority"],"credentials":file.config()["credentials"]}));
    request.locked = true;
    let typed = deployment::resolve(request).unwrap();
    assert_eq!(file.config(), typed.config());
    assert_eq!(file.fingerprint(), typed.fingerprint());
    assert_ne!(file.input_fingerprint(), typed.input_fingerprint());
}

#[test]
fn ambient_process_state_cannot_change_locked_resolution() {
    let f = Fixture::new();
    f.corpus();
    let expected = deployment::resolve(f.request("production.toml")).unwrap();
    let outside = Fixture::new();
    outside.write(".env", "PRIVATE-UNREAD-AMBIENT");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(outside.0.join(".env"), fs::Permissions::from_mode(0o0)).unwrap();
    }
    outside.write("user.toml", "not toml");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "environment_child_probe", "--nocapture"])
        .current_dir(&outside.0)
        .env("PABLO_CONFIG_TEST_ROOT", &f.0)
        .env("PABLO_CONFIG_TEST_EXPECTED", expected.fingerprint())
        .env("PABLO_CONFIG_TEST_INPUT", expected.input_fingerprint())
        .env("HOME", &outside.0)
        .env("XDG_CONFIG_HOME", &outside.0)
        .env("OTEL_TRACES_EXPORTER", "otlp")
        .env("AI_GATEWAY_API_KEY", "PRIVATE-UNREAD-PROCESS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
#[test]
fn environment_child_probe() {
    let Ok(root) = std::env::var("PABLO_CONFIG_TEST_ROOT") else {
        return;
    };
    let mut request = ResolveRequest::new(Path::new(&root).to_path_buf(), "production.toml");
    request
        .path_bindings
        .insert("workspace".into(), root.into());
    let result = deployment::resolve(request).unwrap();
    assert_eq!(
        result.fingerprint(),
        std::env::var("PABLO_CONFIG_TEST_EXPECTED").unwrap()
    );
    assert_eq!(
        result.input_fingerprint(),
        std::env::var("PABLO_CONFIG_TEST_INPUT").unwrap()
    );
}

#[cfg(unix)]
#[test]
fn prepared_trace_creation_rejects_parent_swaps_and_existing_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let outside = Fixture::new();
    fs::create_dir(f.0.join("traces")).unwrap();
    let resolved = f.resolve(
        json!({"options":{"trace":{"path":{"base":"config","path":"traces/trace.jsonl"}}}}),
    );
    let prepared = resolved
        .prepare_run(deployment::RunInput {
            input: "synthetic".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap();
    fs::rename(f.0.join("traces"), f.0.join("saved")).unwrap();
    symlink(&outside.0, f.0.join("traces")).unwrap();
    assert_eq!(
        prepared.create_trace_file().unwrap_err().code,
        "config_path_unavailable"
    );
    assert!(!outside.0.join("trace.jsonl").exists());
    fs::remove_file(f.0.join("traces")).unwrap();
    fs::rename(f.0.join("saved"), f.0.join("traces")).unwrap();
    let file = prepared.create_trace_file().unwrap().unwrap();
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    drop(file);
    fs::write(f.0.join("traces/trace.jsonl"), "existing evidence").unwrap();
    assert_eq!(
        prepared.create_trace_file().unwrap_err().code,
        "config_path_unavailable"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("traces/trace.jsonl")).unwrap(),
        "existing evidence"
    );
}

#[test]
fn shell_configuration_roundtrips_composes_and_preserves_authority() {
    let f = Fixture::new();
    let rule = json!({"id":"ordinary.printf","executable":"/usr/bin/printf","args":["%s"],"match":"prefix"});
    let mut request=f.document(json!({
        "options":{"shell":{"commands":{"default":"deny","allow":[rule]},
            "environment":{"values":{"PABLO_TASK_A":"default"}},
            "cwd_roots":{"default":"deny","allow":[{"id":"cwd.base","value":"."}]}}},
        "profiles":{"selected":{"options":{"shell":{"cwd_roots":{"default":"deny","allow":{"mode":"append","items":[{"id":"cwd.profile","value":"nested"}]}}}}}},
        "authority":[{"id":"root","shell":{"commands":{"default":"allow","deny":[{"id":"root.push","executable":"/usr/bin/git","args":["push"],"match":"prefix"}]},
            "environment":{"allowed_names":["PABLO_TASK_A"]}}}]
    }));
    request.profile = Some("selected".into());
    let resolved = deployment::resolve(request).unwrap();
    assert_eq!(
        resolved.config()["options"]["shell"]["cwd_roots"]["allow"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        resolved.config()["options"]["shell"]["commands"]["deny"],
        json!([])
    );
    assert_eq!(
        resolved.config()["authority"][0]["shell"]["commands"]["allow"],
        json!([])
    );
    let prepared = resolved
        .prepare_run(deployment::RunInput {
            input: "synthetic".into(),
            workspace: Some(f.0.clone()),
            session_id: None,
        })
        .unwrap();
    assert!(
        prepared.tools().unwrap().descriptors()[0]
            .description
            .contains("literal")
    );
    f.write("rendered.toml", &resolved.render().unwrap());
    let reloaded = deployment::resolve(f.request("rendered.toml")).unwrap();
    assert_eq!(resolved.fingerprint(), reloaded.fingerprint());
    assert_eq!(resolved.config(), reloaded.config());
    let error = deployment::resolve(f.document(json!({
        "options":{"shell":{"environment":{"values":{"PABLO_TASK_SECRET":"private-value"}}}},
        "authority":[{"id":"root","shell":{"environment":{"allowed_names":[]}}}]
    })))
    .unwrap_err();
    assert_eq!(error.code, "config_authority_violation");
    assert_eq!(error.authority_id.as_deref(), Some("root"));
    assert!(!error.to_string().contains("private-value"));
}

#[test]
fn shell_configuration_rejects_unknown_invalid_inactive_and_duplicate_rules() {
    let f = Fixture::new();
    let good = json!({"id":"valid","executable":"/usr/bin/printf","args":[],"match":"exact"});
    for (key, value) in [
        ("id", json!("builtin.bad")),
        ("executable", json!("printf")),
        ("match", json!("substring")),
        ("args", json!(["é".repeat(4097)])),
        ("args", json!(["a\nb"])),
        ("extra", json!(true)),
    ] {
        let mut rule = good.clone();
        rule[key] = value;
        let bad=f.document(json!({"profiles":{"inactive":{"options":{"shell":{"commands":{"default":"deny","allow":[rule]}}}}}}));
        assert!(deployment::resolve(bad).is_err(), "{key}");
    }
    for shell in [
        json!({"environment":{"values":{"AI_GATEWAY_API_KEY":"private"}}}),
        json!({"environment":{"allowed_names":["PABLO_TASK_A","PABLO_TASK_A"]}}),
        json!({"environment":{"values":{"PABLO_TASK_A":"x\u{0000}y"}}}),
        json!({"cwd_roots":{"default":"allow","allow":[{"id":"root","value":"../escape"}]}}),
    ] {
        assert!(deployment::resolve(f.document(json!({"options":{"shell":shell}}))).is_err());
    }
    assert!(deployment::resolve(f.document(json!({"authority":[{"id":"root","shell":{"environment":{"values":{"PABLO_TASK_A":"x"}}}}]}))).is_err());
    error(f.document(json!({"options":{"policy":{"tools":{"default":"allow","allow":[{"id":"valid","value":"shell.run"}]}},"shell":{"commands":{"default":"deny","allow":[good.clone()]}}}})),"config_conflict");
    error(f.document(json!({"options":{"shell":{"commands":{"default":"deny","allow":[good.clone()]}}},"authority":[{"id":"root","shell":{"commands":{"default":"deny","allow":[good]}}}]})),"config_conflict");
}

#[test]
fn provider_defaults_metadata_authority_and_admission_are_shared() {
    use pablo_core::gateway::{GatewayKind, ModelProfile};
    let f = Fixture::new();
    for kind in [GatewayKind::Vercel, GatewayKind::Openrouter] {
        let consumer = if kind == GatewayKind::Vercel {
            "provider.vercel"
        } else {
            "provider.openrouter"
        };
        let resolved=f.resolve(json!({"options":{"model":{"provider":kind.name()}},"credentials":{"gateway":{"consumer":consumer,"sources":[{"kind":"environment","name":"UNREAD_SYNTHETIC_KEY"}]}}}));
        assert_eq!(
            resolved.model_profile().unwrap(),
            ModelProfile::resolve(kind, None).unwrap()
        );
        assert_eq!(resolved.options()["model"]["id"], kind.default_model());
        assert_eq!(resolved.options()["model"]["endpoint"], kind.endpoint());
        f.write("rendered.toml", &resolved.render().unwrap());
        assert_eq!(
            deployment::resolve(f.request("rendered.toml"))
                .unwrap()
                .fingerprint(),
            resolved.fingerprint()
        );
        let admitted = resolved.prepare_run(deployment::RunInput {
            input: "synthetic".into(),
            workspace: Some(f.0.clone()),
            session_id: None,
        });
        assert_eq!(admitted.unwrap().spec().model, kind.default_model());
        assert!(
            !resolved
                .model_profile()
                .unwrap()
                .capabilities
                .hard_accounting_bounds
        );
    }
    f.write(
        "openrouter.toml",
        include_str!("../../../docs/project/fixtures/c3-openrouter.toml"),
    );
    let file = deployment::resolve(f.request("openrouter.toml")).unwrap();
    assert_eq!(file.model_profile().unwrap().model, "z-ai/glm-5.3-flash");
    error(
        f.document(json!({"options":{"model":{"provider":"openrouter"}}})),
        "config_invalid_value",
    );
    error(f.document(json!({"options":{"model":{"endpoint":"https://openrouter.ai/api/v1/chat/completions"}}})),"config_invalid_value");
    error(
        f.document(json!({"options":{"model":{"endpoint":"https://private.invalid/private"}}})),
        "config_invalid_value",
    );
    for name in [
        "OPENROUTER_API_KEY",
        "AI_GATEWAY_API_KEY",
        "VERCEL_AI_GATEWAY",
    ] {
        error(
            f.document(json!({"environment":{name:{"option":"otel.service_name"}}})),
            "config_invalid_value",
        );
    }
    let resolved=f.resolve(json!({"authority":[{"id":"provider.ceiling","provider_endpoints":["https://ai-gateway.vercel.sh/v1/chat/completions"]}]}));
    // Explicit endpoint replacement cannot cross the original authority.
    let mut overrides = serde_json::Map::new();
    overrides.insert(
        "model.endpoint".into(),
        json!("https://openrouter.ai/api/v1/chat/completions"),
    );
    assert!(resolved.with_overrides(overrides).is_err());
}

#[test]
fn provider_switches_recompute_only_defaults_and_preserve_explicit_values_and_authority() {
    let f = Fixture::new();
    let credentials = json!({"gateway":{"consumer":"provider.vercel","sources":[{"kind":"environment","name":"VERCEL_SYNTHETIC"}]},"router":{"consumer":"provider.openrouter","sources":[{"kind":"environment","name":"ROUTER_SYNTHETIC"}]}});
    let configured = f.resolve(json!({"credentials":credentials}));
    let overrides = serde_json::Map::from_iter([
        ("model.provider".into(), json!("openrouter")),
        ("model.credential".into(), json!("router")),
    ]);
    let router = configured.with_overrides(overrides.clone()).unwrap();
    assert_eq!(router.options()["model"]["id"], "z-ai/glm-5.3-flash");
    assert_eq!(
        router.options()["model"]["endpoint"],
        pablo_core::gateway::OPENROUTER_ENDPOINT
    );
    let vercel = router
        .with_overrides(serde_json::Map::from_iter([
            ("model.provider".into(), json!("vercel")),
            ("model.credential".into(), json!("gateway")),
        ]))
        .unwrap();
    assert_eq!(vercel.options()["model"]["id"], "zai/glm-5.3-flash");
    let configured =
        f.resolve(json!({"credentials":credentials,"options":{"model":{"id":"explicit/model"}}}));
    assert_eq!(
        configured
            .with_overrides(overrides.clone())
            .unwrap()
            .options()["model"]["id"],
        "explicit/model"
    );
    let configured=f.resolve(json!({"credentials":credentials,"options":{"model":{"endpoint":pablo_core::gateway::VERCEL_ENDPOINT}}}));
    assert_eq!(
        configured
            .with_overrides(overrides.clone())
            .unwrap_err()
            .code,
        "config_invalid_value"
    );
    let configured=f.resolve(json!({"credentials":credentials,"authority":[{"id":"host","provider_endpoints":[pablo_core::gateway::VERCEL_ENDPOINT]}]}));
    let error = configured.with_overrides(overrides).unwrap_err();
    assert_eq!(error.code, "config_authority_violation");
    assert_eq!(error.authority_id.as_deref(), Some("host"));
}

#[test]
fn output_repair_admission_requires_schema_and_locked_overrides_only_narrow() {
    let f = Fixture::new();
    error(
        f.document(json!({"options":{"output":{"repair":{"enabled":true}}}})),
        "config_invalid_value",
    );
    for bad in [511, 4097] {
        error(f.document(json!({"options":{"output":{"schema":"true","repair":{"enabled":true,"max_feedback_bytes":bad}}}})),"config_invalid_value");
    }
    for (enabled, option, value, allowed) in [
        (false, "output.schema", json!("true"), true),
        (false, "output.schema", json!("false"), false),
        (false, "output.max_validation_work", json!(1), true),
        (
            false,
            "output.max_validation_work",
            json!(1000000000),
            false,
        ),
        (false, "output.repair.enabled", json!(true), false),
        (true, "output.repair.enabled", json!(false), true),
        (true, "output.repair.max_feedback_bytes", json!(512), true),
        (true, "output.repair.max_feedback_bytes", json!(4096), false),
    ] {
        let mut request=f.document(json!({"deployment":{"locked":true,"allowed_run_overrides":["output.schema","output.max_validation_work","output.repair.enabled","output.repair.max_feedback_bytes"]},"options":{"output":{"schema":"true","repair":{"enabled":enabled,"max_feedback_bytes":1024}}}}));
        request.overrides.insert(option.into(), value);
        if allowed {
            deployment::resolve(request).unwrap();
        } else {
            error(request, "config_authority_violation");
        }
    }
}

#[test]
fn mcp_configuration_preserves_defaults_provenance_and_private_references() {
    let f = Fixture::new();
    let document = json!({"options":{"mcp":{"servers":{
        "local":{"transport":"stdio","command":"/usr/bin/printf","env":{"TOKEN":"local-key"}},
        "remote":{"transport":"http","url":"https://example.test/mcp","required":false}
    }}},"credentials":{
        "gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"gateway"}]},
        "local-key":{"consumer":"mcp.env","sources":[{"kind":"file","path":{"base":"config","path":"absent-secret"},"encoding":"utf8"}]}
    }});
    let resolved = f.resolve(document.clone());
    let exposed = serde_json::to_value(&resolved).unwrap();
    assert_eq!(
        exposed["provenance"]["/config/options/mcp/servers/local/required"][0]["source"],
        "source-0000"
    );
    assert_ne!(
        exposed["provenance"]["/config/options/mcp/servers/local/command"][0]["source"],
        "source-0000"
    );
    assert!(resolved.mcp().unwrap().servers["local"].required());
    assert!(!resolved.mcp().unwrap().servers["remote"].required());
    f.write("rendered-mcp.toml", &resolved.render().unwrap());
    let reloaded = deployment::resolve(f.request("rendered-mcp.toml")).unwrap();
    assert_eq!(reloaded.fingerprint(), resolved.fingerprint());
    let e = resolved
        .prepare_run(deployment::RunInput {
            input: "synthetic".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap()
        .tools()
        .err()
        .unwrap();
    assert_eq!(e.code, "config_async_mcp_required");
    let mut wrong = document.clone();
    wrong["credentials"]["local-key"]["consumer"] = "mcp.headers".into();
    error(f.document(wrong), "config_invalid_value");
    let mut request = f.document(document);
    request
        .host_authority
        .push(json!({"id":"host","credential_ids":["gateway"]}));
    error(request, "config_authority_violation");
}

#[test]
fn mcp_host_ceilings_survive_rendering_and_optional_denials_do_not_launch() {
    let f = Fixture::new();
    for required in [true, false] {
        let mut request = f.document(json!({"options":{"mcp":{"servers":{"local":{
            "transport":"stdio","command":"/not-installed/never-launch","required":required
        }}}}}));
        request.host_authority.push(json!({"id":"host","mcp":{"servers":{"default":"allow","deny":[{"id":"host.mcp.no","value":"local"}]}}}));
        let resolved = deployment::resolve(request).unwrap();
        assert_eq!(resolved.options()["mcp"]["policies"], json!([]));
        assert!(
            resolved
                .mcp()
                .unwrap()
                .admit_server("local", &Default::default())
                .is_err()
        );
        f.write("denied-mcp.toml", &resolved.render().unwrap());
        let reloaded = deployment::resolve(f.request("denied-mcp.toml")).unwrap();
        assert_eq!(reloaded.fingerprint(), resolved.fingerprint());
        assert!(
            reloaded
                .mcp()
                .unwrap()
                .admit_server("local", &Default::default())
                .is_err()
        );
        let prepared = resolved.prepare_run(deployment::RunInput {
            input: "synthetic".into(),
            session_id: None,
            workspace: None,
        });
        if required {
            assert_eq!(prepared.unwrap_err().code, "config_authority_violation");
        } else {
            prepared.unwrap();
        }
    }
    let mut request = f.document(json!({"options":{"mcp":{"servers":{"remote":{"transport":"http","url":"https://example.test/mcp"}}}}}));
    request.host_authority.push(
        json!({"id":"host.tools","tool_names":["shell.run","fs.read","fs.list","fs.search"]}),
    );
    let resolved = deployment::resolve(request).unwrap();
    let denied = resolved.admit_mcp_tool("remote", "read").unwrap_err();
    assert_eq!(denied.authority_id.as_deref(), Some("host.tools"));
}

#[test]
fn mcp_server_replacement_cannot_inherit_stale_arguments_or_credentials() {
    let f = Fixture::new();
    let mut request = f.document(json!({"options":{"mcp":{"servers":{"local":{
        "transport":"stdio","command":"/bin/true"
    }}}}}));
    request.user_config = Some(ConfigInput::Document(
        json!({"schema_version":1,"options":{"mcp":{"servers":{"local":{
            "transport":"stdio","command":"/bin/echo","args":["old-argument"],"env":{"TOKEN":"undeclared-stale-secret"}
        }}}}}),
    ));
    let resolved = deployment::resolve(request).unwrap();
    assert_eq!(
        resolved.options()["mcp"]["servers"]["local"]["args"],
        json!([])
    );
    assert_eq!(
        resolved.options()["mcp"]["servers"]["local"]["env"],
        json!({})
    );
    assert_eq!(
        resolved.options()["mcp"]["servers"]["local"]["command"],
        "/bin/true"
    );
}

#[test]
fn mcp_tool_admission_always_uses_deployment_policy() {
    let f = Fixture::new();
    let resolved = f.resolve(json!({"options":{
        "mcp":{"servers":{"remote":{"transport":"http","url":"https://example.test/mcp"}}},
        "policy":{"tools":{"default":"allow","deny":[{"id":"host.tool.no","value":"mcp/remote/read"}]}}
    }}));
    assert!(resolved.admit_mcp_tool("remote", "read").is_err());
    assert!(resolved.admit_mcp_tool("remote", "other").is_ok());
}
