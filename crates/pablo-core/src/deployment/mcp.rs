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
