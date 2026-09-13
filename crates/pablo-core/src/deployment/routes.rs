//! Offline route composition and immutable inherited profile selection.
use super::*;
use crate::gateway::{GatewayKind, ModelProfile};
use serde_json::json;
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RouteEntry {
    pub(super) name: String,
    pub(super) profile: ModelProfile,
    pub(super) credential: String,
    pub(super) max_output_tokens: u32,
    upstream_routing: &'static str,
}
impl RouteEntry {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn profile(&self) -> &ModelProfile {
        &self.profile
    }
    pub fn credential(&self) -> &str {
        &self.credential
    }
    pub fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RoutePolicy {
    pub max_attempts: usize,
    pub per_attempt_timeout_ms: u64,
    pub eligible_errors: Vec<String>,
    pub retry_uncertain_delivery: bool,
    pub retry_owner: String,
    pub sticky: bool,
    pub required_capabilities: Vec<String>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResolvedRoute {
    name: String,
    entries: Vec<RouteEntry>,
    policy: RoutePolicy,
    selected_entry: String,
    selection_reason: &'static str,
    execution_available: bool,
    fallback_owner: &'static str,
}
impl ResolvedRoute {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }
    pub fn policy(&self) -> &RoutePolicy {
        &self.policy
    }
    pub fn execution_available(&self) -> bool {
        self.execution_available
    }
    /// Copy exact approved entries in their existing order, narrowing attempt
    /// capacity. No child-supplied model/endpoint/credential record is accepted.
    pub fn subsequence(&self, names: &[&str]) -> Result<Self, ConfigError> {
        if names.is_empty() || names.len() > self.entries.len() {
            return Err(error("config_authority_violation", "/model_route"));
        }
        let mut start = 0;
        let mut entries = Vec::new();
        for name in names {
            let offset = self.entries[start..]
                .iter()
                .position(|e| e.name == *name)
                .ok_or_else(|| error("config_authority_violation", "/model_route"))?;
            start += offset + 1;
            entries.push(self.entries[start - 1].clone());
        }
        let mut route = self.clone();
        route.selected_entry = entries[0].name.clone();
        route.policy.max_attempts = route.policy.max_attempts.min(entries.len());
        route.execution_available = true;
        route.entries = entries;
        route.selection_reason = "inherited_ordered_subsequence";
        Ok(route)
    }
}
fn invalid(path: &str) -> ConfigError {
    error("config_invalid_value", path)
}
fn catalog(config: &Value) -> bool {
    config["options"]["shell"]["enabled"] == true
        || config["options"]["filesystem"]["enabled"] == true
}

/// Defaults depend on the explicit provider/root limits. The resolver records
/// every added leaf separately as a derived default before fingerprinting.
pub(super) fn complete(config: &mut Value) -> Result<(), ConfigError> {
    let root_tokens = config["options"]["limits"]["max_output_tokens"].clone();
    if let Some(models) = config["options"].get_mut("models") {
        for (name, model) in models
            .as_object_mut()
            .ok_or_else(|| invalid("/options/models"))?
        {
            let path = format!("/options/models/{name}");
            let kind: GatewayKind = model["provider"]
                .as_str()
                .ok_or_else(|| invalid(&path))?
                .parse()
                .map_err(|_| invalid(&path))?;
            let map = model.as_object_mut().unwrap();
            if kind != GatewayKind::OpenResponses {
                map.entry("endpoint")
                    .or_insert_with(|| kind.endpoint().into());
            } else {
                map.entry("auth_header")
                    .or_insert_with(|| "Authorization".into());
                map.entry("auth_scheme").or_insert_with(|| "bearer".into());
            }
            map.entry("context_window_tokens")
                .or_insert_with(|| json!({"unset":true}));
            let options = map.entry("model_options").or_insert_with(|| json!({}));
            options
                .as_object_mut()
                .unwrap()
                .entry("max_output_tokens")
                .or_insert(root_tokens.clone());
            let caps = map.entry("capabilities").or_insert_with(|| json!({}));
            for key in ["text_streaming", "tool_calls"] {
                caps.as_object_mut()
                    .unwrap()
                    .entry(key)
                    .or_insert(true.into());
            }
        }
    }
    let tools = catalog(config);
    let duration = config["options"]["limits"]["max_run_duration_ms"].clone();
    let routes = config["options"]
        .get("routes")
        .cloned()
        .unwrap_or_else(|| json!({}));
    for name in routes
        .as_object()
        .ok_or_else(|| invalid("/options/routes"))?
        .keys()
    {
        let entries = expand(&routes, name)?;
        let defaults = json!({"max_attempts":entries.len(),"per_attempt_timeout_ms":duration,"eligible_errors":["not_sent","rate_limited","service_unavailable"],
            "retry_uncertain_delivery":false,"retry_owner":"pablo","sticky":true,
            "required_capabilities":if tools {vec!["text_streaming","tool_calls"]} else {vec!["text_streaming"]}});
        let target = config["options"]["routes"][name].as_object_mut().unwrap();
        for (key, value) in defaults.as_object().unwrap() {
            target.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    Ok(())
}
fn expand(routes: &Value, name: &str) -> Result<Vec<String>, ConfigError> {
    fn visit(
        routes: &Value,
        name: &str,
        active: &mut Vec<String>,
        seen: &mut HashSet<String>,
        models: &mut Vec<String>,
    ) -> Result<(), ConfigError> {
        if active.iter().any(|s| s == name) {
            return Err(error("config_cycle", "/options/routes")
                .with_chain(active.iter().map(String::as_str).chain([name])));
        }
        if active.len() >= 16 || !seen.insert(name.into()) {
            return Err(invalid("/options/routes"));
        }
        let route = routes.get(name).ok_or_else(|| invalid("/options/routes"))?;
        active.push(name.into());
        for entry in route["entries"]
            .as_array()
            .ok_or_else(|| invalid("/options/routes"))?
        {
            if let Some(model) = entry["model"].as_str() {
                if models.len() >= 32 || models.iter().any(|s| s == model) {
                    return Err(invalid("/options/routes"));
                }
                models.push(model.into());
            } else if let Some(route) = entry["route"].as_str() {
                visit(routes, route, active, seen, models)?;
            } else {
                return Err(invalid("/options/routes"));
            }
        }
        active.pop();
        Ok(())
    }
    let mut models = Vec::new();
    visit(
        routes,
        name,
        &mut Vec::new(),
        &mut HashSet::new(),
        &mut models,
    )?;
    if models.is_empty() {
        return Err(invalid("/options/routes"));
    }
    Ok(models)
}
fn entry(config: &Value, name: &str, model: &Value) -> Result<RouteEntry, ConfigError> {
    let path = format!("/options/models/{name}");
    let kind: GatewayKind = model["provider"]
        .as_str()
        .ok_or_else(|| invalid(&path))?
        .parse()
        .map_err(|_| invalid(&path))?;
    let mut profile = if kind == GatewayKind::OpenResponses {
        ModelProfile::responses(super::admission::responses_profile(model)?)
    } else {
        if model["endpoint"] != kind.endpoint()
            || ["auth_header", "auth_scheme", "capability_profile"]
                .iter()
                .any(|key| model.get(key).is_some())
        {
            return Err(invalid(&path));
        }
        let id = model["id"].as_str().ok_or_else(|| invalid(&path))?;
        ModelProfile::resolve(kind, Some(id)).map_err(|_| invalid(&path))?
    };
    profile.context_window_tokens = model["context_window_tokens"].as_u64();
    let credential = model["credential"].as_str().ok_or_else(|| invalid(&path))?;
    if config["credentials"][credential]["consumer"] != kind.credential_consumer().name() {
        return Err(invalid("/credentials"));
    }
    let tokens = model["model_options"]["max_output_tokens"]
        .as_u64()
        .ok_or_else(|| invalid(&path))?
        .min(
            config["options"]["limits"]["max_output_tokens"]
                .as_u64()
                .unwrap(),
        );
    if tokens == 0 || tokens > u32::MAX as u64 || kind == GatewayKind::OpenResponses && tokens < 16
    {
        return Err(invalid(&path));
    }
    reasoning_profile(&mut profile, model, tokens as u32)?;
    profile.capabilities.text_streaming &= model["capabilities"]["text_streaming"]
        .as_bool()
        .unwrap_or(true);
    profile.capabilities.tool_calls &= model["capabilities"]["tool_calls"]
        .as_bool()
        .unwrap_or(true);
    Ok(RouteEntry {
        name: name.into(),
        profile,
        credential: credential.into(),
        max_output_tokens: tokens as u32,
        upstream_routing: "opaque_to_pablo",
    })
}
pub(super) fn resolve(config: &Value) -> Result<Option<ResolvedRoute>, ConfigError> {
    let options = &config["options"];
    if ["models", "routes", "model_route"]
        .iter()
        .all(|key| options.get(key).is_none())
    {
        return Ok(None);
    }
    let mut models = BTreeMap::new();
    if let Some(catalog) = options["models"].as_object() {
        for (name, model) in catalog {
            models.insert(name.clone(), entry(config, name, model)?);
        }
    }
    let routes = options.get("routes").cloned().unwrap_or_else(|| json!({}));
    let mut selected = None;
    for (name, route) in routes.as_object().unwrap() {
        let mut entries = Vec::new();
        let mut identities = HashSet::new();
        for name in expand(&routes, name)? {
            let entry = models
                .get(&name)
                .ok_or_else(|| invalid("/options/routes"))?;
            let identity = fingerprint(
                &json!({"profile":entry.profile,"credential":entry.credential,"max_output_tokens":entry.max_output_tokens}),
            )?;
            if !identities.insert(identity) {
                return Err(invalid("/options/routes"));
            }
            entries.push(entry.clone());
        }
        let mut requirements: Vec<String> =
            serde_json::from_value(route["required_capabilities"].clone())
                .map_err(|_| invalid("/options/routes"))?;
        for required in if catalog(config) {
            vec!["text_streaming", "tool_calls"]
        } else {
            vec!["text_streaming"]
        } {
            if !requirements.iter().any(|value| value == required) {
                requirements.push(required.into());
            }
        }
        let maximum = route["max_attempts"].as_u64().unwrap() as usize;
        if maximum > entries.len() {
            return Err(invalid("/options/routes"));
        }
        let errors: Vec<String> = serde_json::from_value(route["eligible_errors"].clone())
            .map_err(|_| invalid("/options/routes"))?;
        let uncertain = route["retry_uncertain_delivery"].as_bool().unwrap();
        if errors.iter().any(|e| e == "transport_uncertain") && !uncertain {
            return Err(invalid("/options/routes"));
        }
        for entry in &entries {
            let caps = entry.profile.capabilities;
            if !caps.text_streaming
                || catalog(config) && !caps.tool_calls
                || requirements.iter().any(|r| match r.as_str() {
                    "text_streaming" => !caps.text_streaming,
                    "tool_calls" => !caps.tool_calls,
                    _ => true,
                })
            {
                return Err(invalid("/options/routes/required_capabilities"));
            }
            for ceiling in config["authority"].as_array().unwrap() {
                for (field, value) in [
                    ("model_ids", entry.profile.model.as_str()),
                    ("provider_endpoints", entry.profile.endpoint.as_str()),
                    ("credential_ids", entry.credential.as_str()),
                ] {
                    if let Some(allowed) = ceiling[field].as_array()
                        && !allowed.iter().any(|s| s.as_str() == Some(value))
                    {
                        let mut e =
                            error("config_authority_violation", &format!("/authority/{field}"));
                        e.authority_id = Some(ceiling["id"].as_str().unwrap().into());
                        return Err(e);
                    }
                }
            }
        }
        if options["model_route"].as_str() == Some(name) {
            selected = Some(ResolvedRoute {
                name: name.clone(),
                selected_entry: entries[0].name.clone(),
                execution_available: true,
                entries,
                policy: RoutePolicy {
                    max_attempts: maximum,
                    per_attempt_timeout_ms: route["per_attempt_timeout_ms"].as_u64().unwrap(),
                    eligible_errors: errors,
                    retry_uncertain_delivery: uncertain,
                    retry_owner: "pablo".into(),
                    sticky: true,
                    required_capabilities: requirements,
                },
                selection_reason: "first_declared_compatible_entry",
                fallback_owner: "C3.11",
            });
        }
    }
    if options.get("model_route").is_some() && selected.is_none() {
        return Err(invalid("/options/model_route"));
    }
    Ok(selected)
}

/// Parse the same typed intent for the root model and named route entries.
pub(super) fn reasoning_profile(
    profile: &mut ModelProfile,
    model: &Value,
    max_output_tokens: u32,
) -> Result<(), ConfigError> {
    let value = model
        .get("reasoning")
        .or_else(|| model.get("model_options").and_then(|o| o.get("reasoning")));
    profile.reasoning = value
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|_| invalid("/model/reasoning"))?
        .unwrap_or_default();
    let caps = model
        .get("reasoning_capabilities")
        .or_else(|| model.get("capabilities").and_then(|c| c.get("reasoning")));
    profile.reasoning_capabilities = caps
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|_| invalid("/model/reasoning_capabilities"))?;
    profile
        .reasoning
        .validate_gateway(
            profile.provider,
            profile.reasoning_capabilities.as_ref(),
            max_output_tokens,
        )
        .map_err(|_| invalid("/model/reasoning"))
}
