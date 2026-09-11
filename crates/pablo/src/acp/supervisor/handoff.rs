//! Resolve owned results without copying any source conversation or granting authority.
use super::*;
use pablo_core::children::{
    SpawnRequest,
    handoff::{ArtifactReference, HandoffKind},
};

impl Supervisor {
    pub(super) fn resolve_handoffs(
        &self,
        request: &SpawnRequest,
    ) -> Result<(SpawnRequest, Vec<ArtifactReference>), &'static str> {
        request
            .validate_shape()
            .map_err(|_| "invalid child selection")?;
        let mut resolved = request.clone();
        resolved.handoffs.clear();
        let mut artifacts = Vec::new();
        for selection in &request.handoffs {
            let source = self.inspect(&selection.source_agent_id)?;
            if source.agent.state() != AgentState::Settled
                || source.result_id.as_deref() != Some(selection.result_id.as_str())
                || source
                    .validation
                    .as_ref()
                    .is_none_or(|v| v.status != "valid")
            {
                return Err("handoff requires a matching schema-valid settled result");
            }
            let Some(RunOutcome::Completed { output, .. }) = &source.outcome else {
                return Err("handoff source did not complete successfully");
            };
            let value: Value = serde_json::from_str(output)
                .map_err(|_| "handoff source is not structured output")?;
            if selection.kind == HandoffKind::Artifact {
                let reference: ArtifactReference = serde_json::from_value(value.clone())
                    .map_err(|_| "invalid artifact reference")?;
                reference
                    .validate()
                    .map_err(|_| "invalid artifact reference")?;
                artifacts.push(reference);
            }
            // This is ordinary selected user context. The immutable source snapshot
            // remains owned by this root, including its usage and native trace links.
            resolved.context.push(
                serde_json::json!({"handoff":{
                    "selection":selection,
                    "value":value,
                    "validation":source.validation,
                    "repair":source.repair,
                    "accounting":source.accounting,
                    "trace":source.trace,
                }})
                .to_string(),
            );
            resolved
                .validate_shape()
                .map_err(|_| "handoff input exceeds bound")?;
        }
        Ok((resolved, artifacts))
    }
}
