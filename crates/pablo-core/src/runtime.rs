use crate::{
    contracts::*,
    events::{EventSink, SinkError},
    provider::{ModelRequest, Provider, ProviderEvent},
    telemetry,
    tool::{ToolContext, ToolRegistry, ToolResult, ToolStatus},
};
use futures_util::StreamExt;
use opentelemetry::{
    Context, KeyValue,
    trace::{SpanKind, Status, TraceContextExt, Tracer},
};
use std::{
    collections::HashSet,
    fmt,
    time::{Duration, SystemTime},
};
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod compaction;
mod routing;
use compaction::{TaskState, context_bytes, estimate_bytes};

#[derive(Debug)]
pub enum RunError {
    InvalidSpec(&'static str),
    InvalidTracer,
    EventDelivery {
        outcome: Box<RunOutcome>,
        source: SinkError,
    },
}
impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(message) => write!(f, "invalid run specification: {message}"),
            Self::InvalidTracer => {
                f.write_str("a tracer producing valid span identities is required")
            }
            Self::EventDelivery { .. } => {
                f.write_str("run settled but terminal event delivery failed")
            }
        }
    }
}
impl std::error::Error for RunError {}

/// The caller drives one future; tools complete cleanup before it settles.
pub struct Runtime<T> {
    deployment: Option<crate::deployment::DeploymentIdentity>,
    tracer: T,
    parent: Context,
}

struct Execution<'a> {
    attempts: Vec<Attempt<'a>>,
    route_policy: Option<&'a crate::deployment::RoutePolicy>,
    route: Option<&'a crate::deployment::ResolvedRoute>,
    spec: &'a RunSpec,
    tools: &'a ToolRegistry,
    cancellation: &'a CancellationToken,
    deadline: Instant,
    root: &'a Context,
    filesystem: Option<crate::filesystem::Workspace>,
}
struct Attempt<'a> {
    provider: &'a dyn Provider,
    model: &'a str,
    max_output_tokens: u32,
    ledger: crate::task::Ledger,
    window_tokens: Option<u64>,
}
fn attempts<'a>(
    spec: &'a RunSpec,
    provider: &'a dyn Provider,
) -> Result<Vec<Attempt<'a>>, RunError> {
    validate(spec, provider)?;
    if let Some(route) = provider.route() {
        if spec.model != route.resolved.entries()[0].profile().model {
            return Err(RunError::InvalidSpec(
                "run model must match the first route entry",
            ));
        }
        route
            .resolved
            .entries()
            .iter()
            .zip(&route.providers)
            .map(|(entry, provider)| {
                let model = &entry.profile().model;
                let max_output_tokens =
                    spec.limits.max_output_tokens.min(entry.max_output_tokens());
                provider
                    .validate_model(model, max_output_tokens)
                    .map_err(RunError::InvalidSpec)?;
                Ok(Attempt {
                    window_tokens: entry.profile().context_window_tokens,
                    provider: provider.as_ref(),
                    model,
                    max_output_tokens,
                    ledger: crate::task::Ledger::for_model(
                        spec,
                        provider.as_ref(),
                        model,
                        max_output_tokens,
                    )
                    .map_err(RunError::InvalidSpec)?,
                })
            })
            .collect()
    } else {
        Ok(vec![Attempt {
            window_tokens: spec.context.window_tokens,
            provider,
            model: &spec.model,
            max_output_tokens: spec.limits.max_output_tokens,
            ledger: crate::task::Ledger::new(spec, provider).map_err(RunError::InvalidSpec)?,
        }])
    }
}
impl Execution<'_> {
    fn stop(&self) -> Option<RunOutcome> {
        if self.cancellation.is_cancelled() {
            Some(RunOutcome::Cancelled)
        } else if Instant::now() >= self.deadline {
            Some(RunOutcome::TimedOut)
        } else {
            None
        }
    }
}

