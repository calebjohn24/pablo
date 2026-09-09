use super::{
    input::{self, Document, Reader},
    *,
};
use serde_json::json;
use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
    sync::Arc,
};

/// Resolve only declared data. No environment lookup, credential reader,
/// provider, tool, runtime or telemetry setup is reachable from this entry point.
pub fn resolve(request: ResolveRequest) -> Result<ResolvedDeployment, ConfigError> {
    load(request)?.resolve()
}

impl ResolvedDeployment {
    /// Apply explicit per-task values to this snapshot through the same
    /// override/authority checks, preserving its source and provenance history.
    pub fn with_overrides(&self, overrides: Map<String, Value>) -> Result<Self, ConfigError> {
        if overrides.len() > 64 {
            return Err(limit());
        }
        let mut request = ResolveRequest::new(self.config_root.clone(), "unused");
        request.path_bindings = self.path_bindings.clone();
        request.overrides = overrides;
        let resolver = Resolver {
            reader: Reader::new(&request.config_root)?,
            request: Arc::new(request),
            config: self.config.clone(),
            sources: self.sources.clone(),
            provenance: self.provenance.clone(),
            origins: self.provenance.values().map(Vec::len).sum(),
            environment: Map::new(),
            active: Vec::new(),
            edges: 0,
            profiles: 0,
            expanded_profiles: 0,
            authority_sources: Vec::new(),
            aliases: BTreeMap::new(),
        };
        LoadedDeployment {
            resolver,
            locked: self.config["deployment"]["locked"].as_bool().unwrap(),
        }
        .resolve()
    }
}

/// Load and compose the selected file trees once, without consulting ambient
/// environment. Hosts may then capture only `environment_names()` and finish
/// resolution using these same pinned inputs, even if the files change.
pub fn load(request: ResolveRequest) -> Result<LoadedDeployment, ConfigError> {
    let request = Arc::new(request);
    if request.path_bindings.len() > 64
        || request.host_authority.len() > 64
        || request.overrides.len() > 64
    {
        return Err(limit());
    }
    for (name, root) in &request.path_bindings {
        if !validate::name(name) || !input::absolute_root(root) {
            return Err(error("config_invalid_value", "/path_bindings"));
        }
    }
    let mut reader = Reader::new(&request.config_root)?;
    let entry = prepare(&mut reader, &request.entry, "host:entry")?;
    let locked = request.locked
        || entry
            .value
            .pointer("/deployment/locked")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if locked && !request.path_bindings.contains_key("workspace") {
        return Err(error("config_invalid_value", "/path_bindings/workspace"));
    }
    if request.locked && entry.value.pointer("/deployment/locked") == Some(&Value::Bool(false)) {
        return Err(error("config_authority_violation", "/deployment/locked"));
    }
    let defaults = json!({"options":input::defaults(),"deployment":{"locked":false,"allowed_run_overrides":["input"]}});
    let config = json!({"options":defaults["options"],"deployment":defaults["deployment"],"credentials":{},"authority":[]});
    let mut resolver = Resolver {
        request: request.clone(),
        reader,
        config,
        sources: Vec::new(),
        provenance: BTreeMap::new(),
        origins: 0,
        environment: Map::new(),
        active: Vec::new(),
        edges: 0,
        profiles: 0,
        expanded_profiles: 0,
        authority_sources: Vec::new(),
        aliases: BTreeMap::new(),
    };
    let source = resolver.source(
        "defaults",
        "deployment-defaults-v1.json",
        fingerprint(&defaults)?,
    )?;
    resolver.record(&resolver.config.clone(), "/config", &source, "default")?;
    for authority in &request.host_authority {
        let mut document = json!({"schema_version":1,"authority":[authority]});
        input::check_value(&document, &mut resolver.reader.nodes, 1)?;
        input::shape(&document)?;
        normalize_paths(&mut document, "", None, &request.path_bindings)?;
        let authority = &document["authority"][0];
        let source = resolver.source(
            "host_authority",
            authority["id"].as_str().unwrap(),
            fingerprint(authority)?,
        )?;
        resolver.add_authority(authority, &source)?;
    }
    if !locked {
        for (input, locator) in [
            (&request.user_config, "host:user"),
            (&request.workspace_config, "host:workspace"),
        ] {
            if let Some(input) = input {
                let document = prepare(&mut resolver.reader, input, locator)?;
                if document.value.get("deployment").is_some() {
                    return Err(
                        error("config_override_forbidden", "/deployment").at(&document.locator)
                    );
                }
                resolver.tree(document, None)?;
            }
        }
    }
    let controls = entry
        .value
        .get("deployment")
        .cloned()
        .unwrap_or_else(|| json!({}));
    resolver.tree(entry, request.profile.as_deref())?;
    // Only the explicit entry controls bootstrap settings. Profile/module
    // bodies cannot touch them, and host lock mode is never replaced.
    let control_source = resolver
        .sources
        .iter()
        .rev()
        .find(|s| matches!(s.kind, "file" | "override"))
        .unwrap()
        .id
        .clone();
    merge(
        &mut resolver.config["deployment"],
        &controls,
        "/config/deployment",
        &control_source,
        &mut resolver.provenance,
        &mut resolver.origins,
    )?;
    resolver.config["deployment"]["locked"] = locked.into();
    // Reject overlapping secret/non-secret declarations before the host even
    // receives the names to capture. Do not turn credential bytes into options,
    // source digests or inspection output by way of an environment binding.
    let mut secret_names: HashSet<&str> = [
        "AI_GATEWAY_API_KEY",
        "VERCEL_AI_GATEWAY",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    ]
    .into();
    for credential in resolver.config["credentials"].as_object().unwrap().values() {
        for source in credential["sources"].as_array().unwrap() {
            if source["kind"] == "environment" {
                secret_names.insert(source["name"].as_str().unwrap());
            }
        }
    }
    if resolver
        .environment
        .keys()
        .any(|name| secret_names.contains(name.as_str()))
    {
        return Err(error("config_invalid_value", "/environment"));
    }
    Ok(LoadedDeployment { resolver, locked })
}

