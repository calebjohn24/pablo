use super::*;

pub(super) fn settings(config: &Value) -> Result<crate::mcp::Settings, ConfigError> {
    let mut settings: crate::mcp::Settings =
        serde_json::from_value(config["options"]["mcp"].clone())
            .map_err(|_| error("config_invalid_value", "/options/mcp"))?;
    for layer in config["authority"].as_array().unwrap() {
        if let Some(policy) = layer.get("mcp") {
            settings.policies.push(
                serde_json::from_value(policy.clone())
                    .map_err(|_| error("config_invalid_value", "/authority/mcp"))?,
            );
        }
    }
    settings
        .validate()
        .map_err(|_| error("config_invalid_value", "/options/mcp"))?;
    Ok(settings)
}

impl ResolvedDeployment {
    fn mcp_policy(&self) -> Result<crate::policy::PolicySet, ConfigError> {
        let ordinary = serde_json::from_value(self.options()["policy"].clone())
            .map_err(|_| error("config_invalid_value", "/options/policy"))?;
        let ceilings = self.config["authority"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|layer| layer.get("policy"))
            .map(|policy| serde_json::from_value(policy.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| error("config_invalid_value", "/authority/policy"))?;
        crate::policy::PolicySet::new(ordinary, ceilings)
            .map_err(|_| error("config_invalid_value", "/options/policy"))
    }

    pub fn admit_mcp_client(
        &self,
        requests: &[crate::mcp::ClientServer],
    ) -> Result<Vec<String>, ConfigError> {
        self.mcp()?
            .admit_client(requests, &self.mcp_policy()?)
            .map_err(|_| error("config_authority_violation", "/options/mcp"))
    }

    /// Immutable host ceilings are intersected at admission, never merged into ordinary policy.
    pub fn mcp(&self) -> Result<crate::mcp::Settings, ConfigError> {
        settings(&self.config)
    }

    pub fn admit_mcp_tool(&self, server: &str, tool: &str) -> Result<Vec<String>, ConfigError> {
        let identity = crate::mcp::qualified(server, tool)
            .map_err(|_| error("config_invalid_value", "/options/mcp"))?;
        for layer in self.config["authority"].as_array().unwrap() {
            if let Some(names) = layer.get("tool_names").and_then(Value::as_array)
                && !names.iter().any(|name| name.as_str() == Some(&identity))
            {
                let mut e = error("config_authority_violation", "/authority/tool_names");
                e.authority_id = layer["id"].as_str().map(Into::into);
                return Err(e);
            }
        }
        self.mcp()?
            .admit_tool(server, tool, &self.mcp_policy()?)
            .map_err(|_| error("config_authority_violation", "/options/mcp"))
    }
}

#[cfg(unix)]
impl PreparedRun {
    /// Start a fresh per-run stdio catalog using only admitted host configuration.
    /// The runtime joins it before its terminal event; callers abandoning a prepared catalog must call close().
    pub async fn tools_with_mcp(
        &self,
        inputs: &dyn CredentialInputs,
        deadline: tokio::time::Instant,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<crate::ToolRegistry, ConfigError> {
        use crate::mcp::{Server, stdio::StdioSession};
        let deadline = deadline.min(
            tokio::time::Instant::now()
                .checked_add(std::time::Duration::from_millis(
                    self.spec().limits.max_run_duration_ms,
                ))
                .ok_or_else(|| {
                    error(
                        "config_invalid_value",
                        "/options/limits/max_run_duration_ms",
                    )
                })?,
        );
        let settings = self.deployment().mcp()?;
        let policy = self.deployment().mcp_policy()?;
        let mut tools = self.builtin_tools()?;
        tools.constrain_deadline(deadline);
        let mut catalog_bytes = 0usize;
        let mut catalog_tools = 0usize;
        for (id, server) in &settings.servers {
            if settings.admit_server(id, &policy).is_err() {
                tools.omit_mcp(id, "policy_denied");
                continue;
            }
            let started = async {
                let Server::Stdio { cwd, .. } = server else {
                    return Err(error("config_unsupported_feature", "/options/mcp"));
                };
                let root = match cwd["base"].as_str().unwrap() {
                    "workspace" => &self.spec().workspace,
                    "config" => &self.config_root,
                    "binding" => self
                        .path_bindings
                        .get(cwd["name"].as_str().unwrap())
                        .ok_or_else(|| error("config_path_unavailable", "/options/mcp"))?,
                    _ => return Err(error("config_path_unavailable", "/options/mcp")),
                };
                let canonical_root = root
                    .canonicalize()
                    .map_err(|_| error("config_path_unavailable", "/options/mcp"))?;
                let cwd = root
                    .join(cwd["path"].as_str().unwrap())
                    .canonicalize()
                    .map_err(|_| error("config_path_unavailable", "/options/mcp"))?;
                if !cwd.is_dir() || !cwd.starts_with(canonical_root) {
                    return Err(error("config_path_unavailable", "/options/mcp"));
                }
                let env = self.mcp_environment(id, server, inputs)?;
                let mut session = StdioSession::start(server, &cwd, env, deadline, cancellation)
                    .await
                    .map_err(|failure| {
                        error(
                            if failure == crate::mcp::stdio::Error::Cleanup {
                                "config_mcp_cleanup"
                            } else {
                                "config_mcp_startup"
                            },
                            "/options/mcp",
                        )
                    })?;
                catalog_bytes = catalog_bytes.saturating_add(session.catalog_bytes());
                catalog_tools = catalog_tools.saturating_add(session.tools().len());
                if catalog_bytes > crate::mcp::MAX_RESULT_BYTES
                    || catalog_tools > crate::mcp::MAX_TOOLS
                {
                    session
                        .close()
                        .await
                        .map_err(|_| error("config_mcp_cleanup", "/options/mcp"))?;
                    return Err(error("config_mcp_catalog_bound", "/options/mcp"));
                }
                let admitted = session
                    .tools()
                    .iter()
                    .filter_map(|tool| {
                        self.deployment()
                            .admit_mcp_tool(id, &tool.name)
                            .ok()
                            .map(|decisions| (tool.name.clone(), decisions))
                    })
                    .collect();
                tools
                    .attach_mcp(id, session, admitted)
                    .await
                    .map_err(|failure| {
                        error(
                            if failure == crate::mcp::stdio::Error::Cleanup {
                                "config_mcp_cleanup"
                            } else {
                                "config_mcp_catalog"
                            },
                            "/options/mcp",
                        )
                    })
            }
            .await;
            if let Err(error) = started {
                if error.code == "config_mcp_cleanup"
                    || server.required()
                    || cancellation.is_cancelled()
                    || tokio::time::Instant::now() >= deadline
                {
                    tools
                        .close()
                        .await
                        .map_err(|_| super::error("config_mcp_cleanup", "/options/mcp"))?;
                    return Err(error);
                }
                tools.omit_mcp(id, error.code);
            }
        }
        Ok(tools)
    }
}
