//! Finite root capacity, independent of cumulative accounting. Leases remain
//! owned until the associated work has joined; replacement is all-or-nothing.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resources {
    pub active_children: usize,
    pub pending_children: usize,
    pub processes: usize,
    pub mcp_sessions: usize,
    pub context_bytes: usize,
    pub queued_input_bytes: usize,
    pub result_bytes: usize,
}
impl Resources {
    fn checked_add(self, rhs: Self) -> Option<Self> {
        Some(Self {
            active_children: self.active_children.checked_add(rhs.active_children)?,
            pending_children: self.pending_children.checked_add(rhs.pending_children)?,
            processes: self.processes.checked_add(rhs.processes)?,
            mcp_sessions: self.mcp_sessions.checked_add(rhs.mcp_sessions)?,
            context_bytes: self.context_bytes.checked_add(rhs.context_bytes)?,
            queued_input_bytes: self
                .queued_input_bytes
                .checked_add(rhs.queued_input_bytes)?,
            result_bytes: self.result_bytes.checked_add(rhs.result_bytes)?,
        })
    }
    fn subtract(self, rhs: Self) -> Self {
        Self {
            active_children: self
                .active_children
                .checked_sub(rhs.active_children)
                .unwrap(),
            pending_children: self
                .pending_children
                .checked_sub(rhs.pending_children)
                .unwrap(),
            processes: self.processes.checked_sub(rhs.processes).unwrap(),
            mcp_sessions: self.mcp_sessions.checked_sub(rhs.mcp_sessions).unwrap(),
            context_bytes: self.context_bytes.checked_sub(rhs.context_bytes).unwrap(),
            queued_input_bytes: self
                .queued_input_bytes
                .checked_sub(rhs.queued_input_bytes)
                .unwrap(),
            result_bytes: self.result_bytes.checked_sub(rhs.result_bytes).unwrap(),
        }
    }
}
struct Entry {
    agent_id: String,
    resources: Resources,
}
#[derive(Default)]
pub(super) struct ResourceState {
    used: Resources,
    next: u64,
    entries: BTreeMap<u64, Entry>,
}
/// Drop only after the leased processes, sessions or owned task have joined.
/// This bookkeeping object does not perform their cleanup.
pub struct ResourceLease {
    ledger: RootLedger,
    id: u64,
}
fn validate(
    s: &State,
    agent_id: &str,
    before: Resources,
    requested: Resources,
) -> Result<Resources, AdmissionError> {
    if s.closed {
        return Err(AdmissionError::Closed);
    }
    let agent = s.agents.get(agent_id).ok_or(AdmissionError::UnknownAgent)?;
    let next = s
        .resources
        .used
        .subtract(before)
        .checked_add(requested)
        .ok_or(AdmissionError::Capacity)?;
    let own = s
        .resources
        .entries
        .values()
        .filter(|e| e.agent_id == agent_id)
        .try_fold(Resources::default(), |sum, e| sum.checked_add(e.resources))
        .ok_or(AdmissionError::Capacity)?
        .subtract(before)
        .checked_add(requested)
        .ok_or(AdmissionError::Capacity)?;
    let is_root = agent_id == s.root_id;
    let context_cap = if is_root {
        agent.limits.max_context_bytes
    } else {
        agent
            .limits
            .max_context_bytes
            .min(crate::children::MAX_CONTEXT_BYTES)
    };
    // C3.22 enables exactly one active child. C3.23 owns the two-child transition.
    if next.active_children > 1
        || next.pending_children > crate::children::MAX_PENDING
        || next.processes > crate::children::MAX_PROCESSES
        || next.mcp_sessions > crate::children::MAX_MCP_SESSIONS
        || next.context_bytes > s.limits.max_context_bytes
        || next.queued_input_bytes > crate::children::MAX_QUEUED_INPUT_BYTES
        || next.result_bytes > crate::children::MAX_RETAINED_RESULT_BYTES
        || own.context_bytes > context_cap
        || own.queued_input_bytes > crate::children::MAX_INPUT_BYTES
        || own.result_bytes > crate::children::MAX_RESULT_BYTES
        || own.active_children.saturating_add(own.pending_children) > 1
        || (!is_root
            && (own.processes > 0 || own.mcp_sessions > 0 || own.context_bytes > 0)
            && own.active_children != 1)
        || (own.queued_input_bytes > 0 && own.pending_children != 1)
        || (is_root
            && (own.active_children != 0
                || own.pending_children != 0
                || own.queued_input_bytes != 0
                || own.result_bytes != 0))
    {
        return Err(AdmissionError::Capacity);
    }
    Ok(next)
}
impl RootLedger {
    pub fn reserve_resources(
        &self,
        agent_id: &str,
        requested: Resources,
    ) -> Result<ResourceLease, AdmissionError> {
        let mut s = self.0.lock().unwrap();
        reserve_locked(self, &mut s, agent_id, requested)
    }
    pub fn resources(&self) -> Resources {
        self.0.lock().unwrap().resources.used
    }
}
pub(super) fn reserve_locked(
    ledger: &RootLedger,
    s: &mut State,
    agent_id: &str,
    requested: Resources,
) -> Result<ResourceLease, AdmissionError> {
    if requested == Resources::default() || s.resources.entries.len() >= 128 {
        return Err(AdmissionError::Capacity);
    }
    let next = validate(s, agent_id, Resources::default(), requested)?;
    let id = s
        .resources
        .next
        .checked_add(1)
        .ok_or(AdmissionError::Capacity)?;
    s.resources.next = id;
    s.resources.used = next;
    s.resources.entries.insert(
        id,
        Entry {
            agent_id: agent_id.into(),
            resources: requested,
        },
    );
    Ok(ResourceLease {
        ledger: ledger.clone(),
        id,
    })
}
impl RootLedger {
    /// Serialize only native filesystem mutations. Shell/external writes still
    /// require host isolation. The caller retains this guard through joined I/O.
    pub async fn lock_mutation(
        &self,
        agent_id: &str,
        cancellation: &crate::CancellationToken,
        deadline: tokio::time::Instant,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, AdmissionError> {
        let (gate, closed) = {
            let s = self.0.lock().unwrap();
            if s.closed {
                return Err(AdmissionError::Closed);
            }
            if !s.agents.contains_key(agent_id) {
                return Err(AdmissionError::UnknownAgent);
            }
            (s.mutation.clone(), s.closed_token.clone())
        };
        let guard = tokio::select! {
            biased;
            _=closed.cancelled()=>return Err(AdmissionError::Closed),
            _=cancellation.cancelled()=>return Err(AdmissionError::Cancelled),
            _=tokio::time::sleep_until(deadline)=>return Err(AdmissionError::TimedOut),
            guard=gate.lock_owned()=>guard,
        };
        if closed.is_cancelled() {
            return Err(AdmissionError::Closed);
        }
        if cancellation.is_cancelled() {
            return Err(AdmissionError::Cancelled);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AdmissionError::TimedOut);
        }
        Ok(guard)
    }
}
impl ResourceLease {
    /// Read-only proof of this lease's admitted capacity and ownership.
    pub fn covers(&self, ledger: &RootLedger, agent_id: &str, required: Resources) -> bool {
        if !Arc::ptr_eq(&self.ledger.0, &ledger.0) {
            return false;
        }
        let s = self.ledger.0.lock().unwrap();
        s.resources.entries.get(&self.id).is_some_and(|entry| {
            let Resources {
                active_children,
                pending_children,
                processes,
                mcp_sessions,
                context_bytes,
                queued_input_bytes,
                result_bytes,
            } = required;
            entry.agent_id == agent_id
                && entry.resources.active_children >= active_children
                && entry.resources.pending_children >= pending_children
                && entry.resources.processes >= processes
                && entry.resources.mcp_sessions >= mcp_sessions
                && entry.resources.context_bytes >= context_bytes
                && entry.resources.queued_input_bytes >= queued_input_bytes
                && entry.resources.result_bytes >= result_bytes
        })
    }
    /// Verify a promoted lease belongs to this root and this exact child.
    pub fn is_active_child(&self, ledger: &RootLedger, agent_id: &str) -> bool {
        if !std::sync::Arc::ptr_eq(&self.ledger.0, &ledger.0) {
            return false;
        }
        let s = self.ledger.0.lock().unwrap();
        s.resources.entries.get(&self.id).is_some_and(|entry| {
            entry.agent_id == agent_id
                && entry.resources.active_children == 1
                && entry.resources.pending_children == 0
        })
    }
    /// Release joined execution capacity while retaining a bounded result.
    /// Pure reductions remain legal after root admission closes.
    pub fn reduce(&mut self, requested: Resources) -> Result<(), AdmissionError> {
        let mut s = self.ledger.0.lock().unwrap();
        let before = s
            .resources
            .entries
            .get(&self.id)
            .expect("owned capacity lease")
            .resources;
        let Resources {
            active_children,
            pending_children,
            processes,
            mcp_sessions,
            context_bytes,
            queued_input_bytes,
            result_bytes,
        } = before;
        if requested.active_children > active_children
            || requested.pending_children > pending_children
            || requested.processes > processes
            || requested.mcp_sessions > mcp_sessions
            || requested.context_bytes > context_bytes
            || requested.queued_input_bytes > queued_input_bytes
            || requested.result_bytes > result_bytes
        {
            return Err(AdmissionError::Capacity);
        }
        s.resources.used = s
            .resources
            .used
            .subtract(before)
            .checked_add(requested)
            .unwrap();
        s.resources.entries.get_mut(&self.id).unwrap().resources = requested;
        Ok(())
    }

    /// A queued-to-active transition never releases queue capacity before active
    /// admission succeeds. Context/result growth uses the same transaction.
    pub fn replace(&mut self, requested: Resources) -> Result<(), AdmissionError> {
        let mut s = self.ledger.0.lock().unwrap();
        let entry = s
            .resources
            .entries
            .get(&self.id)
            .expect("owned capacity lease");
        let next = validate(&s, &entry.agent_id, entry.resources, requested)?;
        s.resources.used = next;
        s.resources.entries.get_mut(&self.id).unwrap().resources = requested;
        Ok(())
    }
}
impl Drop for ResourceLease {
    fn drop(&mut self) {
        let mut s = self.ledger.0.lock().unwrap();
        let entry = s
            .resources
            .entries
            .remove(&self.id)
            .expect("one capacity release");
        s.resources.used = s.resources.used.subtract(entry.resources);
    }
}

#[cfg(test)]
mod tests;
