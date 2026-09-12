use pablo_core::deployment::{self, ConfigInput, ResolveRequest, RunInput};
use serde_json::{Value, json};
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("pablo-routes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
    fn resolve(
        &self,
        mut value: Value,
    ) -> Result<deployment::ResolvedDeployment, deployment::ConfigError> {
        value["schema_version"] = 1.into();
        let mut r = ResolveRequest::new(self.0.clone(), "unused.toml");
        r.entry = ConfigInput::Document(value);
        r.path_bindings.insert("workspace".into(), self.0.clone());
        deployment::resolve(r)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn preset() -> Value {
    json!({"options":{"models":{
        "primary":{"provider":"vercel","id":"zai/glm-5.3-flash","credential":"vercel"},
        "secondary":{"provider":"openrouter","id":"z-ai/glm-5.3-flash","credential":"router","model_options":{"max_output_tokens":2048}},
        "third":{"provider":"open_responses","id":"fixture-text-tools-v1","endpoint":"https://responses.example.test/v1/responses","capability_profile":"open-responses-text-tools-v1","credential":"responses"}},
        "routes":{"tail":{"entries":[{"model":"secondary"},{"model":"third"}]},"main":{"entries":[{"model":"primary"},{"route":"tail"}]}},"model_route":"main"},
        "credentials":{"vercel":{"consumer":"provider.vercel","sources":[{"kind":"environment","name":"VERCEL_ROUTE_KEY"}]},"router":{"consumer":"provider.openrouter","sources":[{"kind":"environment","name":"ROUTER_ROUTE_KEY"}]},"responses":{"consumer":"provider.open_responses","sources":[{"kind":"environment","name":"RESPONSES_ROUTE_KEY"}]}}})
}
#[test]
fn ordered_profiles_resolve_exact_identity_options_and_scoped_credentials_offline() {
    let f = Fixture::new();
    let resolved = f.resolve(preset()).unwrap();
    let route = resolved.model_route().unwrap();
    assert_eq!(
        route.entries().iter().map(|e| e.name()).collect::<Vec<_>>(),
        ["primary", "secondary", "third"]
    );
    assert_eq!(
        route.entries()[0].profile().endpoint,
        "https://ai-gateway.vercel.sh/v1/chat/completions"
    );
    assert_eq!(route.entries()[1].profile().model, "z-ai/glm-5.3-flash");
    assert_eq!(route.entries()[1].max_output_tokens(), 2048);
    assert_eq!(route.entries()[2].credential(), "responses");
    assert_eq!(
        route.entries()[2].profile().protocol,
        "open-responses-http-sse"
    );
    assert_eq!(route.policy().max_attempts, 3);
    assert!(route.policy().sticky);
    assert_eq!(route.policy().retry_owner, "pablo");
    assert!(route.execution_available());
    assert_eq!(
        resolved.model_profile().unwrap(),
        *route.entries()[0].profile()
    );
    assert!(!resolved.provenance()["/config/options/models/primary/endpoint"].is_empty());
    let inspected = serde_json::to_value(&resolved).unwrap();
    assert_eq!(inspected["model_route"]["selected_entry"], "primary");
    assert!(
        resolved
            .prepare_run(RunInput {
                input: "task".into(),
                session_id: None,
                workspace: None
            })
            .is_ok()
    );
    let rendered = resolved.render().unwrap();
    let value: Value = toml::from_str(&rendered).unwrap();
    let reloaded = f.resolve(value).unwrap();
    assert_eq!(reloaded.fingerprint(), resolved.fingerprint());
    assert_eq!(reloaded.model_route(), resolved.model_route());
}
#[test]
fn invalid_route_graphs_requirements_and_profiles_reject_without_partial_selection() {
    let f = Fixture::new();
    for case in 0..13 {
        let mut value = preset();
        match case {
            0 => value["options"]["model_route"] = "missing".into(),
            1 => value["options"]["routes"]["main"]["entries"][0]["model"] = "missing".into(),
            2 => value["options"]["routes"]["tail"]["entries"] = json!([{"route":"main"}]),
            3 => {
                value["options"]["routes"]["main"]["entries"] =
                    json!([{"route":"tail"},{"route":"tail"}])
            }
            4 => {
                value["options"]["routes"]["main"]["entries"] =
                    json!([{"model":"primary"},{"model":"primary"}])
            }
            5 => {
                value["options"]["routes"]["main"]["required_capabilities"] =
                    json!(["structured_output"])
            }
            6 => {
                value["options"]["models"]["secondary"]["capabilities"] =
                    json!({"tool_calls":false})
            }
            7 => value["credentials"]["router"]["consumer"] = "provider.vercel".into(),
            8 => value["options"]["routes"]["main"]["max_attempts"] = 4.into(),
            9 => value["options"]["routes"]["main"]["retry_owner"] = "gateway".into(),
            10 => {
                value["options"]["models"]["secondary"]["endpoint"] =
                    "https://other.example.test/completions".into()
            }
            11 => {
                value["options"]["routes"]["main"]["eligible_errors"] =
                    json!(["transport_uncertain"])
            }
            12 => {
                value["options"]["models"]["third"]["model_options"] =
                    json!({"max_output_tokens":15})
            }
            _ => unreachable!(),
        }
        assert!(f.resolve(value).is_err(), "case {case}");
    }
}
#[test]
fn inherited_subsequences_cannot_reorder_replace_repeat_or_append_destinations() {
    let f = Fixture::new();
    let original = f.resolve(preset()).unwrap();
    let mut configured = preset();
    configured["options"]["models"]["secondary"]["model_options"]["reasoning"] = "low".into();
    configured["options"]["models"]["third"]["model_options"] = json!({"reasoning":"high"});
    let resolved = f.resolve(configured).unwrap();
    assert_ne!(resolved.fingerprint(), original.fingerprint());
    let route = resolved.model_route().unwrap();
    let child = route.subsequence(&["secondary", "third"]).unwrap();
    assert_eq!(child.entries(), &route.entries()[1..]);
    assert_eq!(
        child.entries()[0].profile().reasoning,
        pablo_core::ReasoningConfig::Effort(pablo_core::ReasoningEffort::Low)
    );
    assert_eq!(
        child.entries()[1].profile().reasoning,
        pablo_core::ReasoningConfig::Effort(pablo_core::ReasoningEffort::High)
    );
    assert_eq!(child.policy().max_attempts, 2);
    for names in [
        vec![],
        vec!["third", "secondary"],
        vec!["primary", "primary"],
        vec!["new-destination"],
    ] {
        assert!(route.subsequence(&names).is_err());
    }
    let one = route.subsequence(&["third"]).unwrap();
    assert!(one.execution_available());
    assert_eq!(one.policy().max_attempts, 1);
}
#[test]
fn every_route_entry_obeys_host_authority_and_single_entry_uses_selected_model() {
    let f = Fixture::new();
    for (field, allowed) in [
        ("model_ids", json!(["zai/glm-5.3-flash"])),
        (
            "provider_endpoints",
            json!(["https://ai-gateway.vercel.sh/v1/chat/completions"]),
        ),
        ("credential_ids", json!(["vercel"])),
    ] {
        let mut value = preset();
        value["authority"] = json!([{"id":"host.route","model_ids":["zai/glm-5.3-flash","z-ai/glm-5.3-flash","fixture-text-tools-v1"]}]);
        value["authority"][0][field] = allowed;
        let error = f.resolve(value).err().unwrap();
        assert_eq!(error.code, "config_authority_violation");
        assert_eq!(error.authority_id.as_deref(), Some("host.route"));
    }
    let mut value = preset();
    value["options"]["model_route"] = "single".into();
    value["options"]["routes"]["single"] = json!({"entries":[{"model":"secondary"}]});
    value["options"]["limits"] = json!({"max_output_tokens":1024});
    let resolved = f.resolve(value).unwrap();
    let prepared = resolved
        .prepare_run(RunInput {
            input: "task".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap();
    assert_eq!(prepared.spec().model, "z-ai/glm-5.3-flash");
    assert_eq!(prepared.spec().limits.max_output_tokens, 1024);
}

#[test]
fn selected_profile_credential_lookup_and_attempt_deadline_admission_are_explicit() {
    use deployment::{CredentialConsumer, CredentialInputs, CredentialReadError};
    struct Keys;
    impl CredentialInputs for Keys {
        fn environment(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
            assert_eq!(name, "ROUTER_ROUTE_KEY");
            Ok(Some(b"private-route-token".to_vec()))
        }
    }
    let f = Fixture::new();
    let mut value = preset();
    value["options"]["routes"]["single"] = json!({"entries":[{"model":"secondary"}]});
    value["options"]["model_route"] = "single".into();
    let resolved = f.resolve(value.clone()).unwrap();
    let prepared = resolved
        .prepare_run(RunInput {
            input: "task".into(),
            session_id: None,
            workspace: None,
        })
        .unwrap();
    let lease = prepared
        .credential(CredentialConsumer::OpenRouter, &Keys)
        .unwrap()
        .unwrap();
    assert_eq!(
        lease
            .expose_for(
                CredentialConsumer::OpenRouter,
                "https://openrouter.ai/api/v1/chat/completions"
            )
            .unwrap(),
        "private-route-token"
    );
    assert!(
        prepared
            .credential(CredentialConsumer::Vercel, &Keys)
            .is_err()
    );
    assert!(!format!("{lease:?} {resolved:?}").contains("private-route-token"));
    value["options"]["routes"]["single"]["per_attempt_timeout_ms"] = 100.into();
    let resolved = f.resolve(value).unwrap();
    assert!(resolved.model_route().unwrap().execution_available());
    assert!(
        resolved
            .prepare_run(RunInput {
                input: "task".into(),
                session_id: None,
                workspace: None
            })
            .is_ok()
    );
}

#[test]
fn typed_overrides_refresh_derived_defaults_without_widening_explicit_profile_caps() {
    let f = Fixture::new();
    let resolved = f.resolve(preset()).unwrap();
    let changed = resolved.with_overrides(
        serde_json::from_value(json!({"limits.max_output_tokens":131072,"options_not_a_key":1}))
            .unwrap(),
    );
    assert!(changed.is_err());
    let changed = resolved
        .with_overrides(serde_json::from_value(json!({"limits.max_output_tokens":131072})).unwrap())
        .unwrap();
    let route = changed.model_route().unwrap();
    assert_eq!(route.entries()[0].max_output_tokens(), 131072);
    assert_eq!(route.entries()[1].max_output_tokens(), 2048);
    let changed=changed.with_overrides(serde_json::from_value(json!({"routes.main.entries":[{"model":"primary"}],"models.primary.provider":"openrouter","models.primary.id":"z-ai/glm-5.3-flash","models.primary.credential":"router"})).unwrap()).unwrap();
    let route = changed.model_route().unwrap();
    assert_eq!(route.policy().max_attempts, 1);
    assert!(route.execution_available());
    assert_eq!(
        route.entries()[0].profile().endpoint,
        "https://openrouter.ai/api/v1/chat/completions"
    );
}
