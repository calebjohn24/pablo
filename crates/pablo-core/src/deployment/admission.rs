//! Non-secret run admission shared by CLI, ACP and embedding hosts. Provider
//! and exporter credentials are resolved separately, immediately before use.
use super::*;
use crate::{
    RunSpec, ToolRegistry,
    policy::{Policy, PolicySet},
};
use std::path::{Component, Path};
mod child;

/// Per-task content and standard protocol session inputs, outside portable config.
pub struct RunInput {
    pub input: String,
    pub session_id: Option<String>,
    pub workspace: Option<PathBuf>,
}

/// An immutable task projection. Preparation creates no runtime, trace file,
/// credential handle or network client. Hosts complete scoped credential and
/// exporter setup before passing this spec/catalog to the existing runtime.
#[derive(Clone)]
pub struct PreparedRun {
    pub(super) config_root: PathBuf,
    pub(super) path_bindings: BTreeMap<String, PathBuf>,
    deployment: ResolvedDeployment,
    spec: RunSpec,
    policy: PolicySet,
    trace_path: Option<PathBuf>,
    bindings_fingerprint: String,
    pub(super) mcp_selection: Option<Vec<String>>,
    child_scope: Option<child::Scope>,
}
impl std::fmt::Debug for PreparedRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRun")
            .field("fingerprint", &self.deployment.fingerprint)
            .finish_non_exhaustive()
    }
}
impl PreparedRun {
    /// Authority only: deployment activation and joined root ownership are separate.
    pub fn admits_subagent_tool(&self) -> bool {
        !self.is_child()
            && self.policy.decide("tools", "subagent", false).is_ok()
            && self.deployment.config()["authority"]
                .as_array()
                .unwrap()
                .iter()
                .all(|layer| {
                    layer
                        .get("tool_names")
                        .and_then(Value::as_array)
                        .is_none_or(|names| names.iter().any(|name| name == "subagent"))
                })
    }