impl<T: Tracer> Runtime<T>
where
    T::Span: Send + Sync + 'static,
{
    pub fn new(tracer: T) -> Self {
        Self {
            deployment: None,
            tracer,
            parent: Context::new(),
        }
    }

    /// Explicit host context; never reads or installs a thread-local context.
    /// Protocol hosts must extract remote context before crossing this boundary.
    pub fn with_parent_context(mut self, parent: Context) -> Self {
        self.parent = parent;
        self
    }

    pub fn with_deployment(mut self, deployment: &crate::deployment::ResolvedDeployment) -> Self {
        self.deployment = Some(deployment.identity());
        self
    }

    /// Convenience entry point granting no tools.
    pub async fn run(
        &self,
        spec: &RunSpec,
        provider: &dyn Provider,
        sink: &mut dyn EventSink,
    ) -> Result<RunOutcome, RunError> {
        self.run_with_tools(
            spec,
            provider,
            &ToolRegistry::default(),
            &CancellationToken::new(),
            sink,
        )
        .await
    }

    pub async fn run_with_tools(
        &self,
        spec: &RunSpec,
        provider: &dyn Provider,
        tools: &ToolRegistry,
        cancellation: &CancellationToken,
        sink: &mut dyn EventSink,
    ) -> Result<RunOutcome, RunError> {
        let attempts = attempts(spec, provider)?;
        #[cfg(unix)]
        let filesystem = if tools.has_filesystem() {
            Some(
                crate::filesystem::Workspace::new(&spec.workspace, tools.policy())
                    .map_err(RunError::InvalidSpec)?,
            )
        } else {
            None
        };
        #[cfg(not(unix))]
        let filesystem = None;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(spec.limits.max_run_duration_ms))
            .ok_or(RunError::InvalidSpec(
                "run duration overflows the monotonic clock",
            ))?;
        let run_id = Uuid::new_v4().to_string();
        let session_id = spec
            .session_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let started = telemetry::now();
        let root = Context::new().with_span(
            self.tracer
                .span_builder("invoke_agent pablo")
                .with_kind(SpanKind::Internal)
                .with_start_time(started)
                .with_attributes([
                    KeyValue::new("gen_ai.operation.name", "invoke_agent"),
                    KeyValue::new("gen_ai.agent.name", "pablo"),
                    KeyValue::new("gen_ai.request.model", spec.model.clone()),
                    KeyValue::new("gen_ai.conversation.id", session_id.clone()),
                    KeyValue::new("pablo.run.id", run_id.clone()),
                    KeyValue::new("pablo.otel.mapping.version", telemetry::MAPPING_VERSION),
                    KeyValue::new("pablo.trace.format.version", SCHEMA_VERSION),
                ])
                .start_with_context(&self.tracer, &self.parent),
        );
        if !root.span().span_context().is_valid() {
            root.span().end();
            return Err(RunError::InvalidTracer);
        }
        if let Some(identity) = &self.deployment {
            let identity = serde_json::to_value(identity).expect("bounded deployment identity");
            root.span().set_attribute(KeyValue::new(
                "pablo.config.fingerprint",
                identity["fingerprint"].as_str().unwrap().to_owned(),
            ));
            root.span()
                .set_attribute(KeyValue::new("pablo.config.schema_version", 1_i64));
            root.span().set_attribute(KeyValue::new(
                "pablo.config.contract_revision",
                crate::deployment::CONTRACT_REVISION,
            ));
        }
        let mut lifecycle = Lifecycle {
            model_profile: None,
            model_route: None,
            compaction: None,
            extra_closing: 0,
            deployment: self.deployment.clone(),
            sink,
            run_id,
            session_id,
            seq: 0,
            max_events: spec.limits.max_events,
            delivery: DeliveryCertainty::NotSent,
            accounting: crate::task::Accounting::default(),
        };
        attempts[0].ledger.initialize(&mut lifecycle.accounting);
        let execution = Execution {
            attempts,
            route_policy: provider.route().map(|r| r.resolved.policy()),
            route: provider.route().map(|r| &r.resolved),
            spec,
            tools,
            cancellation,
            deadline,
            root: &root,
            filesystem,
        };
        let parent_id = self
            .parent
            .span()
            .span_context()
            .is_valid()
            .then(|| self.parent.span().span_context().span_id().to_string());
        let outcome = match lifecycle.emit(
            EventKind::RunStarted,
            &root,
            parent_id.as_deref(),
            started,
            false,
        ) {
            Err(outcome) => outcome,
            Ok(()) if execution.stop().is_some() => execution.stop().expect("stop is monotonic"),
            Ok(())
                if spec.input.len().saturating_add(spec.instructions.len())
                    > spec.limits.max_input_bytes =>
            {
                limit(LimitKind::InputBytes)
            }
            Ok(()) => self.drive(&execution, &mut lifecycle).await,
        };
        if let Some(record) = &mut lifecycle.model_route
            && record.phase == "selected"
        {
            record.phase = "blocked".into();
            record.outcome(Some(&outcome));
        }
        let finished = telemetry::now();
        telemetry::outcome(&root, &outcome);
        let terminal = lifecycle.event(
            EventKind::RunFinished {
                outcome: outcome.clone(),
            },
            &root,
            parent_id.as_deref(),
            finished,
        );
        let delivered = lifecycle.sink.emit(&terminal);
        if delivered.is_err() {
            root.span()
                .set_attribute(KeyValue::new("pablo.event.delivery_failed", true));
        }
        root.span().end_with_timestamp(finished);
        delivered.map_err(|source| RunError::EventDelivery {
            outcome: Box::new(outcome.clone()),
            source,
        })?;
        Ok(outcome)
    }

    async fn drive(&self, execution: &Execution<'_>, lifecycle: &mut Lifecycle<'_>) -> RunOutcome {
        let spec = execution.spec;
        let mut state = TaskState::new(execution);

        'generation: loop {
            if let Some(outcome) = execution.stop() {
                return outcome;
            }
            let context_bytes = context_bytes(execution, &state.history, &state.continuations);
            let estimated_bytes = estimate_bytes(execution, context_bytes, state.history.len());
            let tokens = state.calibration[state.selected].estimate(estimated_bytes);
            let attempt = &execution.attempts[state.selected];
            let near = context_bytes as u64
                >= spec.context.usable(spec.limits.max_context_bytes as u64)
                || attempt.window_tokens.is_some_and(|cap| {
                    tokens.saturating_add(u64::from(attempt.max_output_tokens))
                        >= spec.context.usable(cap)
                });
            if spec.context.enabled
                && !state.compacted
                && near
                && state.split(spec.context.keep_recent_turns).is_some()
            {
                if let Err(outcome) = self
                    .compact(execution, lifecycle, &mut state, "threshold")
                    .await
                {
                    return outcome;
                }
                continue;
            }
            if context_bytes > spec.limits.max_context_bytes {
                return limit(LimitKind::ContextBytes);
            }
            state.operation += 1; // Each state.operation consumes bounded native event slots.
            let mut attempts_used = 0;
            let progress = loop {
                lifecycle.model_route = execution.route.map(|route| {
                    Box::new(ModelRouteRecord::selected(
                        route,
                        state.selected,
                        state.operation,
                        attempts_used + 1,
                    ))
                });
                if let Some(outcome) = execution.stop() {
                    return outcome;
                }
                if let Some(remaining) = &mut state.remaining_models {
                    if *remaining == 0 {
                        return limit(LimitKind::ModelCalls);
                    }
                    *remaining -= 1;
                }
                let attempt = &execution.attempts[state.selected];
                if !attempt.provider.accepts_history(
                    attempt.model,
                    &state.history,
                    &state.continuations,
                ) {
                    return RunOutcome::Failed {
                        code: FailureCode::ContinuationIncompatible,
                        delivery: DeliveryCertainty::NotSent,
                    };
                }
                if attempt.window_tokens.is_some_and(|cap| {
                    state.calibration[state.selected]
                        .estimate(estimated_bytes)
                        .saturating_add(u64::from(attempt.max_output_tokens))
                        > cap
                }) {
                    return limit(LimitKind::ContextBytes);
                }
                let deadline = execution.route_policy.map_or(execution.deadline, |p| {
                    Instant::now()
                        .checked_add(Duration::from_millis(p.per_attempt_timeout_ms))
                        .unwrap_or(execution.deadline)
                        .min(execution.deadline)
                });
                let input = ModelInput {
                    summary: false,
                    max_output_bytes: spec.limits.max_output_bytes,
                    parent: execution.root,
                    attempt,
                    deadline,
                    history: &state.history,
                    continuations: &state.continuations,
                    remaining_context: spec.limits.max_context_bytes - context_bytes,
                    allow_tool_calls: state.remaining_tools != Some(0)
                        && state.remaining_models != Some(0)
                        && !execution.tools.descriptors().is_empty(),
                };
                let mut progress = ModelProgress::default();
                attempts_used += 1;
                let result = self
                    .model(execution, lifecycle, &input, &mut progress, &mut state.ids)
                    .await;
                state.calibration[state.selected]
                    .observe(estimated_bytes, progress.usage.input_tokens);
                match result {
                    Ok(()) => break progress,
                    Err(outcome) => {
                        if matches!(
                            outcome,
                            RunOutcome::Failed {
                                code: FailureCode::ContextOverflow,
                                ..
                            }
                        ) && spec.context.enabled
                            && !state.compacted
                            && !progress.closing_failed
                            && progress.chunks == 0
                            && progress.pending.is_empty()
                            && progress.tool_calls.is_empty()
                        {
                            if let Err(outcome) = self
                                .compact(execution, lifecycle, &mut state, "overflow")
                                .await
                            {
                                return outcome;
                            }
                            state.recovering = true;
                            continue 'generation;
                        }
                        if state.recovering {
                            return outcome;
                        }
                        let eligible = execution.route_policy.is_some_and(|policy| {
                            progress.retry_class.is_some_and(|class| {
                                policy.eligible_errors.iter().any(|e| e == class)
                                    && (class != "transport_uncertain"
                                        || policy.retry_uncertain_delivery)
                            }) && attempts_used < policy.max_attempts
                        });
                        if !eligible
                            || progress.chunks != 0
                            || !progress.pending.is_empty()
                            || !progress.tool_calls.is_empty()
                            || state.selected + 1 >= execution.attempts.len()
                        {
                            return outcome;
                        }
                        state.selected += 1;
                    }
                }
            };

            state.recovering = false;
            if let Some(outcome) = execution.stop() {
                return outcome;
            }
            if progress.tool_calls.is_empty() {
                return RunOutcome::Completed {
                    output: progress.output,
                    finish_reason: FinishReason::Stop,
                    usage: lifecycle.accounting.usage.clone(),
                };
            }
            if state
                .remaining_tools
                .is_some_and(|remaining| progress.tool_calls.len() > remaining as usize)
            {
                return limit(LimitKind::ToolCalls);
            }
            // Do not perform an effectful tool call without budget to consume its result.
            if state.remaining_models == Some(0) {
                return limit(LimitKind::ModelCalls);
            }
            if let Some(value) = progress.continuation {
                state
                    .continuations
                    .push(crate::provider::ContinuationEntry {
                        message_index: state.history.len(),
                        value,
                    });
            }
            state.history.push(Message::Assistant {
                text: progress.output,
                tool_calls: progress.tool_calls.clone(),
            });
            for call in progress.tool_calls {
                if let Some(outcome) = execution.stop() {
                    return outcome;
                }
                let result = match self.tool(execution, lifecycle, &call).await {
                    Ok(result) => result,
                    Err(outcome) => return outcome,
                };
                if let Some(remaining) = &mut state.remaining_tools {
                    *remaining -= 1;
                }
                state.history.push(Message::Tool {
                    call_id: call.id,
                    name: call.name,
                    result,
                });
            }
        }
    }

    async fn model(
        &self,
        execution: &Execution<'_>,
        lifecycle: &mut Lifecycle<'_>,
        input: &ModelInput<'_>,
        progress: &mut ModelProgress,
        ids: &mut HashSet<String>,
    ) -> Result<(), RunOutcome> {
        lifecycle.model_profile = input.attempt.provider.profile_identity();
        if !lifecycle.can_start_operation() {
            return Err(limit(LimitKind::Events));
        }
        if let Some(record) = &mut lifecycle.model_route {
            record.phase = "started".into();
        }
        let started = telemetry::now();
        let model = input.parent.with_span(
            self.tracer
                .span_builder(format!("chat {}", input.attempt.model))
                .with_kind(SpanKind::Client)
                .with_start_time(started)
                .with_attributes([
                    KeyValue::new("gen_ai.operation.name", "chat"),
                    KeyValue::new("gen_ai.provider.name", input.attempt.provider.name()),
                    KeyValue::new("gen_ai.request.model", input.attempt.model.to_owned()),
                    KeyValue::new(
                        "gen_ai.request.max_tokens",
                        i64::from(input.attempt.max_output_tokens),
                    ),
                    KeyValue::new("gen_ai.request.stream", true),
                    KeyValue::new("gen_ai.conversation.id", lifecycle.session_id.clone()),
                    KeyValue::new("pablo.run.id", lifecycle.run_id.clone()),
                ])
                .start_with_context(&self.tracer, input.parent),
        );
        let parent = input.parent.span().span_context().span_id().to_string();
        if let Some(profile) = &lifecycle.model_profile {
            model.span().set_attribute(KeyValue::new(
                "pablo.provider.protocol",
                profile.protocol.clone(),
            ));
            model.span().set_attribute(KeyValue::new(
                "pablo.provider.revision",
                profile.revision.clone(),
            ));
            model.span().set_attribute(KeyValue::new(
                "pablo.provider.capability_profile",
                profile.capability_profile.clone(),
            ));
        }
        model.span().set_attribute(KeyValue::new(
            "pablo.model.purpose",
            if input.summary {
                "compaction"
            } else {
                "generation"
            },
        ));
        let mut result = match lifecycle.emit(
            EventKind::ModelStarted {
                provider: input.attempt.provider.name().into(),
                model: input.attempt.model.to_owned(),
            },
            &model,
            Some(&parent),
            started,
            false,
        ) {
            Err(outcome) => Err(outcome),
            Ok(()) => consume(execution, &model, lifecycle, input, progress, ids).await,
        };
        if progress.dispatched {
            let not_sent = matches!(
                result,
                Err(RunOutcome::Failed {
                    delivery: DeliveryCertainty::NotSent,
                    ..
                })
            );
            if let Err(outcome) = input.attempt.ledger.settle(
                &mut lifecycle.accounting,
                &progress.usage,
                progress.cost_microusd,
                not_sent,
            ) {
                progress.retry_class = None;
                result = Err(outcome);
            }
        }
        if let Some(record) = &mut lifecycle.model_route {
            record.phase = "finished".into();
            record.dispatched = progress.dispatched;
            record.delivery = progress.delivery.unwrap_or(DeliveryCertainty::NotSent);
            record.retry_class = progress.retry_class.map(str::to_owned);
            record.accounting = Some(Box::new(lifecycle.accounting.clone()));
            record.outcome(result.as_ref().err());
        }
        let finished = telemetry::now();
        let finish_event = EventKind::ModelFinished {
            status: result
                .as_ref()
                .err()
                .map_or("completed", RunOutcome::label)
                .into(),
            finish_reason: progress.finish_reason,
            usage: progress.usage.clone(),
            output_bytes: progress.output.len(),
        };
        let closing = lifecycle.emit(finish_event, &model, Some(&parent), finished, true);
        if closing.is_err() {
            progress.closing_failed = true;
            progress.retry_class = None;
        }
        let result = result.and(closing); // Preserve the first failure, including delivery certainty.
        if let Err(outcome) = &result {
            telemetry::outcome(&model, outcome);
        }
        telemetry::usage(&model, &progress.usage);
        model.span().set_attribute(KeyValue::new(
            "pablo.model.output.bytes",
            i64::try_from(progress.output.len()).unwrap_or(i64::MAX),
        ));
        model
            .span()
            .set_attribute(KeyValue::new("pablo.model.output.chunks", progress.chunks));
        if let Some(reason) = progress.finish_reason {
            let name = match reason {
                FinishReason::Stop => "stop",
                FinishReason::Length => "length",
                FinishReason::ToolCalls => "tool_calls",
            };
            model.span().set_attribute(KeyValue::new(
                "gen_ai.response.finish_reasons",
                opentelemetry::Value::Array(opentelemetry::Array::String(vec![name.into()])),
            ));
        }
        model.span().end_with_timestamp(finished);
        result
    }

    async fn tool(
        &self,
        execution: &Execution<'_>,
        lifecycle: &mut Lifecycle<'_>,
        call: &ToolCall,
    ) -> Result<ToolResult, RunOutcome> {
        if !lifecycle.can_start_operation() {
            return Err(limit(LimitKind::Events));
        }
        let Some(tool) = execution.tools.get(&call.name) else {
            return Err(RunOutcome::PolicyDenied {
                rule: PolicyRule::ToolUnavailable,
            });
        };
        let policy = execution.tools.policy();
        let mut decisions = policy
            .decide("tools", &call.name, false)
            .map_err(|rule| RunOutcome::PolicyDenied { rule })?;
        if call.name == "shell.run" {
            decisions.extend(
                policy
                    .decide("executables", "/bin/sh", false)
                    .map_err(|rule| RunOutcome::PolicyDenied { rule })?,
            );
        }
        if let Some(outcome) = execution.stop() {
            return Err(outcome);
        }
        let started = telemetry::now();
        let context = execution.root.with_span(
            self.tracer
                .span_builder(format!("execute_tool {}", call.name))
                .with_kind(SpanKind::Internal)
                .with_start_time(started)
                .with_attributes([
                    KeyValue::new("gen_ai.operation.name", "execute_tool"),
                    KeyValue::new("gen_ai.tool.name", call.name.clone()),
                    KeyValue::new("gen_ai.tool.type", "function"),
                    KeyValue::new("gen_ai.tool.call.id", call.id.clone()),
                    KeyValue::new("pablo.run.id", lifecycle.run_id.clone()),
                ])
                .start_with_context(&self.tracer, execution.root),
        );
        let parent = execution.root.span().span_context().span_id().to_string();
        let mut event_failure = None;
        let start = lifecycle.emit(
            EventKind::ToolStarted { call: call.clone() },
            &context,
            Some(&parent),
            started,
            false,
        );
        let mut result = if let Err(outcome) = start {
            event_failure = Some(outcome);
            ToolResult::status(ToolStatus::EventSinkFailed)
        } else {
            lifecycle.accounting.tool_calls = lifecycle
                .accounting
                .tool_calls
                .checked_add(1)
                .ok_or_else(|| limit(LimitKind::ToolCalls))?;
            let mut on_started = |process_id| match lifecycle.emit(
                EventKind::ShellStarted {
                    call_id: call.id.clone(),
                    process_id,
                },
                &context,
                Some(&parent),
                telemetry::now(),
                false,
            ) {
                Ok(()) => true,
                Err(outcome) => {
                    event_failure = Some(outcome);
                    false
                }
            };
            tool.execute(
                call.arguments.clone(),
                ToolContext {
                    workspace: &execution.spec.workspace,
                    filesystem: execution.filesystem.as_ref(),
                    deadline: execution.deadline,
                    limits: &execution.spec.limits,
                    policy_decisions: &decisions,
                    cancellation: execution.cancellation,
                    context: context.clone(),
                    on_started: &mut on_started,
                },
            )
            .await
        };
        if result.policy_decisions.is_empty() && !decisions.is_empty() {
            result.policy_decisions = decisions.into_boxed_slice();
        }
        if !result.policy_decisions.is_empty() {
            context.span().set_attribute(KeyValue::new(
                "pablo.policy.rule_ids",
                opentelemetry::Value::Array(opentelemetry::Array::String(
                    result
                        .policy_decisions
                        .iter()
                        .cloned()
                        .map(Into::into)
                        .collect(),
                )),
            ));
        }
        let finished = telemetry::now();
        let failure = if result.status == ToolStatus::CleanupFailed {
            tool_failure(&result)
        } else {
            event_failure.or_else(|| tool_failure(&result))
        };
        let closing = lifecycle.emit(
            EventKind::ToolFinished {
                call_id: call.id.clone(),
                name: call.name.clone(),
                result: result.clone(),
            },
            &context,
            Some(&parent),
            finished,
            true,
        );
        let failure = failure.or_else(|| closing.err());
        if let Some(outcome) = &failure {
            telemetry::outcome(&context, outcome);
        } else if result.status == ToolStatus::RecoverableError {
            context.span().set_status(Status::error("filesystem_error"));
            context
                .span()
                .set_attribute(KeyValue::new("error.type", "filesystem_error"));
        } else if result
            .shell
            .as_ref()
            .is_some_and(|shell| shell.exit_code != Some(0))
        {
            context
                .span()
                .set_status(Status::error("shell_exit_nonzero"));
            context
                .span()
                .set_attribute(KeyValue::new("error.type", "shell_exit_nonzero"));
        }
        context.span().end_with_timestamp(finished);
        match failure {
            Some(outcome) => Err(outcome),
            None => Ok(result),
        }
    }
}

