//! Immutable child selections over an admitted parent. No provider or tool starts
//! here; the supervisor must reserve root capacity before materializing this run.
use super::*;
use crate::{children::SpawnRequest, tool::ToolDescriptor};
use std::collections::BTreeSet;

#[derive(Clone, Serialize)]
pub(super) struct Scope {
    tools: BTreeSet<String>,
    explicit_tools: bool,
    mcp_descriptors: BTreeMap<String, ToolDescriptor>,
    skills: Vec<String>,
    route: Option<ResolvedRoute>,
}

impl PreparedRun {
    /// Prepare one depth-one child from the parent's admitted tool catalog.
    /// The returned run inherits deployment authority, workspace and trusted
    /// instructions. It contains only the selected child task, never parent history.
    /// The host must still reserve capacity and install joined child supervision.
    #[cfg(unix)]
    pub fn prepare_child(
        &self,
        request: &SpawnRequest,
        parent_tools: &ToolRegistry,
    ) -> Result<Self, ConfigError> {
        let denied = |option| error("config_authority_violation", option);
        if self.child_scope.is_some() {
            return Err(denied("/child/depth"));
        }
        if !request.handoffs.is_empty() {
            return Err(denied("/child/handoffs"));
        }
        request
            .validate_shape()
            .map_err(|_| error("config_invalid_value", "/child"))?;
        let limits = request
            .ceilings
            .apply_to(&self.spec.limits)
            .map_err(|_| denied("/child/ceilings"))?;
        let route = match &request.capabilities.model_route {
            Some(names) => Some(
                self.model_route()
                    .ok_or_else(|| denied("/child/model_route"))?
                    .subsequence(&names.iter().map(String::as_str).collect::<Vec<_>>())?,
            ),
            None => self.model_route().cloned(),
        };
        let settings = self.deployment.mcp()?;
        let inherited_servers: BTreeSet<_> = settings
            .servers
            .keys()
            .filter(|id| {
                self.mcp_selection
                    .as_ref()
                    .is_none_or(|names| names.contains(id))
            })
            .filter(|id| settings.admit_server(id, &self.policy).is_ok())
            .cloned()
            .collect();
        let servers = match &request.capabilities.mcp_servers {
            Some(names) => {
                if names.iter().any(|name| !inherited_servers.contains(name)) {
                    return Err(denied("/child/mcp_servers"));
                }
                names.clone()
            }
            None => inherited_servers.into_iter().collect(),
        };
        let skills = request
            .capabilities
            .skills
            .clone()
            .unwrap_or_else(|| self.skill_names());
        if skills.len() > crate::skills::activation::MAX_ACTIVATIONS {
            return Err(denied("/child/skills"));
        }
        if request.capabilities.skills.is_some() {
            let roots = self.deployment.skill_roots(Some(&self.spec.workspace))?;
            for name in &skills {
                let Some((root, package)) = name.split_once('/') else {
                    return Err(denied("/child/skills"));
                };
                if !roots.iter().any(|approved| approved.id == root)
                    || package.is_empty()
                    || package.contains(['/', '\\'])
                    || matches!(package, "." | "..")
                {
                    return Err(denied("/child/skills"));
                }
            }
        }
        let builtins = self.builtin_tools()?;
        let mut available = BTreeSet::new();
        let mut mcp_descriptors = BTreeMap::new();
        for descriptor in parent_tools.descriptors() {
            if self
                .policy
                .decide("tools", &descriptor.name, false)
                .is_err()
            {
                continue;
            }
            if builtins
                .descriptors()
                .iter()
                .any(|tool| tool.name == descriptor.name)
            {
                available.insert(descriptor.name.clone());
            } else if let Some((server, name)) = crate::mcp::split_identity(&descriptor.name)
                && servers.iter().any(|selected| selected == server)
                && self.deployment.admit_mcp_tool(server, name).is_ok()
            {
                available.insert(descriptor.name.clone());
                mcp_descriptors.insert(descriptor.name.clone(), descriptor.clone());
            }
        }
        if !skills.is_empty() && self.policy.decide("tools", "skill.read", false).is_ok() {
            available.insert("skill.read".into());
        }
        // Inactive Skills are absent from the parent's catalog; activating one
        // must still honor host tool-name ceilings on its resource capability.
        available.retain(|name| {
            self.deployment.config()["authority"]
                .as_array()
                .unwrap()
                .iter()
                .all(|layer| {
                    layer
                        .get("tool_names")
                        .and_then(Value::as_array)
                        .is_none_or(|allowed| {
                            allowed.iter().any(|value| value.as_str() == Some(name))
                        })
                })
        });
        let tools = match &request.capabilities.tools {
            Some(names) => {
                if names.iter().any(|name| !available.contains(name)) {
                    return Err(denied("/child/tools"));
                }
                names.iter().cloned().collect()
            }
            None => available,
        };
        mcp_descriptors.retain(|name, _| tools.contains(name));
        let scope = Scope {
            tools,
            explicit_tools: request.capabilities.tools.is_some(),
            mcp_descriptors,
            skills,
            route,
        };
        let mut spec = self.spec.clone();
        spec.limits = limits;
        spec.session_id = None;
        spec.input = if request.overlay.is_none() && request.context.is_empty() {
            request.input.clone()
        } else {
            serde_json::to_string(&serde_json::json!({"task":request.input,"overlay":request.overlay,"context":request.context}))
                .map_err(|_| error("config_invalid_value", "/child/input"))?
        };
        if spec.input.len() > spec.limits.max_input_bytes
            || spec.input.len().saturating_add(spec.instructions.len())
                > spec.limits.max_context_bytes
        {
            return Err(error("config_limit", "/child/input"));
        }
        spec.output = request.output_schema.as_ref().map(|schema| {
            let mut output = crate::output::OutputSettings::new(schema.clone());
            output.repair =
                serde_json::from_value(self.deployment.options()["output"]["repair"].clone())
                    .expect("admitted output repair settings");
            output.max_validation_work = self.deployment.options()["output"]["max_validation_work"]
                .as_u64()
                .expect("admitted output work bound");
            output
        });
        if let Some(output) = &spec.output {
            output
                .compile()
                .map_err(|_| error("config_invalid_value", "/child/output_schema"))?;
        }
        if let Some(route) = &scope.route {
            spec.model = route.entries()[0].profile().model.clone();
            spec.context.window_tokens = route.entries()[0].profile().context_window_tokens;
        }
        let bindings_fingerprint = fingerprint(&serde_json::json!({
            "parent":self.bindings_fingerprint,"child_scope":scope,"servers":servers,"limits":spec.limits
        }))?;
        Ok(Self {
            config_root: self.config_root.clone(),
            path_bindings: self.path_bindings.clone(),
            deployment: self.deployment.clone(),
            spec,
            policy: self.policy.clone(),
            // Child events belong to the root's shared sink; never reopen its path.
            trace_path: None,
            bindings_fingerprint,
            mcp_selection: Some(servers),
            child_scope: Some(scope),
        })
    }

