//! Cumulative native event capacity with owned closing slots. Reserved events may
//! settle after admission closes; failed delivery never refunds a consumed event.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventCounts {
    pub used: u64,
    pub reserved: u64,
}
#[derive(Default)]
struct AgentEvents {
    counts: EventCounts,
    terminal_claimed: bool,
}
struct Entry {
    agent_id: String,
    remaining: u64,
}
pub(super) struct EventState {
    counts: EventCounts,
    agents: BTreeMap<String, AgentEvents>,
    entries: BTreeMap<u64, Entry>,
    next: u64,
}
impl EventState {
    pub(super) fn new(root_id: &str) -> Self {
        // A host may start a child before driving the root. Protect root settlement now.
        let counts = EventCounts {
            used: 0,
            reserved: 1,
        };
        Self {
            counts,
            agents: [(
                root_id.into(),
                AgentEvents {
                    counts,
                    terminal_claimed: false,
                },
            )]
            .into(),
            entries: BTreeMap::new(),
            next: 0,
        }
    }
}
pub struct EventReservation {
    ledger: RootLedger,
    id: u64,
}
fn exhausted() -> AdmissionError {
    AdmissionError::Outcome(RunOutcome::LimitExceeded {
        limit: LimitKind::Events,
    })
}
fn check(s: &State, agent_id: &str, count: u64) -> Result<(), AdmissionError> {
    if s.closed {
        return Err(AdmissionError::Closed);
    }
    let agent = s.agents.get(agent_id).ok_or(AdmissionError::UnknownAgent)?;
    let own = s
        .events
        .agents
        .get(agent_id)
        .map_or(EventCounts::default(), |a| a.counts);
    for (counts, cap) in [
        (s.events.counts, s.limits.max_events),
        (own, agent.limits.max_events),
    ] {
        counts
            .used
            .checked_add(counts.reserved)
            .and_then(|n| n.checked_add(count))
            .filter(|n| *n <= cap)
            .ok_or_else(exhausted)?;
    }
    Ok(())
}
fn reserve(
    ledger: &RootLedger,
    s: &mut State,
    agent_id: &str,
    count: u64,
    prepaid: bool,
) -> Result<EventReservation, AdmissionError> {
    if count == 0 {
        return Err(AdmissionError::InvalidCeiling);
    }
    if !prepaid {
        check(s, agent_id, count)?;
    }
    let id = s.events.next.checked_add(1).ok_or_else(exhausted)?;
    let own = s.events.agents.entry(agent_id.into()).or_default();
    if !prepaid {
        own.counts.reserved += count;
        s.events.counts.reserved += count;
    }
    s.events.next = id;
    s.events.entries.insert(
        id,
        Entry {
            agent_id: agent_id.into(),
            remaining: count,
        },
    );
    Ok(EventReservation {
        ledger: ledger.clone(),
        id,
    })
}
impl RootLedger {
    pub fn event_counts(&self) -> EventCounts {
        self.0.lock().unwrap().events.counts
    }
    /// Called once before this agent's run span starts.
    pub fn claim_run_event(&self, agent_id: &str) -> Result<EventReservation, AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if !s.agents.contains_key(agent_id)
            || s.events
                .agents
                .get(agent_id)
                .is_some_and(|a| a.terminal_claimed)
        {
            return Err(AdmissionError::UnknownAgent);
        }
        let prepaid = agent_id == s.root_id;
        let reservation = reserve(self, &mut s, agent_id, 1, prepaid)?;
        s.events.agents.get_mut(agent_id).unwrap().terminal_claimed = true;
        Ok(reservation)
    }
    /// Reserve start/finish together before creating an operation's span or effect.
    pub fn reserve_events(
        &self,
        agent_id: &str,
        count: u64,
    ) -> Result<EventReservation, AdmissionError> {
        reserve(self, &mut self.0.lock().unwrap(), agent_id, count, false)
    }
    pub fn admit_event(&self, agent_id: &str) -> Result<(), AdmissionError> {
        let mut s = self.0.lock().unwrap();
        check(&s, agent_id, 1)?;
        s.events.counts.used += 1;
        s.events
            .agents
            .entry(agent_id.into())
            .or_default()
            .counts
            .used += 1;
        Ok(())
    }
}
impl EventReservation {
    pub fn consume(&mut self) -> Result<(), AdmissionError> {
        let mut s = self.ledger.0.lock().unwrap();
        let entry = s
            .events
            .entries
            .get_mut(&self.id)
            .ok_or(AdmissionError::UnknownAgent)?;
        if entry.remaining == 0 {
            return Err(AdmissionError::UnknownAgent);
        }
        entry.remaining -= 1;
        let agent_id = entry.agent_id.clone();
        s.events.counts.reserved -= 1;
        s.events.counts.used += 1;
        let own = &mut s.events.agents.get_mut(&agent_id).unwrap().counts;
        own.reserved -= 1;
        own.used += 1;
        Ok(())
    }
}
impl Drop for EventReservation {
    fn drop(&mut self) {
        let mut s = self.ledger.0.lock().unwrap();
        if let Some(entry) = s.events.entries.remove(&self.id) {
            s.events.counts.reserved -= entry.remaining;
            s.events
                .agents
                .get_mut(&entry.agent_id)
                .unwrap()
                .counts
                .reserved -= entry.remaining;
        }
    }
}
