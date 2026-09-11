//! Read-only event identity. Decoding this projection never grants an AgentRef
//! or a ledger registration; only bind_execution creates execution authority.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentIdentity {
    pub(super) agent_id: String,
    pub(super) root_run_id: String,
    pub(super) root_session_id: String,
    pub(super) parent_agent_id: Option<String>,
    pub(super) kind: super::AgentKind,
    pub(super) depth: u8,
    pub(super) session_id: String,
}
impl AgentIdentity {
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
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn depth(&self) -> u8 {
        self.depth
    }
    pub fn kind(&self) -> super::AgentKind {
        self.kind
    }
    /// Includes worst-case JSON escaping plus fixed keys, kind, depth and punctuation.
    pub fn json_upper_bound(&self) -> usize {
        [
            &self.agent_id,
            &self.root_run_id,
            &self.root_session_id,
            &self.session_id,
        ]
        .into_iter()
        .fold(
            self.parent_agent_id.as_ref().map_or(0, String::len),
            |sum, s| sum.saturating_add(s.len()),
        )
        .saturating_mul(6)
        .saturating_add(512)
    }
    pub(crate) fn attributes(&self) -> Vec<opentelemetry::KeyValue> {
        use opentelemetry::KeyValue;
        let mut attributes = vec![
            KeyValue::new("pablo.agent.id", self.agent_id.clone()),
            KeyValue::new("pablo.root.run.id", self.root_run_id.clone()),
            KeyValue::new("pablo.root.session.id", self.root_session_id.clone()),
            KeyValue::new("pablo.agent.session.id", self.session_id.clone()),
            KeyValue::new("pablo.agent.depth", i64::from(self.depth)),
            KeyValue::new(
                "pablo.agent.kind",
                match self.kind {
                    super::AgentKind::Root => "root",
                    super::AgentKind::LocalAcpTemporary => "local_acp_temporary",
                },
            ),
        ];
        if let Some(parent) = &self.parent_agent_id {
            attributes.push(KeyValue::new("pablo.parent.agent.id", parent.clone()));
        }
        attributes
    }
}

pub struct ExecutionIdentity {
    pub run_id: String,
    pub agent: AgentIdentity,
}