#[derive(Default)]
struct ModelProgress {
    closing_failed: bool,
    delivery: Option<DeliveryCertainty>,
    retry_class: Option<&'static str>,
    continuation: Option<crate::provider::Continuation>,
    cost_microusd: Option<u64>,
    dispatched: bool,
    output: String,
    usage: Usage,
    finish_reason: Option<FinishReason>,
    chunks: i64,
    pending: Vec<PendingCall>,
    tool_calls: Vec<ToolCall>,
}
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

struct ModelInput<'a> {
    summary: bool,
    max_output_bytes: usize,
    parent: &'a Context,
    attempt: &'a Attempt<'a>,
    deadline: Instant,
    history: &'a [Message],
    continuations: &'a [crate::provider::ContinuationEntry],
    remaining_context: usize,
    allow_tool_calls: bool,
}

async fn consume(
    execution: &Execution<'_>,
    model: &Context,
    lifecycle: &mut Lifecycle<'_>,
    input: &ModelInput<'_>,
    progress: &mut ModelProgress,
    ids: &mut HashSet<String>,
) -> Result<(), RunOutcome> {
    let spec = execution.spec;
    let parent = input.parent.span().span_context().span_id().to_string();
    let request = ModelRequest {
        model: input.attempt.model,
        input: &spec.input,
        instructions: &spec.instructions,
        messages: input.history,
        continuations: input.continuations,
        max_continuation_bytes: input.remaining_context,
        max_context_bytes: spec.limits.max_context_bytes,
        max_tool_input_bytes: spec.limits.max_tool_input_bytes,
        max_output_bytes: input.max_output_bytes,
        tools: execution.tools.descriptors(),
        allow_tool_calls: input.allow_tool_calls,
        max_output_tokens: input.attempt.max_output_tokens,
        deadline: input.deadline,
        context: model.clone(),
        cancellation: execution.cancellation.clone(),
    };
    if let Some(outcome) = execution.stop() {
        return Err(outcome);
    }
    input.attempt.ledger.reserve(&mut lifecycle.accounting)?;
    lifecycle.accounting.model_calls = lifecycle
        .accounting
        .model_calls
        .checked_add(1)
        .ok_or_else(|| limit(LimitKind::ModelCalls))?;
    progress.dispatched = true;
    lifecycle.delivery = DeliveryCertainty::MayHaveBeenSent;
    progress.delivery = Some(DeliveryCertainty::MayHaveBeenSent);
    let opened = tokio::select! {
        biased;
        _ = execution.cancellation.cancelled() => return Err(RunOutcome::Cancelled),
        _ = sleep_until(input.deadline) => return Err(attempt_timeout(execution, progress)),
        result = input.attempt.provider.stream(request) => result,
    };
    let mut stream = opened.map_err(|error| {
        progress.delivery = Some(error.delivery);
        progress.retry_class = error.fallback_class();
        RunOutcome::Failed {
            code: error.code,
            delivery: error.delivery,
        }
    })?;
    let mut frames = 0u64;
    loop {
        tokio::task::consume_budget().await;
        if let Some(outcome) = execution.stop() {
            return Err(outcome);
        }
        let next = tokio::select! {
            biased;
            _ = execution.cancellation.cancelled() => return Err(RunOutcome::Cancelled),
            _ = sleep_until(input.deadline) => return Err(attempt_timeout(execution, progress)),
            result = stream.next() => result,
        };
        if matches!(next, Some(Ok(_))) {
            lifecycle.delivery = DeliveryCertainty::ResponseReceived;
            progress.delivery = Some(DeliveryCertainty::ResponseReceived);
        }
        if next.is_some() {
            frames += 1;
            if frames > spec.limits.max_events {
                return Err(limit(LimitKind::Events));
            }
        }
        match next {
            Some(Err(mut error)) => {
                if progress.delivery == Some(DeliveryCertainty::ResponseReceived) {
                    error.delivery = DeliveryCertainty::ResponseReceived;
                }
                progress.delivery = Some(error.delivery);
                progress.retry_class = error.fallback_class();
                return Err(RunOutcome::Failed {
                    code: error.code,
                    delivery: error.delivery,
                });
            }
            Some(Ok(_)) if progress.finish_reason.is_some() => return Err(malformed()),
            Some(Ok(ProviderEvent::ResolvedModel(value))) => {
                let profile = lifecycle.model_profile.as_mut().ok_or_else(malformed)?;
                if value.is_empty()
                    || value.len() > 256
                    || value.chars().any(char::is_control)
                    || profile.resolved_model.replace(value.clone()).is_some()
                {
                    return Err(malformed());
                }
                model
                    .span()
                    .set_attribute(KeyValue::new("gen_ai.response.model", value));
            }
            Some(Ok(ProviderEvent::Continuation(value))) => {
                if value.bytes() > input.remaining_context {
                    return Err(limit(LimitKind::ContextBytes));
                }
                if progress.continuation.replace(value).is_some() {
                    return Err(malformed());
                }
            }
            Some(Ok(ProviderEvent::Cost { microusd })) => {
                if progress.cost_microusd.replace(microusd).is_some() {
                    return Err(malformed());
                }
            }
            Some(Ok(ProviderEvent::TextDelta(text))) => {
                if text.len() > input.max_output_bytes.saturating_sub(progress.output.len()) {
                    return Err(limit(LimitKind::OutputBytes));
                }
                if !input.summary {
                    lifecycle.emit(
                        EventKind::TextDelta { text: text.clone() },
                        model,
                        Some(&parent),
                        telemetry::now(),
                        false,
                    )?;
                }
                progress.output.push_str(&text);
                progress.chunks += 1;
            }
            Some(Ok(ProviderEvent::ToolCallStart { id, name })) => {
                if input.summary {
                    if let Some(record) = &mut lifecycle.compaction {
                        record.reason = Some("tool_call_during_summary".into());
                    }
                    return Err(failed(FailureCode::CompactionFailed));
                }
                if id.is_empty()
                    || id.len() > 128
                    || id.chars().any(char::is_control)
                    || name.is_empty()
                    || name.len() > 64
                    || name.chars().any(char::is_control)
                    || ids.contains(&id)
                {
                    return Err(malformed());
                }
                if spec
                    .limits
                    .max_tool_calls
                    .is_some_and(|max| ids.len() >= max as usize)
                {
                    return Err(limit(LimitKind::ToolCalls));
                }
                ids.insert(id.clone());
                progress.pending.push(PendingCall {
                    id,
                    name,
                    arguments: String::new(),
                });
            }
            Some(Ok(ProviderEvent::ToolCallArgumentsDelta { id, delta })) => {
                let Some(call) = progress.pending.iter_mut().find(|call| call.id == id) else {
                    return Err(malformed());
                };
                if delta.len()
                    > spec
                        .limits
                        .max_tool_input_bytes
                        .saturating_sub(call.arguments.len())
                {
                    return Err(limit(LimitKind::ToolInputBytes));
                }
                call.arguments.push_str(&delta);
            }
            Some(Ok(ProviderEvent::Finished { reason, usage })) => {
                progress.usage = usage;
                progress.finish_reason = Some(reason);
                if reason == FinishReason::Length
                    || progress
                        .usage
                        .output_tokens
                        .is_some_and(|n| n > u64::from(input.attempt.max_output_tokens))
                {
                    return Err(limit(LimitKind::OutputTokens));
                }
            }
            None => match progress.finish_reason {
                Some(FinishReason::Stop) if progress.pending.is_empty() => return Ok(()),
                Some(FinishReason::ToolCalls) if !progress.pending.is_empty() => {
                    for call in progress.pending.drain(..) {
                        let arguments = serde_json::from_str(&call.arguments)
                            .map_err(|_| failed(FailureCode::InvalidToolArguments))?;
                        progress.tool_calls.push(ToolCall {
                            id: call.id,
                            name: call.name,
                            arguments,
                        });
                    }
                    return Ok(());
                }
                _ => return Err(malformed()),
            },
        }
    }
}