    /// True only for a scope derived through parent child admission.
    pub fn is_child(&self) -> bool {
        self.child_scope.is_some()
    }
    pub fn has_skills(&self) -> bool {
        !self.skill_names().is_empty()
    }
    pub fn needs_async_tools(&self) -> bool {
        self.has_mcp() || self.has_skills()
    }
    pub fn has_mcp(&self) -> bool {
        if self.mcp_selection.as_ref().is_some_and(Vec::is_empty) {
            return false;
        }
        self.deployment.options()["mcp"]["servers"]
            .as_object()
            .is_some_and(|servers| !servers.is_empty())
    }
    /// Validate host/provider settings without launching an external capability.
    pub fn preflight(&self, provider: &dyn crate::Provider) -> Result<(), ConfigError> {
        crate::runtime::validate_run(self.spec(), provider, &self.builtin_tools()?)
            .map_err(|_| error("config_invalid_value", "/run"))
    }
    /// A client may select only exact host definitions; empty selection uses host defaults.
    pub fn select_mcp_client(
        &mut self,
        requests: &[crate::mcp::ClientServer],
    ) -> Result<(), ConfigError> {
        if self.child_scope.is_some() {
            if requests.is_empty() {
                return Ok(());
            }
            let selected = self.deployment.admit_mcp_client(requests)?;
            if selected.iter().any(|id| {
                self.mcp_selection
                    .as_ref()
                    .is_none_or(|inherited| !inherited.contains(id))
            }) {
                return Err(error("config_authority_violation", "/child/mcp_servers"));
            }
            self.mcp_selection = Some(selected);
            return Ok(());
        }
        self.mcp_selection = if requests.is_empty() {
            None
        } else {
            Some(self.deployment.admit_mcp_client(requests)?)
        };
        Ok(())
    }
    /// Exclusive no-follow creation after all other admission checks succeed.
    #[cfg(unix)]
    pub fn create_trace_file(&self) -> Result<Option<std::fs::File>, ConfigError> {
        use rustix::fs::{Mode, OFlags, open, openat};
        let Some(path) = &self.trace_path else {
            return Ok(None);
        };
        let diagnostic = || error("config_path_unavailable", "/options/trace/path");
        let flags = OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut parent = open(
            "/",
            flags | OFlags::RDONLY | OFlags::DIRECTORY,
            Mode::empty(),
        )
        .map_err(|_| diagnostic())?;
        let components: Vec<_> = path
            .components()
            .filter(|c| !matches!(c, Component::RootDir | Component::CurDir))
            .collect();
        for (index, component) in components.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(diagnostic());
            };
            let last = index + 1 == components.len();
            let next = openat(
                &parent,
                *name,
                flags
                    | if last {
                        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL
                    } else {
                        OFlags::RDONLY | OFlags::DIRECTORY
                    },
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|_| diagnostic())?;
            if last {
                return Ok(Some(std::fs::File::from(next)));
            }
            parent = next;
        }
        Err(diagnostic())
    }
    pub fn spec(&self) -> &RunSpec {
        &self.spec
    }
    pub fn deployment(&self) -> &ResolvedDeployment {
        &self.deployment
    }
    pub fn trace_path(&self) -> Option<&Path> {
        self.trace_path.as_deref()
    }
    /// Host-only identity; never send to the model or an OTel exporter.
    pub fn bindings_fingerprint(&self) -> &str {
        &self.bindings_fingerprint
    }
    pub fn tools(&self) -> Result<ToolRegistry, ConfigError> {
        if self.has_skills() {
            return Err(error(
                "config_async_skills_required",
                "/options/skills/activate",
            ));
        }
        let settings = self.deployment.mcp()?;
        if settings.servers.iter().any(|(id, _)| {
            self.mcp_selection
                .as_ref()
                .is_none_or(|names| names.contains(id))
                && settings.admit_server(id, &self.policy).is_ok()
        }) {
            return Err(error("config_async_mcp_required", "/options/mcp"));
        }
        let tools = self.builtin_tools()?;
        self.check_child_tools(&tools)?;
        Ok(tools)
    }
    pub(super) fn builtin_tools(&self) -> Result<ToolRegistry, ConfigError> {
        let options = self.deployment.options();
        let mut tools = ToolRegistry::configured_with_policy_set(
            options["shell"]["enabled"].as_bool().unwrap(),
            options["filesystem"]["enabled"].as_bool().unwrap(),
            options["filesystem"]["write"].as_bool().unwrap(),
            self.policy.clone(),
        )
        .map_err(|_| error("config_invalid_value", "/options/policy"))?
        .with_shell_configuration(
            super::shell::settings(&options["shell"])?,
            super::shell::ceilings(self.deployment.config())?,
        )
        .map_err(|_| error("config_invalid_value", "/options/shell"))?;
        self.restrict_tools(&mut tools);
        Ok(tools)
    }
}

fn directory(path: &Path, option: &str) -> Result<PathBuf, ConfigError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| error("config_path_unavailable", option))?;
    if !canonical.is_dir() || canonical.to_str().is_none() {
        return Err(error("config_path_unavailable", option));
    }
    Ok(canonical)
}

