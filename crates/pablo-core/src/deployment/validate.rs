use super::*;
use std::{collections::HashSet, path::Path};

pub(crate) fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
pub(crate) fn counter(value: &str) -> Option<u64> {
    if value.is_empty()
        || value.len() > 20
        || value.len() > 1 && value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}
fn amount(value: &Value) -> Result<Option<u64>, ConfigError> {
    if value.as_str() == Some("unlimited") {
        return Ok(None);
    }
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(counter))
        .map(Some)
        .ok_or_else(|| error("config_invalid_value", "/limits"))
}
fn narrower(value: &Value, ceiling: &Value) -> Result<bool, ConfigError> {
    Ok(match (amount(value)?, amount(ceiling)?) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(a), Some(b)) => a <= b,
    })
}
fn limits(values: &Value, ceiling: Option<&Value>, path: &str) -> Result<(), ConfigError> {
    for (key, value) in values.as_object().unwrap() {
        let path = pointer(path, key);
        let cap = ceiling.and_then(|c| c.get(key));
        if value.is_object() {
            limits(value, cap, &path)?;
            continue;
        }
        let quantity = amount(value).map_err(|_| error("config_invalid_value", &path))?;
        if key == "max_events" && quantity.is_none_or(|n| n < 4) {
            return Err(error("config_invalid_value", &path));
        }
        if let Some(cap) = cap
            && !narrower(value, cap)?
        {
            return Err(error("config_authority_violation", &path));
        }
    }
    Ok(())
}
// Validate intrinsic scalar ranges even in inactive or subsequently overridden
// declarations. Cross-option relationships are checked on the composed config.
pub(crate) fn declarations(value: &Value, path: &str) -> Result<(), ConfigError> {
    if let Some(shell) = value.pointer("/options/shell") {
        super::shell::declaration(shell)?;
    }
    if let Some(values) = value.pointer("/options/limits") {
        limits(values, None, &format!("{path}/options/limits"))?;
    }
    for section in ["model", "otel"] {
        let option = format!("/options/{section}/endpoint");
        if let Some(value) = value.pointer(&option) {
            endpoint(value, &format!("{path}{option}"))?;
        }
    }
    if let Some(models) = value.pointer("/options/models").and_then(Value::as_object) {
        for (name, model) in models {
            if let Some(value) = model.get("endpoint") {
                endpoint(value, &format!("{path}/options/models/{name}/endpoint"))?;
            }
        }
    }
    if let Some(layers) = value.get("authority").and_then(Value::as_array) {
        for (index, layer) in layers.iter().enumerate() {
            if let Some(shell) = layer.get("shell") {
                super::shell::authority(shell)?;
            }
            if let Some(values) = layer.get("limits") {
                limits(values, None, &format!("{path}/authority/{index}/limits"))?;
            }
            for field in ["provider_endpoints", "otel_endpoints"] {
                if let Some(values) = layer.get(field).and_then(Value::as_array) {
                    for (entry, value) in values.iter().enumerate() {
                        endpoint(value, &format!("{path}/authority/{index}/{field}/{entry}"))?;
                    }
                }
            }
        }
    }
    if let Some(profiles) = value.get("profiles").and_then(Value::as_object) {
        for (name, profile) in profiles {
            declarations(profile, &pointer(&format!("{path}/profiles"), name))?;
        }
    }
    Ok(())
}
fn endpoint(value: &Value, path: &str) -> Result<(), ConfigError> {
    let value = value.as_str().unwrap();
    let parsed = reqwest::Url::parse(value).map_err(|_| error("config_invalid_value", path))?;
    if value.len() > 4096
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(error("config_invalid_value", path));
    }
    Ok(())
}
pub(crate) fn strings(value: &Value, path: &str) -> Result<(), ConfigError> {
    match value {
        Value::String(text) => {
            let bound = if path.ends_with("/instructions") {
                MAX_FILE_BYTES
            } else if path.ends_with("/model/id") || path.ends_with("/service_name") {
                256
            } else if path.contains("/resource_attributes/")
                || path.ends_with("/path")
                || path.ends_with("/endpoint")
                || path.ends_with("/value")
            {
                4096
            } else {
                65_536
            };
            if text.len() > bound || text.contains('\0') {
                return Err(error("config_invalid_value", path));
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                strings(value, &format!("{path}/{index}"))?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if key.contains('\0')
                    || key.len()
                        > if path.ends_with("/resource_attributes") {
                            256
                        } else {
                            4096
                        }
                {
                    return Err(error("config_invalid_value", path));
                }
                strings(value, &pointer(path, key))?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub(crate) fn physical(
    value: &Value,
    options: &Value,
    request: &ResolveRequest,
    allow_workspace: bool,
) -> Result<PathBuf, ConfigError> {
    let base = value["base"]
        .as_str()
        .ok_or_else(|| error("config_invalid_value", "/path"))?;
    let root = match base {
        "config" => request.config_root.clone(),
        "binding" => request
            .path_bindings
            .get(value["name"].as_str().unwrap())
            .cloned()
            .ok_or_else(|| error("config_invalid_value", "/path_bindings"))?,
        "workspace" if allow_workspace => {
            physical(&options["run"]["workspace"], options, request, false)?
        }
        _ => return Err(error("config_invalid_value", "/options/run/workspace")),
    };
    let path = super::input::relative(value["path"].as_str().unwrap())?;
    Ok(if path == "." { root } else { root.join(path) })
}
pub(crate) fn narrow(
    option: &str,
    patch: &Value,
    options: &Value,
    request: &ResolveRequest,
) -> Result<(), ConfigError> {
    let path = format!("/{}", option.replace('.', "/"));
    let value = patch.pointer(&path).unwrap();
    let prior = options
        .pointer(&path)
        .ok_or_else(|| error("config_override_forbidden", option))?;
    let allowed = if option.starts_with("limits.")
        || matches!(
            option,
            "output.max_validation_work" | "output.repair.max_feedback_bytes"
        ) {
        narrower(value, prior)?
    } else if option == "run.workspace" {
        physical(value, options, request, false)?
            .starts_with(physical(prior, options, request, false)?)
    } else if matches!(
        option,
        "output.repair.enabled"
            | "shell.enabled"
            | "filesystem.enabled"
            | "filesystem.write"
            | "trace.capture_content"
    ) {
        value == &Value::Bool(false) || prior == &Value::Bool(true)
    } else {
        option == "interfaces.cli_output" || (option == "output.schema" && value == prior)
    };
    if allowed {
        Ok(())
    } else {
        Err(error("config_authority_violation", option))
    }
}
fn policy(
    value: &Value,
    ids: &mut HashSet<String>,
    rules: &mut usize,
    path: &str,
) -> Result<(), ConfigError> {
    let policy: crate::policy::Policy =
        serde_json::from_value(value.clone()).map_err(|_| error("config_invalid_value", path))?;
    for dimension in value.as_object().unwrap().values() {
        let mut size = 0;
        for list in ["allow", "deny"] {
            if let Some(values) = dimension.get(list).and_then(Value::as_array) {
                size += values.len();
                for rule in values {
                    if !ids.insert(rule["id"].as_str().unwrap().to_owned()) {
                        return Err(error("config_conflict", path));
                    }
                }
            }
        }
        *rules += size;
        if size > 128 || *rules > 1024 {
            return Err(limit());
        }
    }
    policy
        .validate()
        .map_err(|_| error("config_invalid_value", path))
}
pub(crate) fn config(config: &Value, request: &ResolveRequest) -> Result<(), ConfigError> {
    strings(config, "/config")?;
    let options = &config["options"];
    limits(&options["limits"], None, "/config/options/limits")?;
    let workspace = physical(&options["run"]["workspace"], options, request, false)?;
    if options["filesystem"]["write"] == true && options["filesystem"]["enabled"] == false {
        return Err(error(
            "config_invalid_value",
            "/config/options/filesystem/write",
        ));
    }
    if options["trace"]["capture_content"] == true
        && options["trace"]["path"].get("unset").is_some()
    {
        return Err(error("config_invalid_value", "/config/options/trace/path"));
    }
    if options["otel"]["max_export_batch_size"].as_u64()
        > options["otel"]["max_queue_size"].as_u64()
    {
        return Err(error(
            "config_invalid_value",
            "/config/options/otel/max_export_batch_size",
        ));
    }
    let selected_route = super::routes::resolve(config)?;
    let model = if let Some(route) = &selected_route {
        &options["models"][route.entries()[0].name()]
    } else {
        &options["model"]
    };
    let provider: crate::gateway::GatewayKind = model["provider"]
        .as_str()
        .unwrap()
        .parse()
        .map_err(|_| error("config_invalid_value", "/config/options/model/provider"))?;
    if provider == crate::gateway::GatewayKind::OpenResponses {
        super::admission::responses_profile(model)?;
        if options["limits"]["max_output_tokens"]
            .as_u64()
            .is_none_or(|n| n < 16)
        {
            return Err(error(
                "config_invalid_value",
                "/options/limits/max_output_tokens",
            ));
        }
    } else if model["endpoint"] != provider.endpoint()
        || ["capability_profile", "auth_header", "auth_scheme"]
            .iter()
            .any(|field| model.get(field).is_some())
    {
        return Err(error(
            "config_invalid_value",
            "/config/options/model/endpoint",
        ));
    }
    for (id, consumer) in [
        (
            model["credential"].as_str(),
            provider.credential_consumer().name(),
        ),
        (options["otel"]["headers"].as_str(), "otel.headers"),
    ] {
        if let Some(id) = id
            && config["credentials"]
                .get(id)
                .is_none_or(|record| record["consumer"] != consumer)
        {
            return Err(error("config_invalid_value", "/config/credentials"));
        }
    }
    let mcp = super::mcp::settings(config)?;
    let mut credential_ids: Vec<&str> = [
        model["credential"].as_str(),
        options["otel"]["headers"].as_str(),
    ]
    .into_iter()
    .flatten()
    .collect();
    for server in mcp.servers.values() {
        for (_, id, consumer) in server.credentials() {
            if config["credentials"]
                .get(id)
                .is_none_or(|record| record["consumer"] != consumer)
            {
                return Err(error("config_invalid_value", "/config/credentials"));
            }
            credential_ids.push(id);
        }
    }
    // Validate declared references, including unused secret sources, without
    // opening a file, observing presence or invoking a host credential callback.
    validate_paths(config, "/config", options, request)?;
    let mut rule_ids = HashSet::new();
    let mut rules = 0;
    policy(
        &options["policy"],
        &mut rule_ids,
        &mut rules,
        "/config/options/policy",
    )?;
    let mut authority_ids = HashSet::new();
    let mut tools = Vec::new();
    if options["shell"]["enabled"] == true {
        tools.push("shell.run");
    }
    if options["filesystem"]["enabled"] == true {
        tools.extend(["fs.read", "fs.list", "fs.search"]);
        if options["filesystem"]["write"] == true {
            tools.extend(["fs.write", "fs.edit"]);
        }
    }
    for layer in config["authority"].as_array().unwrap() {
        let id = layer["id"].as_str().unwrap();
        if !authority_ids.insert(id) {
            return Err(error("config_conflict", "/config/authority"));
        }
        let result = (|| {
            if let Some(names) = layer
                .pointer("/shell/environment/allowed_names")
                .and_then(Value::as_array)
                && options["shell"]["environment"]["values"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .any(|name| !names.iter().any(|allowed| allowed.as_str() == Some(name)))
            {
                return Err(error(
                    "config_authority_violation",
                    "/config/options/shell/environment/values",
                ));
            }
            if let Some(roots) = layer.get("workspace_roots").and_then(Value::as_array) {
                let roots = roots
                    .iter()
                    .map(|r| physical(r, options, request, true))
                    .collect::<Result<Vec<_>, _>>()?;
                if !roots.iter().any(|r| workspace.starts_with(r)) {
                    return Err(error(
                        "config_authority_violation",
                        "/config/options/run/workspace",
                    ));
                }
            }
            for (field, values) in [
                ("tool_names", tools.clone()),
                ("model_ids", vec![model["id"].as_str().unwrap()]),
                (
                    "provider_endpoints",
                    vec![model["endpoint"].as_str().unwrap()],
                ),
                ("credential_ids", credential_ids.clone()),
                (
                    "otel_endpoints",
                    if options["otel"]["exporter"] == "otlp"
                        && options["otel"]["sdk_disabled"] == false
                    {
                        vec![options["otel"]["endpoint"].as_str().unwrap()]
                    } else {
                        vec![]
                    },
                ),
            ] {
                if let Some(allowed) = layer.get(field).and_then(Value::as_array)
                    && values
                        .iter()
                        .any(|value| !allowed.iter().any(|a| a.as_str() == Some(value)))
                {
                    return Err(error(
                        "config_authority_violation",
                        &pointer("/config/authority", field),
                    ));
                }
            }
            if layer.get("capture_content") == Some(&Value::Bool(false))
                && options["trace"]["capture_content"] == true
            {
                return Err(error(
                    "config_authority_violation",
                    "/config/options/trace/capture_content",
                ));
            }
            if let Some(ceiling) = layer.get("limits") {
                limits(ceiling, None, "/config/authority/limits")?;
                limits(&options["limits"], Some(ceiling), "/config/options/limits")?;
            }
            if let Some(p) = layer.get("policy") {
                policy(p, &mut rule_ids, &mut rules, "/config/authority/policy")?;
            }
            Ok(())
        })();
        result.map_err(|mut e: ConfigError| {
            e.authority_id = Some(id.into());
            e
        })?;
    }
    super::shell::validate(config, &mut rule_ids)?;
    Ok(())
}
fn validate_paths(
    value: &Value,
    path: &str,
    options: &Value,
    request: &ResolveRequest,
) -> Result<(), ConfigError> {
    if value.get("base").is_some()
        && value.get("path").is_some()
        && path != "/config/options/otel/resource_attributes"
    {
        physical(value, options, request, true).map_err(|_| error("config_invalid_value", path))?;
        if value["path"]
            .as_str()
            .is_some_and(|p| Path::new(p).is_absolute())
        {
            return Err(error("config_invalid_value", path));
        }
        return Ok(());
    }
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                validate_paths(value, &pointer(path, key), options, request)?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_paths(value, &format!("{path}/{index}"), options, request)?;
            }
        }
        _ => {}
    }
    Ok(())
}