fn limit(limit: LimitKind) -> RunOutcome {
    RunOutcome::LimitExceeded { limit }
}
fn failed(code: FailureCode) -> RunOutcome {
    RunOutcome::Failed {
        code,
        delivery: DeliveryCertainty::ResponseReceived,
    }
}
fn malformed() -> RunOutcome {
    failed(FailureCode::MalformedStream)
}
fn tool_failure(result: &ToolResult) -> Option<RunOutcome> {
    Some(match result.status {
        ToolStatus::Completed | ToolStatus::RecoverableError => return None,
        ToolStatus::WorkLimit => limit(LimitKind::FilesystemWork),
        ToolStatus::Cancelled => RunOutcome::Cancelled,
        ToolStatus::TimedOut => RunOutcome::TimedOut,
        ToolStatus::OutputLimit => limit(LimitKind::ToolOutputBytes),
        ToolStatus::InvalidArguments => failed(FailureCode::InvalidToolArguments),
        ToolStatus::PolicyDenied => RunOutcome::PolicyDenied {
            rule: result
                .policy_rule
                .clone()
                .unwrap_or(PolicyRule::ToolUnavailable),
        },
        ToolStatus::CleanupFailed => failed(FailureCode::ToolCleanup),
        ToolStatus::EventSinkFailed => failed(FailureCode::EventSinkIo),
        ToolStatus::SpawnFailed | ToolStatus::IoFailed => failed(FailureCode::ToolExecution),
    })
}

