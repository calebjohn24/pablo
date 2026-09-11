//! One inline consumer order across root and child sources. Producers already
//! admitted each native event; this projection neither recharges nor replays it.
use super::*;
use crate::children::{AgentRef, ledger::RootLedger};
use std::sync::{Arc, Mutex};

pub struct TreeSink<S>(Arc<Mutex<State<S>>>);
struct State<S> {
    root: AgentRef,
    ledger: RootLedger,
    sink: S,
    seq: u64,
    closed: bool,
}
impl<S> Clone for TreeSink<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<S: EventSink> TreeSink<S> {
    pub fn new(sink: S, ledger: &RootLedger, root: AgentRef) -> Result<Self, SinkError> {
        ledger
            .claim_event_consumer(&root)
            .map_err(|_| io::Error::other("invalid root event consumer"))?;
        Ok(Self(Arc::new(Mutex::new(State {
            root,
            ledger: ledger.clone(),
            sink,
            seq: 0,
            closed: false,
        }))))
    }
    /// Moves an already owned child record without copying its content.
    pub fn emit_owned(&mut self, mut event: RunEvent) -> Result<(), SinkError> {
        let mut s = self
            .0
            .lock()
            .map_err(|_| io::Error::other("root event consumer failed"))?;
        if s.closed || event.root_seq.is_some() || !s.ledger.event_source_matches(&event) {
            return Err(io::Error::other("invalid root event source").into());
        }
        let seq = s.seq.checked_add(1).ok_or(SinkError::Capacity)?;
        event.root_seq = Some(seq);
        // Hold only the delivery mutex during the host's inline sink call, never
        // the admission ledger mutex. All clones share this exact delivery order.
        s.sink.emit(&event)?;
        s.seq = seq;
        if event.run_id == s.root.root_run_id()
            && matches!(event.kind, EventKind::RunFinished { .. })
        {
            s.closed = true;
        }
        Ok(())
    }
}
impl<S: EventSink> EventSink for TreeSink<S> {
    fn emit(&mut self, event: &RunEvent) -> Result<(), SinkError> {
        self.emit_owned(event.clone())
    }
}
