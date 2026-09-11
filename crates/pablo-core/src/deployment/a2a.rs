use super::*;

pub(super) fn settings(config: &Value) -> Result<crate::a2a::Settings, ConfigError> {
    let settings: crate::a2a::Settings =
        serde_json::from_value(config["options"]["a2a"].clone())
            .map_err(|_| error("config_invalid_value", "/options/a2a"))?;
    settings
        .validate()
        .map_err(|_| error("config_invalid_value", "/options/a2a"))?;
    for layer in config["authority"].as_array().unwrap() {
        if let Some(allowed) = layer.get("a2a_remotes") {
            for (name, remote) in &settings.remotes {
                // Match the configured tuple, not independent URL sets that can
                // accidentally authorize a different name/endpoint pairing.
                if allowed.get(name).is_none_or(|entry| {
                    entry["card_url"] != remote.card_url || entry["endpoint"] != remote.endpoint
                }) {
                    let mut e = error("config_authority_violation", "/options/a2a/remotes");
                    e.authority_id = layer["id"].as_str().map(Into::into);
                    return Err(e);
                }
            }
        }
    }
    Ok(settings)
}
impl ResolvedDeployment {
    /// Offline host selection with every immutable authority layer intersected.
    pub fn a2a(&self) -> Result<crate::a2a::Settings, ConfigError> {
        settings(&self.config)
    }
    /// Validate supplied card bytes against an admitted named definition. This
    /// grants no task authority and performs no credential lookup or network I/O.
    pub fn admit_a2a_card(
        &self,
        name: &str,
        bytes: &[u8],
    ) -> Result<crate::a2a::ValidatedCard, ConfigError> {
        let settings = self.a2a()?;
        let remote = settings
            .remotes
            .get(name)
            .ok_or_else(|| error("config_authority_violation", "/options/a2a/remotes"))?;
        remote
            .admission()
            .and_then(|selection| selection.validate(bytes))
            .map_err(|_| error("config_invalid_value", "/options/a2a/remotes"))
    }
}