fn attempt_timeout(execution: &Execution<'_>, progress: &mut ModelProgress) -> RunOutcome {
    if let Some(outcome) = execution.stop() {
        return outcome;
    }
    progress.retry_class = Some("transport_uncertain");
    RunOutcome::Failed {
        code: FailureCode::ModelAttemptTimedOut,
        delivery: progress
            .delivery
            .unwrap_or(DeliveryCertainty::MayHaveBeenSent),
    }
}

fn validate(spec: &RunSpec, provider: &dyn Provider) -> Result<(), RunError> {
    spec.context.validate().map_err(RunError::InvalidSpec)?;
    provider
        .validate_model(&spec.model, spec.limits.max_output_tokens)
        .map_err(RunError::InvalidSpec)?;
    if spec.model.is_empty() || spec.model.len() > 256 || spec.model.chars().any(char::is_control) {
        return Err(RunError::InvalidSpec(
            "model must contain 1–256 bytes without control characters",
        ));
    }
    if spec
        .session_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 128 || id.chars().any(char::is_control))
    {
        return Err(RunError::InvalidSpec(
            "session identity must contain 1–128 bytes without control characters",
        ));
    }
    if provider.name().is_empty()
        || provider.name().len() > 64
        || provider.name().chars().any(char::is_control)
    {
        return Err(RunError::InvalidSpec(
            "provider identity must contain 1–64 bytes without control characters",
        ));
    }
    if !spec.workspace.is_absolute() {
        return Err(RunError::InvalidSpec("workspace must be absolute"));
    }
    if !spec.limits.filesystem.valid() {
        return Err(RunError::InvalidSpec(
            "filesystem work limits must be positive and bounded",
        ));
    }
    if spec.limits.max_events < 4 {
        return Err(RunError::InvalidSpec(
            "at least four event slots are required",
        ));
    }
    if spec.limits.max_output_tokens == 0 {
        return Err(RunError::InvalidSpec("max_output_tokens must be positive"));
    }
    if spec.limits.max_tool_output_bytes < 1024 {
        return Err(RunError::InvalidSpec(
            "at least 1024 bytes are required for a tool result",
        ));
    }
    Ok(())
}

