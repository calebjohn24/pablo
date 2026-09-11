//! Temporary child contract. No supervisor or delegation tool is installed here.
use serde::{Deserialize, Serialize};

pub mod handoff;

pub const MAX_DEPTH: u8 = 1;
pub const MAX_ACTIVE: usize = 2;
pub const MAX_TOTAL: usize = 16;
pub const MAX_PENDING: usize = 14;
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_QUEUED_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CONTEXT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 64 * 1024;
pub const MAX_RETAINED_RESULT_BYTES: usize = 1024 * 1024;
pub const MAX_PROCESSES: usize = 16;
pub const MAX_MCP_SESSIONS: usize = 16;
pub const MAX_WAIT_MS: u64 = 900_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Root,
    LocalAcpTemporary,
    RemoteA2a,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Queued,
    Starting,
    Running,
    Stopping,
    Settled,
}
/// Runtime-owned identity. Deserialization cannot inject or reparent a handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentRef {
    agent_id: String,
    root_run_id: String,
    root_session_id: String,
    parent_agent_id: Option<String>,
    kind: AgentKind,
    depth: u8,
    state: AgentState,
    session_id: Option<String>,
}
impl AgentRef {
    pub fn root(run_id: String, session_id: String) -> Self {
        Self {
            agent_id: uuid::Uuid::new_v4().to_string(),
            root_run_id: run_id,
            root_session_id: session_id.clone(),
            parent_agent_id: None,
            kind: AgentKind::Root,
            depth: 0,
            state: AgentState::Running,
            session_id: Some(session_id),
        }
    }
    pub fn temporary_child(&self) -> Result<Self, ContractError> {
        self.child(AgentKind::LocalAcpTemporary)
    }
    pub(crate) fn remote_proxy(&self) -> Result<Self, ContractError> {
        self.child(AgentKind::RemoteA2a)
    }
    fn child(&self, kind: AgentKind) -> Result<Self, ContractError> {
        if self.kind != AgentKind::Root || self.depth != 0 || self.state != AgentState::Running {
            return Err(ContractError::Depth);
        }
        Ok(Self {
            agent_id: uuid::Uuid::new_v4().to_string(),
            root_run_id: self.root_run_id.clone(),
            root_session_id: self.root_session_id.clone(),
            parent_agent_id: Some(self.agent_id.clone()),
            kind,
            depth: MAX_DEPTH,
            state: AgentState::Queued,
            session_id: None,
        })
    }
    pub fn kind(&self) -> AgentKind {
        self.kind
    }
    pub fn state(&self) -> AgentState {
        self.state
    }
    /// Lifecycle changes preserve ownership and bind one ACP session exactly once.
    pub fn transition(
        &mut self,
        next: AgentState,
        session: Option<String>,
    ) -> Result<(), ContractError> {
        use AgentState::*;
        let valid = matches!(
            self.kind,
            AgentKind::LocalAcpTemporary | AgentKind::RemoteA2a
        ) && matches!(
            (self.state, next),
            (Queued, Starting | Stopping | Settled)
                | (Starting, Running | Stopping | Settled)
                | (Running, Stopping | Settled)
                | (Stopping, Settled)
        );
        if !valid
            || (next == Running) != session.is_some()
            || session.as_ref().is_some_and(|s| s.is_empty())
            || (session.is_some() && self.session_id.is_some())
        {
            return Err(ContractError::Lifecycle);
        }
        if let Some(session) = session {
            self.session_id = Some(session);
        }
        self.state = next;
        Ok(())
    }
    /// A session admitted concurrently with stop still belongs to this child.
    /// Binding its identity must never restart stopping work.
    pub fn bind_session(&mut self, session: String) -> Result<(), ContractError> {
        if self.state == AgentState::Stopping
            && matches!(
                self.kind,
                AgentKind::LocalAcpTemporary | AgentKind::RemoteA2a
            )
        {
            if session.is_empty() || self.session_id.is_some() {
                return Err(ContractError::Lifecycle);
            }
            self.session_id = Some(session);
            Ok(())
        } else {
            self.transition(AgentState::Running, Some(session))
        }
    }
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
    pub fn root_run_id(&self) -> &str {
        &self.root_run_id
    }
    pub fn root_session_id(&self) -> &str {
        &self.root_session_id
    }
    pub fn parent_agent_id(&self) -> Option<&str> {
        self.parent_agent_id.as_deref()
    }
    pub fn depth(&self) -> u8 {
        self.depth
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractError {
    Lifecycle,
    Depth,
    InputBound,
    InvalidSelection,
    WaitBound,
}

/// All selections resolve against inherited definitions; absent means inherit,
/// while an explicit empty list removes the capability. Never accept credentials.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySelection {
    pub tools: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub mcp_servers: Option<Vec<String>>,
    pub model_route: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildCeilings {
    pub max_model_calls: Option<u32>,
    pub max_tool_calls: Option<u32>,
    pub max_duration_ms: Option<u64>,
    pub max_context_bytes: Option<usize>,
    pub max_output_bytes: Option<usize>,
    // Decimal strings follow the existing accounting contract at JSON boundaries.
    pub max_total_tokens: Option<String>,
    pub max_cost_microusd: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnRequest {
    pub input: String,
    #[serde(default)]
    pub overlay: Option<String>,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub capabilities: CapabilitySelection,
    #[serde(default)]
    pub ceilings: ChildCeilings,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub handoffs: Vec<handoff::HandoffSelection>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitMode {
    Any,
    All,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildAction {
    Spawn {
        request: Box<SpawnRequest>,
    },
    Wait {
        agent_ids: Vec<String>,
        mode: WaitMode,
        timeout_ms: u64,
    },
    Inspect {
        agent_id: String,
    },
    Stop {
        agent_id: String,
    },
}
impl SpawnRequest {
    /// Shape bounds only. C3.22 must additionally intersect authority and reserve
    /// root capacity atomically before any child starts.
    pub fn validate_shape(&self) -> Result<(), ContractError> {
        if self.handoffs.len() > MAX_TOTAL {
            return Err(ContractError::InputBound);
        }
        for handoff in &self.handoffs {
            handoff.validate()?;
        }
        let bytes = self.context.iter().fold(
            self.input
                .len()
                .saturating_add(self.overlay.as_ref().map_or(0, String::len)),
            |n, s| n.saturating_add(s.len()),
        );
        if self.input.trim().is_empty() || bytes > MAX_INPUT_BYTES {
            return Err(ContractError::InputBound);
        }
        if self
            .ceilings
            .max_context_bytes
            .is_some_and(|n| n > MAX_CONTEXT_BYTES)
            || self
                .ceilings
                .max_output_bytes
                .is_some_and(|n| n > MAX_RESULT_BYTES)
        {
            return Err(ContractError::InputBound);
        }
        for value in [
            &self.ceilings.max_total_tokens,
            &self.ceilings.max_cost_microusd,
        ]
        .into_iter()
        .flatten()
        {
            if value.parse::<u64>().is_err()
                || value.is_empty()
                || !value.bytes().all(|c| c.is_ascii_digit())
                || (value.len() > 1 && value.starts_with('0'))
            {
                return Err(ContractError::InvalidSelection);
            }
        }
        for values in [
            &self.capabilities.tools,
            &self.capabilities.skills,
            &self.capabilities.mcp_servers,
            &self.capabilities.model_route,
        ]
        .into_iter()
        .flatten()
        {
            if values.len() > 256
                || values.iter().any(|s| s.is_empty() || s.len() > 512)
                || values
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != values.len()
            {
                return Err(ContractError::InvalidSelection);
            }
        }
        Ok(())
    }
}
impl ChildAction {
    pub fn validate_shape(&self) -> Result<(), ContractError> {
        let valid_id = |id: &String| uuid::Uuid::parse_str(id).is_ok();
        match self {
            Self::Spawn { request } => request.validate_shape(),
            Self::Wait {
                agent_ids,
                timeout_ms,
                ..
            } => {
                if agent_ids.is_empty()
                    || agent_ids.len() > MAX_TOTAL
                    || *timeout_ms > MAX_WAIT_MS
                    || !agent_ids.iter().all(valid_id)
                    || agent_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != agent_ids.len()
                {
                    Err(ContractError::WaitBound)
                } else {
                    Ok(())
                }
            }
            Self::Inspect { agent_id } | Self::Stop { agent_id } => {
                if valid_id(agent_id) {
                    Ok(())
                } else {
                    Err(ContractError::InvalidSelection)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_ownership_and_depth_one_cannot_be_supplied_by_model_actions() {
        let root = AgentRef::root("run".into(), "session".into());
        let child = root.temporary_child().unwrap();
        assert_eq!(child.parent_agent_id(), Some(root.agent_id()));
        assert_eq!(child.root_run_id(), "run");
        assert_eq!(child.root_session_id(), "session");
        assert_eq!(child.depth(), 1);
        assert_ne!(child.agent_id(), root.agent_id());
        assert_eq!(child.temporary_child().unwrap_err(), ContractError::Depth);
        assert!(serde_json::from_value::<ChildAction>(serde_json::json!({"action":"spawn","request":{"input":"task","root_run_id":"foreign"}})).is_err());
        let duplicate = ChildAction::Wait {
            agent_ids: vec![child.agent_id().into(), child.agent_id().into()],
            mode: WaitMode::All,
            timeout_ms: 1,
        };
        assert_eq!(duplicate.validate_shape(), Err(ContractError::WaitBound));
    }
    #[test]
    fn child_lifecycle_binds_session_once_and_settlement_is_terminal() {
        let mut root = AgentRef::root("run".into(), "root-session".into());
        assert_eq!(
            root.transition(AgentState::Settled, None),
            Err(ContractError::Lifecycle)
        );
        let mut child = root.temporary_child().unwrap();
        let original = child.clone();
        assert_eq!(
            child.transition(AgentState::Running, Some("session".into())),
            Err(ContractError::Lifecycle)
        );
        child.transition(AgentState::Starting, None).unwrap();
        assert_eq!(
            child.transition(AgentState::Running, None),
            Err(ContractError::Lifecycle)
        );
        child
            .transition(AgentState::Running, Some("child-session".into()))
            .unwrap();
        assert_eq!(
            child.transition(AgentState::Running, Some("replacement".into())),
            Err(ContractError::Lifecycle)
        );
        child.transition(AgentState::Stopping, None).unwrap();
        child.transition(AgentState::Settled, None).unwrap();
        assert_eq!(
            child.transition(AgentState::Starting, None),
            Err(ContractError::Lifecycle)
        );
        assert_eq!(child.agent_id(), original.agent_id());
        assert_eq!(child.parent_agent_id(), original.parent_agent_id());
        assert_eq!(child.root_run_id(), original.root_run_id());
        assert_eq!(child.session_id.as_deref(), Some("child-session"));
        let mut stopping = root.temporary_child().unwrap();
        stopping.transition(AgentState::Starting, None).unwrap();
        stopping.transition(AgentState::Stopping, None).unwrap();
        stopping
            .bind_session("admitted-during-stop".into())
            .unwrap();
        assert_eq!(stopping.state(), AgentState::Stopping);
        assert_eq!(
            stopping.bind_session("replacement".into()),
            Err(ContractError::Lifecycle)
        );
    }
    #[test]
    fn selected_context_counts_toward_one_child_bound_and_empty_capabilities_stay_empty() {
        let mut request: SpawnRequest =
            serde_json::from_value(serde_json::json!({"input":"task","capabilities":{"tools":[]}}))
                .unwrap();
        assert_eq!(request.capabilities.tools, Some(vec![]));
        assert!(request.capabilities.skills.is_none());
        request.context.push("x".repeat(MAX_INPUT_BYTES - 4));
        assert_eq!(request.validate_shape(), Ok(()));
        request.overlay = Some("x".into());
        assert_eq!(request.validate_shape(), Err(ContractError::InputBound));
    }
}

pub mod ledger;
mod limits;
pub use limits::InvalidChildCeiling;

pub mod owner;

mod identity;
pub use identity::{AgentIdentity, ExecutionIdentity};