/// Composed data awaiting an explicit non-secret environment snapshot. No
/// credentials or runtime resources are read or created by this object.
pub struct LoadedDeployment {
    resolver: Resolver,
    locked: bool,
}
impl LoadedDeployment {
    pub fn environment_names(&self) -> impl Iterator<Item = &str> {
        self.resolver.environment.keys().map(String::as_str)
    }

    /// Replace the approved snapshot. Unrelated process variables must not be
    /// copied here; undeclared names reject instead of being retained.
    pub fn with_environment(
        mut self,
        values: BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        if values.len() > 64
            || values
                .keys()
                .any(|name| !self.resolver.environment.contains_key(name))
        {
            return Err(error("config_environment_value", "/environment"));
        }
        Arc::get_mut(&mut self.resolver.request)
            .expect("loaded request is privately owned")
            .environment = values;
        Ok(self)
    }

    pub fn resolve(self) -> Result<ResolvedDeployment, ConfigError> {
        let Self {
            mut resolver,
            locked,
        } = self;
        resolver.apply_environment()?;
        if resolver.request.locked {
            let source = resolver.source(
                "override",
                "deployment.locked",
                fingerprint(&json!({"option":"deployment.locked","value":true}))?,
            )?;
            resolver.mark("/config/deployment/locked", &source, "constrain")?;
        }
        resolver.apply_overrides(locked)?;
        resolver.complete_policies()?;
        resolver.complete_aliases()?;
        input::resolved_shape(&resolver.config)?;
        validate::config(&resolver.config, &resolver.request)?;
        resolver.provenance.retain(|path, _| {
            resolver
                .config
                .pointer(path.strip_prefix("/config").unwrap())
                .is_some()
        });
        let fingerprint = fingerprint(
            &json!({"schema_version":1,"contract_revision":CONTRACT_REVISION,"config":resolver.config}),
        )?;
        let input_fingerprint = super::fingerprint(
            &json!({"schema_version":1,"contract_revision":CONTRACT_REVISION,"sources":resolver.sources}),
        )?;
        let result = ResolvedDeployment {
            config_root: resolver.request.config_root.clone(),
            path_bindings: resolver.request.path_bindings.clone(),
            schema_version: 1,
            contract_revision: CONTRACT_REVISION,
            config: resolver.config,
            fingerprint,
            sources: resolver.sources,
            provenance: resolver.provenance,
            input_fingerprint,
        };
        // Inspection is bounded independently of the effective config. Avoid an
        // unbounded to_vec of the complete source/provenance envelope.
        canonical::check_inspection_size(&result)?;
        Ok(result)
    }
}

