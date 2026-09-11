//! Atomic root/agent accounting. Admission hooks are installed by supervision,
//! never by a child independently constructing a fresh remaining-budget view.
use super::AgentRef;
use crate::{
    LimitKind, RunLimits, RunOutcome, Usage,
    provider::AccountingBounds,
    task::{Accounting, Ledger},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Debug)]
pub enum AdmissionError {
    Closed,
    UnknownAgent,
    InvalidCeiling,
    Capacity,
    Unattested,
    Cancelled,
    TimedOut,
    Outcome(RunOutcome),
}
#[derive(Clone)]
pub struct RootLedger(Arc<Mutex<State>>);
struct Agent {
    execution: Option<(String, String)>,
    limits: RunLimits,
    accounting: Accounting,
}
struct PendingModel {
    agent_id: String,
    root: Ledger,
    local: Ledger,
}
struct State {
    root_id: String,
    root_run_id: String,
    root_session_id: String,
    limits: RunLimits,
    deadline: tokio::time::Instant,
    accounting: Accounting,
    agents: BTreeMap<String, Agent>,
    pending: BTreeMap<u64, PendingModel>,
    next_reservation: u64,
    closed: bool,
    closed_token: crate::CancellationToken,
    resources: resources::ResourceState,
    events: events::EventState,
    mutation: Arc<tokio::sync::Mutex<()>>,
}
/// One owned admission. Settle after the provider operation is closed/joined.
/// Abandonment retains uncertain liability rather than inventing a refund.
pub struct ModelReservation {
    ledger: RootLedger,
    id: Option<u64>,
}
fn initial(limits: &RunLimits) -> Accounting {
    Accounting {
        charged_tokens: limits.max_total_tokens.map(|_| 0),
        charged_cost_microusd: limits.max_cost_microusd.map(|_| 0),
        ..Accounting::default()
    }
}
fn increment(current: u64, cap: Option<u32>, kind: LimitKind) -> Result<u64, AdmissionError> {
    current
        .checked_add(1)
        .filter(|n| cap.is_none_or(|cap| *n <= u64::from(cap)))
        .ok_or(AdmissionError::Outcome(RunOutcome::LimitExceeded {
            limit: kind,
        }))
}
impl RootLedger {
    pub fn new(root: &AgentRef, limits: RunLimits) -> Result<Self, AdmissionError> {
        if limits.max_events == 0 {
            return Err(AdmissionError::InvalidCeiling);
        }
        if root.depth() != 0 || root.parent_agent_id().is_some() {
            return Err(AdmissionError::UnknownAgent);
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(limits.max_run_duration_ms))
            .ok_or(AdmissionError::InvalidCeiling)?;
        let accounting = initial(&limits);
        let agents = [(
            root.agent_id().to_owned(),
            Agent {
                execution: None,
                limits: limits.clone(),
                accounting: accounting.clone(),
            },
        )]
        .into();
        Ok(Self(Arc::new(Mutex::new(State {
            root_id: root.agent_id().into(),
            root_run_id: root.root_run_id().into(),
            root_session_id: root.root_session_id().into(),
            limits,
            deadline,
            accounting,
            agents,
            pending: BTreeMap::new(),
            next_reservation: 0,
            closed: false,
            closed_token: crate::CancellationToken::new(),
            resources: resources::ResourceState::default(),
            events: events::EventState::new(root.agent_id()),
            mutation: Arc::new(tokio::sync::Mutex::new(())),
        }))))
    }
    /// Check a data-only event projection against an already bound execution.
    pub fn event_source_matches(&self, event: &crate::RunEvent) -> bool {
        let Some(agent) = &event.agent else {
            return false;
        };
        let s = self.0.lock().unwrap();
        let root = agent.agent_id() == s.root_id;
        agent.root_run_id() == s.root_run_id
            && agent.root_session_id() == s.root_session_id
            && agent.session_id() == event.session_id
            && agent.depth() == u8::from(!root)
            && agent.parent_agent_id() == (!root).then_some(s.root_id.as_str())
            && agent.kind()
                == if root {
                    super::AgentKind::Root
                } else {
                    super::AgentKind::LocalAcpTemporary
                }
            && s.agents
                .get(agent.agent_id())
                .and_then(|a| a.execution.as_ref())
                .is_some_and(|(run, session)| *run == event.run_id && *session == event.session_id)
    }
    /// Bind one registered agent to one native execution and its ACP/host session.
    /// Root ownership comes from this ledger, never incoming trace metadata.
    pub fn bind_execution(
        &self,
        agent_id: &str,
        session_id: Option<&str>,
    ) -> Result<super::ExecutionIdentity, AdmissionError> {
        let mut s = self.0.lock().unwrap();
        let registered = s.agents.get(agent_id).ok_or(AdmissionError::UnknownAgent)?;
        if registered.execution.is_some() {
            return Err(AdmissionError::UnknownAgent);
        }
        let is_root = agent_id == s.root_id;
        if is_root && session_id.is_some_and(|id| id != s.root_session_id) {
            return Err(AdmissionError::UnknownAgent);
        }
        let session_id = if is_root {
            s.root_session_id.clone()
        } else {
            session_id
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        };
        let bounded =
            |id: &str| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control);
        if !bounded(&s.root_run_id) || !bounded(&s.root_session_id) || !bounded(&session_id) {
            return Err(AdmissionError::UnknownAgent);
        }
        let run_id = if is_root {
            s.root_run_id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let agent = super::AgentIdentity {
            agent_id: agent_id.into(),
            root_run_id: s.root_run_id.clone(),
            root_session_id: s.root_session_id.clone(),
            parent_agent_id: (!is_root).then(|| s.root_id.clone()),
            kind: if is_root {
                super::AgentKind::Root
            } else {
                super::AgentKind::LocalAcpTemporary
            },
            depth: u8::from(!is_root),
            session_id: session_id.clone(),
        };
        s.agents.get_mut(agent_id).unwrap().execution = Some((run_id.clone(), session_id));
        Ok(super::ExecutionIdentity { run_id, agent })
    }
    /// Register counters only after supervisor authority/capacity admission.
    /// This does not start a task or grant capabilities.
    pub fn register_child(
        &self,
        child: &AgentRef,
        limits: RunLimits,
    ) -> Result<(), AdmissionError> {
        let mut s = self.0.lock().unwrap();
        Self::register_child_locked(&mut s, child, limits)
    }
    fn register_child_locked(
        s: &mut State,
        child: &AgentRef,
        limits: RunLimits,
    ) -> Result<(), AdmissionError> {
        if s.closed {
            return Err(AdmissionError::Closed);
        }
        if child.depth() != 1
            || child.parent_agent_id() != Some(s.root_id.as_str())
            || child.root_run_id() != s.root_run_id
            || child.root_session_id() != s.root_session_id
            || s.agents.contains_key(child.agent_id())
        {
            return Err(AdmissionError::UnknownAgent);
        }
        if s.agents.len() > super::MAX_TOTAL {
            return Err(AdmissionError::Capacity);
        }
        if !super::limits::inherits(&s.limits, &limits) {
            return Err(AdmissionError::InvalidCeiling);
        }
        s.agents.insert(
            child.agent_id().into(),
            Agent {
                execution: None,
                accounting: initial(&limits),
                limits,
            },
        );
        Ok(())
    }
    /// Atomically register an authority-admitted child and its initial capacity.
    /// A rejected reservation consumes neither a child identity nor total slots.
    pub fn admit_child(
        &self,
        child: &AgentRef,
        limits: RunLimits,
        capacity: resources::Resources,
    ) -> Result<resources::ResourceLease, AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if s.closed {
            return Err(AdmissionError::Closed);
        }
        if tokio::time::Instant::now() >= s.deadline {
            return Err(AdmissionError::TimedOut);
        }
        Self::register_child_locked(&mut s, child, limits)?;
        match resources::reserve_locked(self, &mut s, child.agent_id(), capacity) {
            Ok(lease) => Ok(lease),
            Err(error) => {
                s.agents.remove(child.agent_id());
                Err(error)
            }
        }
    }
    pub fn reserve_model(
        &self,
        agent_id: &str,
        bounds: AccountingBounds,
    ) -> Result<ModelReservation, AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if s.closed {
            return Err(AdmissionError::Closed);
        }
        if s.pending.len() >= 3 || s.pending.values().any(|p| p.agent_id == agent_id) {
            return Err(AdmissionError::Capacity);
        }
        let agent = s.agents.get(agent_id).ok_or(AdmissionError::UnknownAgent)?;
        let root = Ledger::for_bounds(
            s.limits.max_total_tokens,
            s.limits.max_cost_microusd,
            bounds,
        )
        .map_err(|_| AdmissionError::Unattested)?;
        let local = Ledger::for_bounds(
            agent.limits.max_total_tokens,
            agent.limits.max_cost_microusd,
            bounds,
        )
        .map_err(|_| AdmissionError::Unattested)?;
        let mut total = s.accounting.clone();
        let mut own = agent.accounting.clone();
        total.model_calls = increment(
            total.model_calls,
            s.limits.max_model_calls,
            LimitKind::ModelCalls,
        )?;
        own.model_calls = increment(
            own.model_calls,
            agent.limits.max_model_calls,
            LimitKind::ModelCalls,
        )?;
        root.reserve(&mut total).map_err(AdmissionError::Outcome)?;
        local.reserve(&mut own).map_err(AdmissionError::Outcome)?;
        let next = s
            .next_reservation
            .checked_add(1)
            .ok_or(AdmissionError::Capacity)?;
        // Commit all counters/charges only once every check has passed.
        s.accounting = total;
        s.agents.get_mut(agent_id).unwrap().accounting = own;
        s.next_reservation = next;
        s.pending.insert(
            next,
            PendingModel {
                agent_id: agent_id.into(),
                root,
                local,
            },
        );
        Ok(ModelReservation {
            ledger: self.clone(),
            id: Some(next),
        })
    }
    pub fn admit_tool(&self, agent_id: &str) -> Result<(), AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if s.closed {
            return Err(AdmissionError::Closed);
        }
        let agent = s.agents.get(agent_id).ok_or(AdmissionError::UnknownAgent)?;
        let root = increment(
            s.accounting.tool_calls,
            s.limits.max_tool_calls,
            LimitKind::ToolCalls,
        )?;
        let own = increment(
            agent.accounting.tool_calls,
            agent.limits.max_tool_calls,
            LimitKind::ToolCalls,
        )?;
        s.accounting.tool_calls = root;
        s.agents.get_mut(agent_id).unwrap().accounting.tool_calls = own;
        Ok(())
    }
    /// One root clock includes subsequent child admission and queue time.
    pub fn deadline(&self) -> tokio::time::Instant {
        self.0.lock().unwrap().deadline
    }
    pub fn total(&self) -> Accounting {
        self.0.lock().unwrap().accounting.clone()
    }
    pub fn agent(&self, id: &str) -> Option<Accounting> {
        self.0
            .lock()
            .unwrap()
            .agents
            .get(id)
            .map(|a| a.accounting.clone())
    }
    pub fn close_admission(&self) {
        let mut state = self.0.lock().unwrap();
        state.closed = true;
        state.closed_token.cancel();
    }
    fn settle(
        &self,
        id: u64,
        usage: &Usage,
        cost: Option<u64>,
        not_sent: bool,
    ) -> Result<(), RunOutcome> {
        let mut s = self.0.lock().unwrap();
        let reservation = s.pending.remove(&id).expect("one owned model settlement");
        let root = reservation
            .root
            .settle(&mut s.accounting, usage, cost, not_sent);
        let local = reservation.local.settle(
            &mut s.agents.get_mut(&reservation.agent_id).unwrap().accounting,
            usage,
            cost,
            not_sent,
        );
        root.and(local)
    }
}
impl ModelReservation {
    pub fn settle(
        mut self,
        usage: &Usage,
        cost: Option<u64>,
        not_sent: bool,
    ) -> Result<(), RunOutcome> {
        self.ledger
            .settle(self.id.take().unwrap(), usage, cost, not_sent)
    }
}
impl Drop for ModelReservation {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.ledger.settle(id, &Usage::default(), None, false);
        }
    }
}

#[cfg(test)]
mod tests;

pub mod resources;

pub mod events;
