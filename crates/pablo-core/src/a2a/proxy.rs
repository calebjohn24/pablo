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