fn prepare(
    reader: &mut Reader,
    input: &ConfigInput,
    locator: &str,
) -> Result<Document, ConfigError> {
    match input {
        ConfigInput::File(path) => reader.read(&reader.entry_path(path)?),
        ConfigInput::Document(value) => reader.document(value, locator),
    }
}
struct Profile {
    value: Value,
    locators: Vec<String>,
}
struct Resolver {
    request: Arc<ResolveRequest>,
    reader: Reader,
    config: Value,
    sources: Vec<Source>,
    provenance: BTreeMap<String, Vec<Origin>>,
    origins: usize,
    environment: Map<String, Value>,
    active: Vec<String>,
    edges: usize,
    profiles: usize,
    expanded_profiles: usize,
    authority_sources: Vec<String>,
    aliases: BTreeMap<String, Vec<String>>,
}
impl Resolver {
    fn source(
        &mut self,
        kind: &'static str,
        locator: &str,
        digest: String,
    ) -> Result<String, ConfigError> {
        if self.sources.len() >= 1024 {
            return Err(limit());
        }
        let id = format!("source-{:04}", self.sources.len());
        self.sources.push(Source {
            id: id.clone(),
            kind,
            locator: locator.into(),
            digest,
        });
        Ok(id)
    }
    fn record(
        &mut self,
        value: &Value,
        path: &str,
        source: &str,
        operation: &'static str,
    ) -> Result<(), ConfigError> {
        record(
            value,
            path,
            source,
            operation,
            &mut self.provenance,
            &mut self.origins,
        )
    }
    fn mark(
        &mut self,
        path: &str,
        source: &str,
        operation: &'static str,
    ) -> Result<(), ConfigError> {
        mark(
            path,
            source,
            operation,
            &mut self.provenance,
            &mut self.origins,
        )
    }
    fn tree(&mut self, document: Document, selected: Option<&str>) -> Result<(), ConfigError> {
        let selected = selected.map(str::to_owned).or_else(|| {
            document
                .value
                .get("profile")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let mut profiles = BTreeMap::new();
        self.visit(document, 1, &mut profiles)?;
        if let Some(selected) = selected {
            self.profile(&selected, &profiles, &mut Vec::new(), &mut HashSet::new())?;
        }
        Ok(())
    }
    fn visit(
        &mut self,
        mut document: Document,
        depth: usize,
        profiles: &mut BTreeMap<String, Profile>,
    ) -> Result<(), ConfigError> {
        if depth > MAX_IMPORT_DEPTH {
            return Err(limit().at(&document.locator));
        }
        self.active.push(document.locator.clone());
        if let Some(imports) = document.value.get("imports").and_then(Value::as_array) {
            for import in imports {
                admit_import(&mut self.edges, depth).map_err(|e| e.at(&document.locator))?;
                let name = import.as_str().unwrap();
                if name.contains("://")
                    || name.contains(['*', '?', '[', ']'])
                    || Path::new(name).is_absolute()
                {
                    return Err(error("config_import_path", "/imports").at(&document.locator));
                }
                let parent = if document.file {
                    Path::new(&document.locator).parent().unwrap()
                } else {
                    Path::new("")
                };
                let path = input::relative(
                    parent
                        .join(name)
                        .to_str()
                        .ok_or_else(|| error("config_import_path", "/imports"))?,
                )?;
                if self.active.contains(&path) {
                    return Err(error("config_import_cycle", "/imports")
                        .at(&document.locator)
                        .with_chain(
                            self.active
                                .iter()
                                .map(String::as_str)
                                .chain(std::iter::once(path.as_str())),
                        ));
                }
                let child = self.reader.read(&path)?;
                if child.value.get("profile").is_some() || child.value.get("deployment").is_some() {
                    return Err(error("config_override_forbidden", "/deployment").at(&path));
                }
                self.visit(child, depth + 1, profiles)?;
            }
        }
        normalize_paths(
            &mut document.value,
            "",
            if document.file {
                Some(&document.locator)
            } else {
                None
            },
            &self.request.path_bindings,
        )
        .map_err(|e| e.at(&document.locator))?;
        let digest = if document.file {
            document.digest
        } else {
            fingerprint(&json!({"option":"document","value":document.value}))?
        };
        let source = self.source(
            if document.file { "file" } else { "override" },
            &document.locator,
            digest,
        )?;
        if let Some(definitions) = document.value.get("profiles").and_then(Value::as_object) {
            for (name, value) in definitions {
                if let Some(prior) = profiles.get_mut(name) {
                    if prior.value != *value {
                        return Err(error("config_conflict", &pointer("/profiles", name))
                            .at(&document.locator));
                    }
                    prior.locators.push(document.locator.clone());
                } else {
                    self.profiles += 1;
                    if self.profiles > 64 {
                        return Err(limit());
                    }
                    profiles.insert(
                        name.clone(),
                        Profile {
                            value: value.clone(),
                            locators: vec![document.locator.clone()],
                        },
                    );
                }
            }
        }
        if let Some(credentials) = document.value.get("credentials").and_then(Value::as_object) {
            for (name, value) in credentials {
                let path = pointer("/config/credentials", name);
                let prior = self.config["credentials"].get(name);
                if prior.is_some_and(|p| p != value) {
                    return Err(error("config_conflict", &path).at(&document.locator));
                }
                if prior.is_none() && self.config["credentials"].as_object().unwrap().len() == 64 {
                    return Err(limit());
                }
                self.config["credentials"][name] = value.clone();
                self.record(value, &path, &source, "set")?;
            }
        }
        if let Some(bindings) = document.value.get("environment").and_then(Value::as_object) {
            for (name, value) in bindings {
                if self.environment.get(name).is_some_and(|p| p != value) {
                    return Err(error("config_conflict", &pointer("/environment", name))
                        .at(&document.locator));
                }
                if !self.environment.contains_key(name) && self.environment.len() == 64 {
                    return Err(limit());
                }
                self.environment.insert(name.clone(), value.clone());
            }
        }
        self.body(&document.value, &source)
            .map_err(|e| e.at(&document.locator))?;
        self.active.pop();
        Ok(())
    }
    fn profile(
        &mut self,
        name: &str,
        profiles: &BTreeMap<String, Profile>,
        active: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) -> Result<(), ConfigError> {
        if active.iter().any(|p| p == name) {
            return Err(error("config_profile_cycle", "/profiles").with_chain(
                active
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(name)),
            ));
        }
        if !seen.insert(name.into()) {
            return Err(error("config_duplicate_profile_ancestor", "/profiles"));
        }
        self.expanded_profiles += 1;
        if active.len() >= 16 || self.expanded_profiles > 64 {
            return Err(limit());
        }
        let profile = profiles
            .get(name)
            .ok_or_else(|| error("config_unknown_profile", "/profile"))?;
        active.push(name.into());
        if let Some(parents) = profile.value.get("extends").and_then(Value::as_array) {
            for parent in parents {
                self.profile(parent.as_str().unwrap(), profiles, active, seen)?;
            }
        }
        input::check_value(&profile.value, &mut self.reader.nodes, 1)?;
        let digest = fingerprint(&json!({"name":name,"profile":profile.value}))?;
        let source = self.source(
            "profile",
            &format!("{}#{name}", profile.locators[0]),
            digest.clone(),
        )?;
        for locator in profile.locators.iter().skip(1) {
            let alias = self.source("profile", &format!("{locator}#{name}"), digest.clone())?;
            self.aliases.entry(source.clone()).or_default().push(alias);
        }
        self.body(&profile.value, &source)
            .map_err(|e| e.at(&profile.locators[0]))?;
        active.pop();
        Ok(())
    }
    fn body(&mut self, body: &Value, source: &str) -> Result<(), ConfigError> {
        if let Some(options) = body.get("options") {
            merge(
                &mut self.config["options"],
                options,
                "/config/options",
                source,
                &mut self.provenance,
                &mut self.origins,
            )?;
        }
        if let Some(layers) = body.get("authority").and_then(Value::as_array) {
            for layer in layers {
                self.add_authority(layer, source)?;
            }
        }
        Ok(())
    }
    fn add_authority(&mut self, layer: &Value, source: &str) -> Result<(), ConfigError> {
        let mut layer = layer.clone();
        if let Some(roots) = layer
            .get_mut("workspace_roots")
            .and_then(Value::as_array_mut)
        {
            for root in roots {
                if root["base"] == "workspace" {
                    // Resolve this ceiling against the workspace at declaration,
                    // retaining its portable config/binding anchor. Later option
                    // precedence cannot move an already accumulated ceiling.
                    let mut anchor = self.config["options"]["run"]["workspace"].clone();
                    if !matches!(anchor["base"].as_str(), Some("config" | "binding")) {
                        return Err(error("config_invalid_value", "/options/run/workspace"));
                    }
                    anchor["path"] = input::relative(&format!(
                        "{}/{}",
                        anchor["path"].as_str().unwrap(),
                        root["path"].as_str().unwrap()
                    ))?
                    .into();
                    *root = anchor;
                }
            }
        }
        let layers = self.config["authority"].as_array_mut().unwrap();
        if layers.len() == 64 {
            return Err(limit());
        }
        let path = format!("/config/authority/{}", layers.len());
        layers.push(layer.clone());
        self.authority_sources.push(source.into());
        self.mark(&path, source, "constrain")?;
        self.record(&layer, &path, source, "constrain")
    }
    fn apply_environment(&mut self) -> Result<(), ConfigError> {
        let mut targets = BTreeSet::new();
        for (name, binding) in self.environment.clone() {
            let option = binding["option"].as_str().unwrap();
            if !targets.insert(option.to_owned()) {
                return Err(error("config_conflict", "/environment"));
            }
            let Some(value) = self.request.environment.get(&name) else {
                if binding
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return Err(error("config_environment_missing", option));
                }
                self.source(
                    "environment",
                    &name,
                    fingerprint(&json!({"name":name,"option":option,"present":false}))?,
                )?;
                continue;
            };
            if value.len() > 65_536 {
                return Err(error("config_environment_value", option));
            }
            let parsed = environment_value(option, value, &self.config["options"])?;
            let patch = patch(option, parsed.clone())?;
            let document = json!({"schema_version":1,"options":patch});
            input::check_value(&document, &mut self.reader.nodes, 1)?;
            input::shape(&document).map_err(|_| error("config_environment_value", option))?;
            let source = self.source(
                "environment",
                &name,
                fingerprint(&json!({"name":name,"option":option,"present":true,"value":parsed}))?,
            )?;
            merge(
                &mut self.config["options"],
                &patch,
                "/config/options",
                &source,
                &mut self.provenance,
                &mut self.origins,
            )?;
        }
        Ok(())
    }
    fn apply_overrides(&mut self, locked: bool) -> Result<(), ConfigError> {
        let request = self.request.clone();
        for (option, value) in &request.overrides {
            if locked
                && !self.config["deployment"]["allowed_run_overrides"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p.as_str() == Some(option))
            {
                return Err(error("config_override_forbidden", option));
            }
            if option == "input" {
                if !value.is_string() {
                    return Err(error("config_invalid_value", "input"));
                }
                continue;
            }
            let mut patch = patch(option, value.clone())?;
            let mut document = json!({"schema_version":1,"options":patch});
            input::check_value(&document, &mut self.reader.nodes, 1)?;
            input::shape(&document)?;
            normalize_paths(&mut document, "", None, &self.request.path_bindings)?;
            patch = document["options"].take();
            if locked {
                validate::narrow(option, &patch, &self.config["options"], &self.request)?;
            }
            let source = self.source("override",option,fingerprint(&json!({"option":option,"value":patch.pointer(&option_pointer(option)?).unwrap()}))?)?;
            merge(
                &mut self.config["options"],
                &patch,
                "/config/options",
                &source,
                &mut self.provenance,
                &mut self.origins,
            )?;
        }
        Ok(())
    }
    fn complete_policies(&mut self) -> Result<(), ConfigError> {
        let mut locations = vec![("/options/policy".to_owned(), "source-0000".to_owned())];
        for (index, source) in self.authority_sources.iter().enumerate() {
            locations.push((format!("/authority/{index}/policy"), source.clone()));
        }
        for (path, source) in locations {
            let Some(policy) = self
                .config
                .pointer_mut(&path)
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            let mut added = Vec::new();
            for (dimension, rules) in policy {
                for list in ["allow", "deny"] {
                    if rules.get(list).is_none() {
                        rules[list] = json!([]);
                        added.push(format!("/config{path}/{dimension}/{list}"));
                    }
                }
            }
            for path in added {
                self.mark(&path, &source, "default")?;
            }
        }
        let shell_locations =
            std::iter::once(("/options/shell".to_owned(), "source-0000".to_owned()))
                .chain(
                    self.authority_sources
                        .iter()
                        .enumerate()
                        .map(|(i, source)| (format!("/authority/{i}/shell"), source.clone())),
                )
                .collect::<Vec<_>>();
        for (path, source) in shell_locations {
            for dimension in ["commands", "cwd_roots"] {
                let root = format!("{path}/{dimension}");
                let Some(rules) = self
                    .config
                    .pointer_mut(&root)
                    .and_then(Value::as_object_mut)
                else {
                    continue;
                };
                let mut added = Vec::new();
                for list in ["allow", "deny"] {
                    if !rules.contains_key(list) {
                        rules.insert(list.into(), json!([]));
                        added.push(format!("/config{root}/{list}"));
                    }
                }
                for path in added {
                    self.mark(&path, &source, "default")?;
                }
            }
        }
        Ok(())
    }
    fn complete_aliases(&mut self) -> Result<(), ConfigError> {
        if self.aliases.is_empty() {
            return Ok(());
        }
        for origins in self.provenance.values_mut() {
            let mut expanded = Vec::new();
            for origin in origins.drain(..) {
                let aliases = self.aliases.get(&origin.source);
                expanded.push(origin.clone());
                for source in aliases.into_iter().flatten() {
                    if self.origins == MAX_ORIGINS {
                        return Err(limit());
                    }
                    self.origins += 1;
                    expanded.push(Origin {
                        source: source.clone(),
                        operation: origin.operation,
                    });
                }
            }
            *origins = expanded;
        }
        Ok(())
    }
}