/// Host preflight before creating trace files or starting an exporter. Uses the
/// same runtime/accounting/capability checks and emits no events or requests.
pub fn validate_run(
    spec: &RunSpec,
    provider: &dyn Provider,
    tools: &ToolRegistry,
) -> Result<(), RunError> {
    attempts(spec, provider)?;
    #[cfg(unix)]
    if tools.has_filesystem() {
        crate::filesystem::Workspace::new(&spec.workspace, tools.policy())
            .map_err(RunError::InvalidSpec)?;
    }
    Instant::now()
        .checked_add(Duration::from_millis(spec.limits.max_run_duration_ms))
        .ok_or(RunError::InvalidSpec(
            "run duration overflows the monotonic clock",
        ))?;
    Ok(())
}

struct Lifecycle<'a> {
    compaction: Option<Box<crate::context::CompactionRecord>>,
    extra_closing: u64,
    model_route: Option<Box<ModelRouteRecord>>,
    model_profile: Option<ProviderIdentity>,
    deployment: Option<crate::deployment::DeploymentIdentity>,
    sink: &'a mut dyn EventSink,
    run_id: String,
    session_id: String,
    seq: u64,
    max_events: u64,
    delivery: DeliveryCertainty,
    accounting: crate::task::Accounting,
}

impl Lifecycle<'_> {
    fn can_start_operation(&self) -> bool {
        self.seq < self.max_events.saturating_sub(2 + self.extra_closing)
    }
    fn event(
        &self,
        kind: EventKind,
        context: &Context,
        parent: Option<&str>,
        time: SystemTime,
    ) -> RunEvent {
        let span = context.span();
        let identity = span.span_context();
        RunEvent {
            compaction: (matches!(
                kind,
                EventKind::CompactionStarted
                    | EventKind::CompactionFinished { .. }
                    | EventKind::RunFinished { .. }
            ) || matches!(
                kind,
                EventKind::ModelStarted { .. } | EventKind::ModelFinished { .. }
            ) && self
                .compaction
                .as_ref()
                .is_some_and(|r| r.status == "running"))
            .then(|| self.compaction.clone())
            .flatten(),
            model_route: matches!(
                kind,
                EventKind::ModelStarted { .. }
                    | EventKind::ModelFinished { .. }
                    | EventKind::RunFinished { .. }
            )
            .then(|| self.model_route.clone())
            .flatten(),
            model_profile: matches!(
                kind,
                EventKind::ModelStarted { .. } | EventKind::ModelFinished { .. }
            )
            .then(|| self.model_profile.clone())
            .flatten(),
            deployment: matches!(kind, EventKind::RunStarted | EventKind::RunFinished { .. })
                .then(|| self.deployment.clone())
                .flatten(),
            accounting: matches!(kind, EventKind::RunFinished { .. })
                .then(|| Box::new(self.accounting.clone())),
            schema_version: SCHEMA_VERSION.into(),
            seq: self.seq + 1,
            timestamp_unix_micros: telemetry::micros(time),
            run_id: self.run_id.clone(),
            session_id: self.session_id.clone(),
            trace_id: identity.trace_id().to_string(),
            span_id: identity.span_id().to_string(),
            parent_span_id: parent.map(str::to_owned),
            trace_flags: format!("{:02x}", identity.trace_flags().to_u8()),
            kind,
        }
    }

    fn emit(
        &mut self,
        kind: EventKind,
        context: &Context,
        parent: Option<&str>,
        time: SystemTime,
        closing: bool,
    ) -> Result<(), RunOutcome> {
        // Reserve model.finished and run.finished, even when deltas exhaust the budget.
        if !closing && self.seq >= self.max_events.saturating_sub(2 + self.extra_closing) {
            return Err(RunOutcome::LimitExceeded {
                limit: LimitKind::Events,
            });
        }
        let event = self.event(kind, context, parent, time);
        self.sink.emit(&event).map_err(|error| match error {
            SinkError::Capacity => RunOutcome::LimitExceeded {
                limit: LimitKind::TraceBytes,
            },
            SinkError::Io(_) => RunOutcome::Failed {
                code: FailureCode::EventSinkIo,
                delivery: self.delivery,
            },
        })?;
        self.seq += 1;
        Ok(())
    }
}