    /// Effective per-run route. Child entries are exact inherited subsequences.
    pub fn model_route(&self) -> Option<&ResolvedRoute> {
        self.child_scope.as_ref().map_or_else(
            || self.deployment.model_route(),
            |scope| scope.route.as_ref(),
        )
    }
    pub fn model_profile(&self) -> Result<crate::gateway::ModelProfile, ConfigError> {
        self.model_route().map_or_else(
            || self.deployment.model_profile(),
            |route| Ok(route.entries()[0].profile().clone()),
        )
    }
    pub(in crate::deployment) fn skill_names(&self) -> Vec<String> {
        self.child_scope.as_ref().map_or_else(
            || {
                self.deployment.options()["skills"]["activate"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| name.as_str().unwrap().to_owned())
                    .collect()
            },
            |scope| scope.skills.clone(),
        )
    }
    pub(in crate::deployment) fn restrict_tools(&self, tools: &mut ToolRegistry) {
        if let Some(scope) = &self.child_scope {
            tools.retain_tools(&scope.tools);
        }
    }
    pub(in crate::deployment) fn check_child_tools(
        &self,
        tools: &ToolRegistry,
    ) -> Result<(), ConfigError> {
        if self.child_scope.as_ref().is_some_and(|scope| {
            scope.explicit_tools
                && scope
                    .tools
                    .iter()
                    .any(|name| !tools.descriptors().iter().any(|tool| &tool.name == name))
        }) {
            return Err(error("config_child_capability_changed", "/child/tools"));
        }
        Ok(())
    }
    #[cfg(unix)]
    pub(in crate::deployment) fn child_mcp_tool(
        &self,
        server: &str,
        tool: &crate::mcp::stdio::CatalogTool,
    ) -> Result<bool, ConfigError> {
        let Some(scope) = &self.child_scope else {
            return Ok(true);
        };
        let name = crate::mcp::qualified(server, &tool.name)
            .map_err(|_| error("config_mcp_catalog", "/options/mcp"))?;
        if !scope.tools.contains(&name) {
            return Ok(false);
        }
        if !scope.mcp_descriptors.get(&name).is_some_and(|expected| {
            expected.description == tool.description && expected.input_schema == tool.input_schema
        }) {
            return Err(error("config_child_capability_changed", "/child/tools"));
        }
        Ok(true)
    }
}

impl PreparedRun {
    /// Check a workspace reference using the same physical path/read authority
    /// and SHA-256 contract as filesystem tools. This never grants a capability.
    #[cfg(unix)]
    pub async fn verify_artifact(
        &self,
        reference: &crate::children::handoff::ArtifactReference,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<(), crate::children::handoff::ArtifactError> {
        use crate::children::handoff::ArtifactError;
        reference
            .validate()
            .map_err(|_| ArtifactError::InvalidReference)?;
        if self.policy.decide("tools", "fs.read", false).is_err()
            || !self
                .builtin_tools()
                .map_err(|_| ArtifactError::Unauthorized)?
                .descriptors()
                .iter()
                .any(|tool| tool.name == "fs.read")
        {
            return Err(ArtifactError::Unauthorized);
        }
        let reference = reference.clone();
        let workspace = self.spec.workspace.clone();
        let policy = self.policy.clone();
        let limits = self.spec.limits.clone();
        let cancellation = cancellation.clone();
        // Retain the join on cancellation, just like ordinary filesystem work.
        tokio::task::spawn_blocking(move || {
            let workspace = crate::filesystem::Workspace::new(&workspace, &policy)
                .map_err(|_| ArtifactError::Unavailable)?;
            workspace.verify_reference(&reference, &policy, &limits, deadline, &cancellation)
        })
        .await
        .map_err(|_| ArtifactError::Unavailable)?
    }
}