fn admit_import(edges: &mut usize, depth: usize) -> Result<(), ConfigError> {
    if *edges == MAX_IMPORT_EDGES || depth >= MAX_IMPORT_DEPTH {
        return Err(limit());
    }
    *edges += 1;
    Ok(())
}

fn option_pointer(option: &str) -> Result<String, ConfigError> {
    if option.len() > 384
        || option.split('.').any(|part| part.is_empty())
        || option.split('.').count() > MAX_DEPTH
    {
        return Err(error("config_unknown_option", "/overrides"));
    }
    Ok(option
        .split('.')
        .fold(String::new(), |path, key| pointer(&path, key)))
}
fn patch(option: &str, value: Value) -> Result<Value, ConfigError> {
    option_pointer(option)?;
    Ok(option.split('.').rev().fold(value, |value, key| {
        let mut map = Map::new();
        map.insert(key.into(), value);
        Value::Object(map)
    }))
}
fn environment_value(option: &str, value: &str, options: &Value) -> Result<Value, ConfigError> {
    let current = options
        .pointer(&option_pointer(option)?)
        .ok_or_else(|| error("config_unknown_option", option))?;
    if matches!(
        option,
        "limits.max_total_tokens" | "limits.max_cost_microusd" | "limits.max_events"
    ) {
        if value != "unlimited" || option == "limits.max_events" {
            validate::counter(value).ok_or_else(|| error("config_environment_value", option))?;
        }
        return Ok(value.into());
    }
    if current.is_boolean() {
        return match value {
            "true" => Ok(true.into()),
            "false" => Ok(false.into()),
            _ => Err(error("config_environment_value", option)),
        };
    }
    if current.is_number() || matches!(option, "limits.max_model_calls" | "limits.max_tool_calls") {
        if value == "unlimited"
            && matches!(option, "limits.max_model_calls" | "limits.max_tool_calls")
        {
            return Ok(value.into());
        }
        return validate::counter(value)
            .map(Value::from)
            .ok_or_else(|| error("config_environment_value", option));
    }
    Ok(value.into())
}

