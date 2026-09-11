//! Cumulative native event capacity with owned closing slots. Reserved events may
//! settle after admission closes; failed delivery never refunds a consumed event.
use super::*;
use crate::{RunEvent, TraceSettings, events::jsonl_size};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventCounts {
    pub used: u64,
    pub reserved: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TraceCounts {
    pub used: usize,
    pub reserved: usize,
}
struct TraceBudget {
    settings: TraceSettings,
    counts: TraceCounts,
}
#[derive(Default)]
struct AgentEvents {
    counts: EventCounts,
    terminal_claimed: bool,
}
struct Entry {
    agent_id: String,
    remaining: u64,
    terminal_bytes: usize,
}
pub(super) struct EventState {
    counts: EventCounts,
    agents: BTreeMap<String, AgentEvents>,
    entries: BTreeMap<u64, Entry>,
    next: u64,
    trace: Option<TraceBudget>,
    multiplex: bool,
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
            trace: None,
            multiplex: false,
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
fn trace_exhausted() -> AdmissionError {
    AdmissionError::Outcome(RunOutcome::LimitExceeded {
        limit: LimitKind::TraceBytes,
    })
}
fn terminal_bytes(limits: &RunLimits, settings: &TraceSettings) -> Result<usize, AdmissionError> {
    // Metadata, optional schema diagnostics, and bounded agent identity. Children
    // may select a schema even when the root does not. JSON escaping expands 6x.
    let content = if settings.capture_content {
        limits
            .max_output_bytes
            .checked_mul(6)
            .ok_or_else(trace_exhausted)?
    } else {
        0
    };
    (12 * 1024usize)
        .checked_add(content)
        .ok_or_else(trace_exhausted)
}
fn trace_after(
    s: &State,
    bytes: Option<usize>,
    released: usize,
) -> Result<Option<TraceCounts>, AdmissionError> {
    match (&s.events.trace, bytes) {
        (None, None) => Ok(None),
        (Some(trace), Some(bytes)) => {
            let reserved = trace
                .counts
                .reserved
                .checked_sub(released)
                .ok_or(AdmissionError::InvalidCeiling)?;
            let used = trace
                .counts
                .used
                .checked_add(bytes)
                .ok_or_else(trace_exhausted)?;
            used.checked_add(reserved)
                .filter(|n| *n <= trace.settings.max_bytes)
                .ok_or_else(trace_exhausted)?;
            Ok(Some(TraceCounts { used, reserved }))
        }
        _ => Err(AdmissionError::InvalidCeiling),
    }
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
    terminal: bool,
) -> Result<EventReservation, AdmissionError> {
    if count == 0 {
        return Err(AdmissionError::InvalidCeiling);
    }
    if !prepaid {
        check(s, agent_id, count)?;
    }
    let terminal_bytes = if terminal {
        s.events
            .trace
            .as_ref()
            .map(|trace| terminal_bytes(&s.agents[agent_id].limits, &trace.settings))
            .transpose()?
            .unwrap_or(0)
    } else {
        0
    };
    let trace_reserved = if let Some(trace) = &s.events.trace {
        let reserved = trace
            .counts
            .reserved
            .checked_add(if prepaid { 0 } else { terminal_bytes })
            .ok_or_else(trace_exhausted)?;
        trace
            .counts
            .used
            .checked_add(reserved)
            .filter(|n| *n <= trace.settings.max_bytes)
            .ok_or_else(trace_exhausted)?;
        Some(reserved)
    } else {
        None
    };
    let id = s.events.next.checked_add(1).ok_or_else(exhausted)?;
    let own = s.events.agents.entry(agent_id.into()).or_default();
    if !prepaid {
        own.counts.reserved += count;
        s.events.counts.reserved += count;
    }
    if let Some(reserved) = trace_reserved {
        s.events.trace.as_mut().unwrap().counts.reserved = reserved;
    }
    s.events.next = id;
    s.events.entries.insert(
        id,
        Entry {
            agent_id: agent_id.into(),
            remaining: count,
            terminal_bytes,
        },
    );
    Ok(EventReservation {
        ledger: ledger.clone(),
        id,
    })
}
impl RootLedger {
    /// Claim one root consumer before execution or child registration. Counting
    /// reserves the sequence field's maximum encoded width, not delivery order.
    pub fn claim_event_consumer(&self, root: &AgentRef) -> Result<(), AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if s.events.multiplex
            || root.agent_id() != s.root_id
            || root.root_run_id() != s.root_run_id
            || root.root_session_id() != s.root_session_id
            || s.closed
            || s.agents.len() != 1
            || s.events.counts.used != 0
            || s.events.next != 0
            || s.agents.values().any(|a| a.execution.is_some())
        {
            return Err(AdmissionError::InvalidCeiling);
        }
        s.events.multiplex = true;
        Ok(())
    }

    /// Install root-owned trace policy before execution/children. Untraced trees
    /// leave this unset. Repeating the same immutable policy is harmless.
    pub fn configure_trace(&self, settings: &TraceSettings) -> Result<(), AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if let Some(trace) = &s.events.trace {
            return if trace.settings.capture_content == settings.capture_content
                && trace.settings.max_bytes == settings.max_bytes
            {
                Ok(())
            } else {
                Err(AdmissionError::InvalidCeiling)
            };
        }
        if s.closed
            || s.agents.len() != 1
            || s.events.counts.used != 0
            || s.events.next != 0
            || s.agents.values().any(|a| a.execution.is_some())
        {
            return Err(AdmissionError::InvalidCeiling);
        }
        let reserved = terminal_bytes(&s.limits, settings)?;
        if reserved > settings.max_bytes {
            return Err(trace_exhausted());
        }
        s.events.trace = Some(TraceBudget {
            settings: settings.clone(),
            counts: TraceCounts { used: 0, reserved },
        });
        Ok(())
    }
    pub fn trace_counts(&self) -> Option<TraceCounts> {
        self.0
            .lock()
            .unwrap()
            .events
            .trace
            .as_ref()
            .map(|t| t.counts)
    }
    fn record_bytes(&self, event: &RunEvent) -> Result<(Option<usize>, bool), AdmissionError> {
        let (settings, multiplex) = {
            let s = self.0.lock().unwrap();
            (
                s.events.trace.as_ref().map(|t| t.settings.clone()),
                s.events.multiplex,
            )
        };
        if multiplex && event.root_seq.is_some() {
            return Err(AdmissionError::InvalidCeiling);
        }
        // Serialization stays outside the shared lock. Root delivery reserves
        // bounded field overhead; it never spends a native event a second time.
        let bytes = settings
            .map(|settings| {
                let extra = if multiplex {
                    crate::events::ROOT_SEQUENCE_BYTES
                } else {
                    0
                };
                let remaining = settings
                    .max_bytes
                    .checked_sub(extra)
                    .ok_or_else(trace_exhausted)?;
                jsonl_size(event, settings.capture_content, remaining)
                    .map_err(|_| trace_exhausted())?
                    .checked_add(extra)
                    .ok_or_else(trace_exhausted)
            })
            .transpose()?;
        Ok((bytes, multiplex))
    }

    pub fn admit_event_record(
        &self,
        agent_id: &str,
        event: &RunEvent,
    ) -> Result<(), AdmissionError> {
        let (bytes, multiplex) = self.record_bytes(event)?;
        self.admit_event_bytes(agent_id, bytes, Some(multiplex))
    }
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
        let reservation = reserve(self, &mut s, agent_id, 1, prepaid, true)?;
        s.events.agents.get_mut(agent_id).unwrap().terminal_claimed = true;
        Ok(reservation)
    }
    /// Reserve start/finish together before creating an operation's span or effect.
    pub fn reserve_events(
        &self,
        agent_id: &str,
        count: u64,
    ) -> Result<EventReservation, AdmissionError> {
        reserve(
            self,
            &mut self.0.lock().unwrap(),
            agent_id,
            count,
            false,
            false,
        )
    }
    pub fn admit_event(&self, agent_id: &str) -> Result<(), AdmissionError> {
        self.admit_event_bytes(agent_id, None, None)
    }
    fn admit_event_bytes(
        &self,
        agent_id: &str,
        bytes: Option<usize>,
        multiplex: Option<bool>,
    ) -> Result<(), AdmissionError> {
        let mut s = self.0.lock().unwrap();
        if multiplex.is_some_and(|mode| mode != s.events.multiplex) {
            return Err(AdmissionError::InvalidCeiling);
        }
        check(&s, agent_id, 1)?;
        let trace = trace_after(&s, bytes, 0)?;
        if let Some(counts) = trace {
            s.events.trace.as_mut().unwrap().counts = counts;
        }
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
    pub fn consume_record(&mut self, event: &RunEvent) -> Result<(), AdmissionError> {
        self.consume_bytes(
            self.ledger.record_bytes(event)?.0,
            matches!(event.kind, crate::EventKind::RunFinished { .. }),
        )
    }
    pub fn consume(&mut self) -> Result<(), AdmissionError> {
        self.consume_bytes(None, true)
    }
    fn consume_bytes(
        &mut self,
        bytes: Option<usize>,
        terminal: bool,
    ) -> Result<(), AdmissionError> {
        let mut s = self.ledger.0.lock().unwrap();
        let entry = s
            .events
            .entries
            .get(&self.id)
            .ok_or(AdmissionError::UnknownAgent)?;
        if entry.remaining == 0 {
            return Err(AdmissionError::UnknownAgent);
        }
        if entry.terminal_bytes > 0 && !terminal {
            return Err(AdmissionError::InvalidCeiling);
        }
        let trace = trace_after(&s, bytes, entry.terminal_bytes)?;
        let entry = s.events.entries.get_mut(&self.id).unwrap();
        entry.remaining -= 1;
        entry.terminal_bytes = 0;
        let agent_id = entry.agent_id.clone();
        if let Some(counts) = trace {
            s.events.trace.as_mut().unwrap().counts = counts;
        }
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
            if let Some(trace) = &mut s.events.trace {
                trace.counts.reserved -= entry.terminal_bytes;
            }
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
