//! Root retained context shares child capacity. Child scopes already reserve their
//! complete context ceiling at admission. This counts encoded model context, not RSS.
use super::*;
use crate::children::ledger::{
    RootLedger,
    resources::{ResourceLease, Resources},
};
use std::sync::Mutex;

pub(super) struct ResidentContext {
    root: Option<(RootLedger, String)>,
    max_bytes: usize,
    state: Mutex<(usize, Option<ResourceLease>)>,
}
impl ResidentContext {
    pub fn new(scope: &AccountingScope, is_root: bool, max_bytes: usize) -> Self {
        Self {
            root: is_root.then(|| (scope.ledger.clone(), scope.agent_id.clone())),
            max_bytes: if is_root {
                max_bytes
            } else {
                max_bytes.min(crate::children::MAX_CONTEXT_BYTES)
            },
            state: Mutex::new((0, None)),
        }
    }
    fn update(
        &self,
        state: &mut (usize, Option<ResourceLease>),
        bytes: usize,
    ) -> Result<(), RunOutcome> {
        if bytes > self.max_bytes {
            return Err(limit(LimitKind::ContextBytes));
        }
        if self.root.is_none() {
            state.0 = bytes;
            return Ok(());
        }
        if bytes == state.0 {
            return Ok(());
        }
        if bytes == 0 {
            *state = (0, None);
            return Ok(());
        }
        let (ledger, agent_id) = self.root.as_ref().expect("root context lease");
        let requested = Resources {
            context_bytes: bytes,
            ..Default::default()
        };
        let result = match &mut state.1 {
            Some(lease) if bytes < state.0 => lease.reduce(requested),
            Some(lease) => lease.replace(requested),
            None => ledger
                .reserve_resources(agent_id, requested)
                .map(|lease| state.1 = Some(lease)),
        };
        result.map_err(|error| match error {
            crate::children::ledger::AdmissionError::Capacity => limit(LimitKind::ContextBytes),
            other => shared_admission_failure(other),
        })?;
        state.0 = bytes;
        Ok(())
    }
    pub fn capacity(&self) -> usize {
        self.root
            .as_ref()
            .map_or(self.max_bytes, |(ledger, agent_id)| {
                self.max_bytes.min(
                    ledger
                        .context_capacity(agent_id)
                        .expect("registered context owner"),
                )
            })
    }
    pub fn cover(&self, bytes: usize) -> Result<(), RunOutcome> {
        let mut state = self.state.lock().unwrap();
        let next = bytes.max(state.0);
        self.update(&mut state, next)
    }
    pub fn set(&self, bytes: usize) -> Result<(), RunOutcome> {
        self.update(&mut self.state.lock().unwrap(), bytes)
    }
    pub fn grow(&self, bytes: usize) -> Result<(), RunOutcome> {
        let mut state = self.state.lock().unwrap();
        let next = state
            .0
            .checked_add(bytes)
            .ok_or_else(|| limit(LimitKind::ContextBytes))?;
        self.update(&mut state, next)
    }
}
impl Execution<'_> {
    pub(super) fn context_capacity(&self) -> usize {
        self.resident.as_ref().map_or(
            self.spec.limits.max_context_bytes,
            ResidentContext::capacity,
        )
    }
    pub(super) fn retain_context(&self, bytes: usize) -> Result<(), RunOutcome> {
        self.resident
            .as_ref()
            .map_or(Ok(()), |resident| resident.set(bytes))
    }
    pub(super) fn grow_context(&self, bytes: usize) -> Result<(), RunOutcome> {
        self.resident
            .as_ref()
            .map_or(Ok(()), |resident| resident.grow(bytes))
    }
}