pub(crate) fn normalize_paths(
    value: &mut Value,
    path: &str,
    file: Option<&str>,
    bindings: &BTreeMap<String, PathBuf>,
) -> Result<(), ConfigError> {
    let parts: Vec<_> = path.split('/').rev().collect();
    let is_path = (parts.len() >= 3
        && ((parts[0] == "workspace" && parts[1] == "run")
            || (parts[0] == "path" && parts[1] == "trace"))
        && parts[2] == "options")
        || (parts.len() >= 5
            && parts[0] == "path"
            && parts[2] == "sources"
            && parts[4] == "credentials")
        || (parts.len() >= 4 && parts[1] == "workspace_roots" && parts[3] == "authority");
    if is_path && value.get("unset").is_none() {
        let base = value["base"].as_str().unwrap();
        let text = value["path"].as_str().unwrap();
        if parts[0] == "workspace" && parts[1] == "run" && base == "workspace" {
            return Err(error("config_invalid_value", path));
        }
        let normalized = if base == "source" {
            let file = file.ok_or_else(|| error("config_invalid_value", path))?;
            if Path::new(text).is_absolute() {
                return Err(error("config_invalid_value", path));
            }
            input::relative(
                Path::new(file)
                    .parent()
                    .unwrap()
                    .join(text)
                    .to_str()
                    .unwrap(),
            )
        } else {
            input::relative(text)
        }
        .map_err(|_| error("config_invalid_value", path))?;
        if base == "binding" && !bindings.contains_key(value["name"].as_str().unwrap()) {
            return Err(error("config_invalid_value", path));
        }
        if base == "source" {
            value["base"] = "config".into();
        }
        value["path"] = normalized.into();
        return Ok(());
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                normalize_paths(value, &pointer(path, key), file, bindings)?;
            }
        }
        Value::Array(values) => {
            for (i, value) in values.iter_mut().enumerate() {
                normalize_paths(value, &format!("{path}/{i}"), file, bindings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn mark(
    path: &str,
    source: &str,
    operation: &'static str,
    origins: &mut BTreeMap<String, Vec<Origin>>,
    count: &mut usize,
) -> Result<(), ConfigError> {
    if *count == MAX_ORIGINS {
        return Err(limit());
    }
    *count += 1;
    origins.entry(path.into()).or_default().push(Origin {
        source: source.into(),
        operation,
    });
    Ok(())
}
fn record(
    value: &Value,
    path: &str,
    source: &str,
    operation: &'static str,
    origins: &mut BTreeMap<String, Vec<Origin>>,
    count: &mut usize,
) -> Result<(), ConfigError> {
    match value {
        Value::Object(map) if !map.is_empty() => {
            if atomic(value, path) {
                mark(path, source, operation, origins, count)?;
            }
            for (key, value) in map {
                record(
                    value,
                    &pointer(path, key),
                    source,
                    operation,
                    origins,
                    count,
                )?;
            }
        }
        Value::Array(values) if !values.is_empty() => {
            mark(path, source, operation, origins, count)?;
            for (index, value) in values.iter().enumerate() {
                let path = format!("{path}/{index}");
                if value.is_object() && !atomic(value, &path) {
                    mark(&path, source, operation, origins, count)?;
                }
                record(value, &path, source, operation, origins, count)?;
            }
        }
        _ => mark(path, source, operation, origins, count)?,
    }
    Ok(())
}
fn atomic(value: &Value, path: &str) -> bool {
    path != "/config/options/otel/resource_attributes"
        && (value.get("base").is_some() && value.get("path").is_some()
            || value.get("unset") == Some(&Value::Bool(true)))
}
fn merge(
    target: &mut Value,
    patch: &Value,
    path: &str,
    source: &str,
    origins: &mut BTreeMap<String, Vec<Origin>>,
    count: &mut usize,
) -> Result<(), ConfigError> {
    let operation = if path == "/config/options/otel/propagators"
        || (path.contains("/policy/") || path.contains("/shell/cwd_roots/"))
            && (path.ends_with("/allow") || path.ends_with("/deny"))
    {
        patch.get("mode").and_then(Value::as_str)
    } else {
        None
    };
    if patch.is_array() || operation.is_some() {
        let operation = match operation.unwrap_or("replace") {
            "append" => "append",
            "prepend" => "prepend",
            _ => "replace",
        };
        let incoming = patch
            .as_array()
            .or_else(|| patch["items"].as_array())
            .unwrap();
        let old = target.as_array().cloned().unwrap_or_default();
        let old_len = if operation == "replace" { 0 } else { old.len() };
        if incoming.len() + old_len > 1024 {
            return Err(limit());
        }
        let prefix = format!("{path}/");
        let prior: Vec<_> = origins
            .range(prefix.clone()..)
            .take_while(|(p, _)| p.starts_with(&prefix))
            .map(|(p, v)| (p.clone(), v.clone()))
            .collect();
        if operation != "append" {
            for (key, _) in &prior {
                origins.remove(key);
            }
            if operation == "prepend" {
                for (key, value) in prior {
                    let suffix = &key[prefix.len()..];
                    let (index, rest) = suffix.split_once('/').unwrap_or((suffix, ""));
                    let index = index.parse::<usize>().map_err(|_| limit())? + incoming.len();
                    origins.insert(
                        format!(
                            "{prefix}{index}{}",
                            if rest.is_empty() {
                                String::new()
                            } else {
                                format!("/{rest}")
                            }
                        ),
                        value,
                    );
                }
            }
        }
        mark(path, source, operation, origins, count)?;
        let start = if operation == "append" { old.len() } else { 0 };
        for (index, value) in incoming.iter().enumerate() {
            let path = format!("{path}/{}", index + start);
            if value.is_object() && !atomic(value, &path) {
                mark(&path, source, operation, origins, count)?;
            }
            record(value, &path, source, operation, origins, count)?;
        }
        *target = Value::Array(match operation {
            "append" => old.into_iter().chain(incoming.iter().cloned()).collect(),
            "prepend" => incoming.iter().cloned().chain(old).collect(),
            _ => incoming.clone(),
        });
    } else if let Value::Object(map) = patch {
        if atomic(patch, path) {
            *target = patch.clone();
            record(
                patch,
                path,
                source,
                if map.contains_key("unset") {
                    "unset"
                } else {
                    "set"
                },
                origins,
                count,
            )?;
            return Ok(());
        }
        if !target.is_object() {
            *target = json!({});
        }
        for (key, value) in map {
            let values = target.as_object().unwrap();
            let bound = if path == "/config/options/otel/resource_attributes" {
                64
            } else {
                1024
            };
            if !values.contains_key(key) && values.len() == bound {
                return Err(limit());
            }
            merge(
                target
                    .as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert(Value::Null),
                value,
                &pointer(path, key),
                source,
                origins,
                count,
            )?;
        }
    } else {
        *target = patch.clone();
        mark(path, source, "set", origins, count)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_edge_budget_is_independent_of_file_identity_admission() {
        // A valid tree hits the 64-file bound first; test the defensive edge
        // counter without weakening repeated-file rejection to reach 128.
        let mut edges = 0;
        for _ in 0..MAX_IMPORT_EDGES {
            admit_import(&mut edges, 1).unwrap();
        }
        assert_eq!(edges, MAX_IMPORT_EDGES);
        assert_eq!(
            admit_import(&mut edges, 1).unwrap_err().code,
            "config_limit"
        );
        assert_eq!(edges, MAX_IMPORT_EDGES);
    }
    #[test]
    fn origin_budget_rejects_before_growing_the_next_contribution() {
        let mut origins = BTreeMap::new();
        let mut count = 0;
        for _ in 0..MAX_ORIGINS {
            mark(
                "/config/options/shell/enabled",
                "source-0000",
                "set",
                &mut origins,
                &mut count,
            )
            .unwrap();
        }
        assert_eq!(
            mark(
                "/config/options/shell/enabled",
                "source-0000",
                "set",
                &mut origins,
                &mut count
            )
            .unwrap_err()
            .code,
            "config_limit"
        );
        assert_eq!(origins["/config/options/shell/enabled"].len(), MAX_ORIGINS);
    }
}
