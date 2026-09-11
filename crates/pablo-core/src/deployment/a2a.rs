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

impl ResolvedDeployment {
    /// Retrieve only a configured public card and create a fresh local proxy.
    /// No credential lookup, task submission or ledger registration occurs here.
    pub async fn resolve_a2a(
        &self,
        parent: &crate::children::AgentRef,
        name: &str,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<crate::a2a::RemoteProxy, crate::a2a::ResolveError> {
        self.resolve_a2a_inner(parent, name, None, deadline, cancellation)
            .await
    }
    /// Explicit host fixture override; original configured authority is checked
    /// before a literal loopback transport can be selected.
    #[doc(hidden)]
    pub async fn resolve_a2a_fixture(
        &self,
        parent: &crate::children::AgentRef,
        name: &str,
        loopback: &str,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<crate::a2a::RemoteProxy, crate::a2a::ResolveError> {
        self.resolve_a2a_inner(parent, name, Some(loopback), deadline, cancellation)
            .await
    }
    /// Offline named admission for a supervisor queue. No HTTP, credential lookup,
    /// ledger reservation or task submission occurs until the owner promotes it.
    pub fn prepare_a2a(
        &self,
        parent: &crate::children::AgentRef,
        name: &str,
    ) -> Result<crate::a2a::PendingRemote, crate::a2a::ResolveError> {
        use crate::a2a::{PendingRemote, ResolveError};
        let settings = self.a2a().map_err(ResolveError::Configuration)?;
        let remote = settings
            .remotes
            .get(name)
            .ok_or_else(|| {
                ResolveError::Configuration(error(
                    "config_authority_violation",
                    "/options/a2a/remotes",
                ))
            })?
            .clone();
        let agent = parent.remote_proxy().map_err(ResolveError::Ownership)?;
        Ok(PendingRemote::new(name.into(), agent, remote))
    }
    async fn resolve_a2a_inner(
        &self,
        parent: &crate::children::AgentRef,
        name: &str,
        loopback: Option<&str>,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<crate::a2a::RemoteProxy, crate::a2a::ResolveError> {
        let pending = self.prepare_a2a(parent, name)?;
        match loopback {
            Some(target) => {
                pending
                    .resolve_fixture(target, deadline, cancellation)
                    .await
            }
            None => pending.resolve(deadline, cancellation).await,
        }
    }
}
