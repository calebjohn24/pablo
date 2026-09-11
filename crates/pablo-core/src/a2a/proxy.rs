//! An owned local description of a remote peer, not evidence of remote execution.
use super::ValidatedCard;
use crate::children::AgentRef;
use serde::Serialize;

#[derive(Debug)]
pub enum ResolveError {
    Configuration(crate::deployment::ConfigError),
    Fetch(super::FetchError),
    Ownership(crate::children::ContractError),
}
pub const MAX_REMOTE_ID_BYTES: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityError {
    Invalid,
    Changed,
}

/// Remote identifiers remain a separate namespace from local AgentRef/session IDs.
/// They are absent at card admission; a card never invents a remote task/context.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RemoteIdentity {
    pub context_id: Option<String>,
    pub task_id: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct RemoteProxy {
    name: String,
    agent: AgentRef,
    card: ValidatedCard,
    remote: RemoteIdentity,
}
impl RemoteProxy {
    pub(crate) fn admitted(name: String, agent: AgentRef, card: ValidatedCard) -> Self {
        Self {
            name,
            agent,
            card,
            remote: RemoteIdentity::default(),
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn agent(&self) -> &AgentRef {
        &self.agent
    }
    pub fn card(&self) -> &ValidatedCard {
        &self.card
    }
    /// Record remote response identity without binding a local session. Later
    /// updates may add a task ID but cannot replace an already observed identity.
    pub fn observe_remote(
        &mut self,
        context_id: &str,
        task_id: Option<&str>,
    ) -> Result<(), IdentityError> {
        let valid = |id: &str| {
            !id.is_empty() && id.len() <= MAX_REMOTE_ID_BYTES && !id.chars().any(char::is_control)
        };
        if !valid(context_id) || task_id.is_some_and(|id| !valid(id)) {
            return Err(IdentityError::Invalid);
        }
        if self
            .remote
            .context_id
            .as_deref()
            .is_some_and(|id| id != context_id)
            || self
                .remote
                .task_id
                .as_deref()
                .zip(task_id)
                .is_some_and(|(old, new)| old != new)
        {
            return Err(IdentityError::Changed);
        }
        self.remote
            .context_id
            .get_or_insert_with(|| context_id.into());
        if let Some(task_id) = task_id {
            self.remote.task_id.get_or_insert_with(|| task_id.into());
        }
        Ok(())
    }
    pub fn remote(&self) -> &RemoteIdentity {
        &self.remote
    }
}

/// Offline admitted remote ownership. Reserve root capacity before fetching a
/// card; resolving this ticket preserves the queued local identity.
#[derive(Debug)]
pub struct PendingRemote {
    name: String,
    agent: AgentRef,
    remote: super::settings::Remote,
}
impl PendingRemote {
    pub(crate) fn new(name: String, agent: AgentRef, remote: super::settings::Remote) -> Self {
        Self {
            name,
            agent,
            remote,
        }
    }
    pub fn agent(&self) -> &AgentRef {
        &self.agent
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub async fn resolve(
        self,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<RemoteProxy, ResolveError> {
        self.resolve_inner(None, deadline, cancellation).await
    }
    #[doc(hidden)]
    pub async fn resolve_fixture(
        self,
        loopback: &str,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<RemoteProxy, ResolveError> {
        self.resolve_inner(Some(loopback), deadline, cancellation)
            .await
    }
    async fn resolve_inner(
        self,
        loopback: Option<&str>,
        deadline: tokio::time::Instant,
        cancellation: &crate::CancellationToken,
    ) -> Result<RemoteProxy, ResolveError> {
        let selection = self
            .remote
            .admission()
            .map_err(|e| ResolveError::Fetch(super::FetchError::Admission(e)))?;
        let client = super::CardClient::new().map_err(ResolveError::Fetch)?;
        let card = match loopback {
            Some(target) => {
                client
                    .fetch_fixture(
                        &selection,
                        &self.remote.card_url,
                        target,
                        deadline,
                        cancellation,
                    )
                    .await
            }
            None => {
                client
                    .fetch(&selection, &self.remote.card_url, deadline, cancellation)
                    .await
            }
        }
        .map_err(ResolveError::Fetch)?;
        Ok(RemoteProxy::admitted(self.name, self.agent, card))
    }
}
