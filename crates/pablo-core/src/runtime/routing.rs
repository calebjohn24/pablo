use super::*;
use crate::{DeliveryCertainty, ModelRouteRecord, RunOutcome, deployment::ResolvedRoute};

impl<T: Tracer> Runtime<T>
where
    T::Span: Send + Sync + 'static,
{
    pub(super) async fn select_task_model(
        &self,
        execution: &Execution<'_>,
        lifecycle: &mut Lifecycle<'_>,
        state: &mut TaskState,
    ) -> Result<(), RunOutcome> {
        let Some(router) = execution.router else {
            return Ok(());
        };
        if let Some(outcome) = execution.stop() {
            return Err(outcome);
        }
        if let Some(remaining) = &mut state.remaining_models {
            if *remaining == 0 {
                return Err(limit(LimitKind::ModelCalls));
            }
            *remaining -= 1;
        }
        let attempt = Attempt {
            provider: router,
            model: crate::gateway::JEV_MODEL,
            max_output_tokens: 1,
            reasoning: crate::ReasoningConfig::default(),
            window_tokens: None,
            ledger: crate::task::Ledger::for_model(
                execution.spec,
                router,
                crate::gateway::JEV_MODEL,
                1,
            )
            .map_err(|_| failed(FailureCode::ProviderRejected))?,
        };
        let bytes = context_bytes(execution, &state.history, &state.continuations);
        if bytes > execution.context_capacity() {
            return Err(limit(LimitKind::ContextBytes));
        }
        let input = ModelInput {
            purpose: ModelPurpose::Routing,
            attempt: &attempt,
            deadline: (Instant::now() + Duration::from_millis(router.config.timeout_ms))
                .min(execution.deadline),
            history: &state.history,
            continuations: &[],
            remaining_context: execution.context_capacity() - bytes,
            allow_tool_calls: false,
            max_output_bytes: 256,
            parent: execution.root,
        };
        let mut progress = ModelProgress::default();
        self.model(execution, lifecycle, &input, &mut progress, &mut state.ids)
            .await?;
        let category = router
            .config
            .categories
            .get(&progress.output)
            .ok_or_else(malformed)?;
        state.selected = execution
            .route
            .unwrap()
            .entries()
            .iter()
            .position(|entry| entry.name() == category.model)
            .ok_or_else(malformed)?;
        execution
            .root
            .span()
            .set_attribute(KeyValue::new("pablo.router.category", progress.output));
        execution.root.span().set_attribute(KeyValue::new(
            "gen_ai.request.model",
            execution.attempts[state.selected].model.to_owned(),
        ));
        // The classification never enters generation history or output. Only this
        // task's local index changes; immutable provider resources remain reusable.
        execution.retain_context(bytes)?;
        Ok(())
    }
}

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
            } else if route.router().is_some() {
                "complexity"
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
