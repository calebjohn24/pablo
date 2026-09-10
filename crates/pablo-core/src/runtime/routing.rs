use crate::{DeliveryCertainty, ModelRouteRecord, RunOutcome, deployment::ResolvedRoute};

impl ModelRouteRecord {
    pub(super) fn selected(
        route: &ResolvedRoute,
        index: usize,
        operation: u64,
        attempt: usize,
    ) -> Self {
        let entry = &route.entries()[index];
        Self {
            schema_version: "model-route-v1".into(),
            route: route.name().into(),
            entry: entry.name().into(),
            entry_index: index,
            provider: entry.profile().provider.name().into(),
            model: entry.profile().model.clone(),
            operation: operation.to_string(),
            attempt,
            selection_reason: if attempt > 1 {
                "fallback"
            } else if operation > 1 {
                "sticky"
            } else {
                "initial"
            }
            .into(),
            phase: "selected".into(),
            dispatched: false,
            delivery: DeliveryCertainty::NotSent,
            status: None,
            failure_code: None,
            limit: None,
            retry_class: None,
            accounting: None,
        }
    }
    pub(super) fn outcome(&mut self, outcome: Option<&RunOutcome>) {
        self.status = Some(outcome.map_or("completed", RunOutcome::label).into());
        self.failure_code = match outcome {
            Some(RunOutcome::Failed { code, .. }) => Some(*code),
            _ => None,
        };
        self.limit = match outcome {
            Some(RunOutcome::LimitExceeded { limit }) => Some(*limit),
            _ => None,
        };
    }
}