impl ResolvedDeployment {
    pub fn model_profile(&self) -> Result<crate::gateway::ModelProfile, ConfigError> {
        if let Some(route) = &self.model_route {
            return Ok(route.entries()[0].profile().clone());
        }
        let model = &self.options()["model"];
        let kind = model["provider"]
            .as_str()
            .unwrap()
            .parse()
            .map_err(|_| error("config_invalid_value", "/options/model/provider"))?;
        let mut profile = if kind == crate::gateway::GatewayKind::OpenResponses {
            crate::gateway::ModelProfile::responses(responses_profile(model)?)
        } else {
            crate::gateway::ModelProfile::resolve(kind, model["id"].as_str())
                .map_err(|_| error("config_invalid_value", "/options/model/id"))?
        };
        profile.context_window_tokens = model["context_window_tokens"].as_u64();
        Ok(profile)
    }
    pub fn prepare_run(&self, input: RunInput) -> Result<PreparedRun, ConfigError> {
        let options = self.options();
        if self.config["deployment"]["locked"] == true
            && !self.config["deployment"]["allowed_run_overrides"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "input")
        {
            return Err(error("config_override_forbidden", "/input"));
        }
        if !self.model_profile()?.adapter_available {
            let mut diagnostic = error("config_unsupported_feature", "/options/model/provider");
            diagnostic.owner = Some("C3.6");
            return Err(diagnostic);
        }
        let config_root = directory(&self.config_root, "/config_root")?;
        let mut roots = BTreeMap::new();
        for (name, root) in &self.path_bindings {
            roots.insert(name.clone(), directory(root, "/path_bindings")?);
        }
        let bindings_fingerprint =
            fingerprint(&serde_json::json!({"config_root":config_root,"path_bindings":roots}))?;
        let reference = |value: &Value, workspace: Option<&Path>| -> Result<PathBuf, ConfigError> {
            let root = match value["base"].as_str() {
                Some("config") => config_root.as_path(),
                Some("binding") => roots
                    .get(value["name"].as_str().unwrap())
                    .map(PathBuf::as_path)
                    .ok_or_else(|| error("config_path_unavailable", "/path_bindings"))?,
                Some("workspace") => {
                    workspace.ok_or_else(|| error("config_invalid_value", "/path"))?
                }
                _ => return Err(error("config_invalid_value", "/path")),
            };
            Ok(root.join(value["path"].as_str().unwrap()))
        };
        let requested = reference(&options["run"]["workspace"], None)?;
        let original = directory(&requested, "/options/run/workspace")?;
        // Following an alias cannot widen the portable root's authority.
        let mut base_reference = options["run"]["workspace"].clone();
        base_reference["path"] = ".".into();
        let base = reference(&base_reference, None)?;
        if original != requested || !original.starts_with(&base) {
            return Err(error(
                "config_authority_violation",
                "/options/run/workspace",
            ));
        }
        let workspace = if let Some(workspace) = input.workspace {
            if !workspace.is_absolute()
                || workspace
                    .components()
                    .any(|c| matches!(c, Component::ParentDir))
            {
                return Err(error("config_invalid_value", "/options/run/workspace"));
            }
            let workspace = directory(&workspace, "/options/run/workspace")?;
            if workspace != original {
                if self.config["deployment"]["locked"] == true
                    && !self.config["deployment"]["allowed_run_overrides"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v == "run.workspace")
                {
                    return Err(error("config_override_forbidden", "/options/run/workspace"));
                }
                if !workspace.starts_with(&original) {
                    return Err(error(
                        "config_authority_violation",
                        "/options/run/workspace",
                    ));
                }
                let mut value = options["run"]["workspace"].clone();
                let relative = workspace.strip_prefix(&original).unwrap().to_str().unwrap();
                value["path"] =
                    input::relative(&format!("{}/{}", value["path"].as_str().unwrap(), relative))?
                        .into();
                let deployment =
                    self.with_overrides([("run.workspace".into(), value)].into_iter().collect())?;
                return deployment.prepare_run(RunInput {
                    input: input.input,
                    session_id: input.session_id,
                    workspace: None,
                });
            }
            workspace
        } else {
            original
        };
        for layer in self.config["authority"].as_array().unwrap() {
            if let Some(allowed) = layer.get("workspace_roots") {
                let mut admitted = false;
                for root in allowed.as_array().unwrap() {
                    let requested_root = reference(root, Some(&workspace))?;
                    let root = directory(&requested_root, "/authority/workspace_roots")?;
                    if root != requested_root {
                        return Err(error(
                            "config_path_unavailable",
                            "/authority/workspace_roots",
                        ));
                    }
                    admitted |= workspace.starts_with(root);
                }
                if !admitted {
                    let mut e = error("config_authority_violation", "/options/run/workspace");
                    e.authority_id = Some(layer["id"].as_str().unwrap().into());
                    return Err(e);
                }
            }
        }
        if input.session_id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 128
                || id.chars().any(char::is_control)
                || id.contains(['/', '\\'])
                || id == "."
                || id == ".."
        }) {
            return Err(error("config_invalid_value", "/session_id"));
        }
        let mut limits = options["limits"].clone();
        fn counters(value: &mut Value) {
            match value {
                Value::Object(map) => {
                    for value in map.values_mut() {
                        counters(value);
                    }
                }
                Value::String(s) => {
                    *value = if s == "unlimited" {
                        Value::Null
                    } else {
                        s.parse::<u64>().unwrap().into()
                    }
                }
                _ => {}
            }
        }
        counters(&mut limits);
        let mut spec = RunSpec::new(
            input.input,
            workspace,
            self.selected_model()["id"].as_str().unwrap(),
        );
        spec.context = serde_json::from_value(options["context"].clone())
            .map_err(|_| error("config_invalid_value", "/options/context"))?;
        spec.context.window_tokens = self.model_profile()?.context_window_tokens;
        if let Some(schema) = options["output"]["schema"].as_str() {
            spec.output = Some(crate::output::OutputSettings {
                repair: serde_json::from_value(options["output"]["repair"].clone())
                    .map_err(|_| error("config_invalid_value", "/options/output/repair"))?,
                schema: serde_json::from_str(schema)
                    .map_err(|_| error("config_invalid_value", "/options/output/schema"))?,
                max_validation_work: options["output"]["max_validation_work"].as_u64().unwrap(),
            });
        }
        spec.session_id = input.session_id;
        spec.instructions = options["run"]["instructions"].as_str().unwrap().into();
        spec.limits = serde_json::from_value(limits)
            .map_err(|_| error("config_invalid_value", "/options/limits"))?;
        spec.trace.capture_content = options["trace"]["capture_content"].as_bool().unwrap();
        spec.trace.max_bytes = options["trace"]["max_bytes"].as_u64().unwrap() as usize;
        let ordinary: Policy = serde_json::from_value(options["policy"].clone())
            .map_err(|_| error("config_invalid_value", "/options/policy"))?;
        let ceilings = self.config["authority"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|layer| layer.get("policy"))
            .map(|policy| serde_json::from_value(policy.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| error("config_invalid_value", "/authority/policy"))?;
        let policy = PolicySet::new(ordinary, ceilings)
            .map_err(|_| error("config_invalid_value", "/options/policy"))?;
        let mcp = self.mcp()?;
        for (id, server) in &mcp.servers {
            if mcp.admit_server(id, &policy).is_err() {
                if server.required() {
                    return Err(error("config_authority_violation", "/options/mcp"));
                }
                continue;
            }
        }
        #[cfg(unix)]
        if options["filesystem"]["enabled"] == true {
            crate::filesystem::Workspace::new(&spec.workspace, &policy)
                .map_err(|_| error("config_path_unavailable", "/options/policy"))?;
        }
        let trace_path = if options["trace"]["path"]["unset"] == true {
            None
        } else {
            let path = reference(&options["trace"]["path"], Some(&spec.workspace))?;
            let value = path
                .to_str()
                .ok_or_else(|| error("config_invalid_value", "/options/trace/path"))?;
            let path = if value.contains("{session_id}") {
                PathBuf::from(
                    value.replace(
                        "{session_id}",
                        spec.session_id
                            .as_deref()
                            .ok_or_else(|| error("config_invalid_value", "/session_id"))?,
                    ),
                )
            } else {
                path
            };
            let parent = path
                .parent()
                .ok_or_else(|| error("config_invalid_value", "/options/trace/path"))?;
            if directory(parent, "/options/trace/path")? != parent
                || path.symlink_metadata().is_ok()
            {
                return Err(error("config_path_unavailable", "/options/trace/path"));
            }
            Some(path)
        };
        if trace_path.is_some() {
            crate::JsonlSink::new(std::io::sink(), &spec)
                .map_err(|_| error("config_invalid_value", "/options/trace/max_bytes"))?;
        }
        Ok(PreparedRun {
            child_scope: None,
            mcp_selection: None,
            config_root,
            path_bindings: roots,
            deployment: self.clone(),
            spec,
            policy,
            trace_path,
            bindings_fingerprint,
        })
    }
}

pub(super) fn responses_profile(
    model: &Value,
) -> Result<crate::gateway::OpenResponsesProfile, ConfigError> {
    crate::gateway::OpenResponsesProfile::new(
        model["endpoint"].as_str().unwrap_or(""),
        model["id"].as_str().unwrap_or(""),
        model["capability_profile"].as_str().unwrap_or(""),
        model["auth_header"].as_str().unwrap_or("Authorization"),
        model["auth_scheme"].as_str().unwrap_or("bearer"),
    )
    .map_err(|_| error("config_invalid_value", "/options/model"))
}
