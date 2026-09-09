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
    ledger: crate::task::Ledger,
    spec: &'a RunSpec,
    provider: &'a dyn Provider,
    tools: &'a ToolRegistry,
    cancellation: &'a CancellationToken,
    deadline: Instant,
    root: &'a Context,
    filesystem: Option<crate::filesystem::Workspace>,
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
        validate(spec, provider)?;
        let ledger = crate::task::Ledger::new(spec, provider).map_err(RunError::InvalidSpec)?;
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
            deployment: self.deployment.clone(),
            sink,
            run_id,
            session_id,
            seq: 0,
            max_events: spec.limits.max_events,
            delivery: DeliveryCertainty::NotSent,
            accounting: crate::task::Accounting::default(),
        };
        ledger.initialize(&mut lifecycle.accounting);
        let execution = Execution {
            ledger,
            spec,
            provider,
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
        let mut history = vec![Message::User {
            text: spec.input.clone(),
        }];
        let mut ids = HashSet::new();
        let mut remaining_tools = spec.limits.max_tool_calls;
        let mut remaining_models = spec.limits.max_model_calls;

        loop {
            if let Some(outcome) = execution.stop() {
                return outcome;
            }
            if let Some(remaining) = &mut remaining_models {
                if *remaining == 0 {
                    return limit(LimitKind::ModelCalls);
                }
                *remaining -= 1;
            }
            let context_bytes =
                serde_json::to_vec(&(&spec.instructions, &history, execution.tools.descriptors()))
                    .expect("serializable request")
                    .len();
            if context_bytes > spec.limits.max_context_bytes {
                return limit(LimitKind::ContextBytes);
            }
            let mut progress = ModelProgress::default();
            let input = ModelInput {
                history: &history,
                allow_tool_calls: remaining_tools != Some(0)
                    && remaining_models != Some(0)
                    && !execution.tools.descriptors().is_empty(),
            };
            if let Err(outcome) = self
                .model(execution, lifecycle, &input, &mut progress, &mut ids)
                .await
            {
                return outcome;
            }

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
            if remaining_tools
                .is_some_and(|remaining| progress.tool_calls.len() > remaining as usize)
            {
                return limit(LimitKind::ToolCalls);
            }
            // Do not perform an effectful tool call without budget to consume its result.
            if remaining_models == Some(0) {
                return limit(LimitKind::ModelCalls);
            }
            history.push(Message::Assistant {
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
                if let Some(remaining) = &mut remaining_tools {
                    *remaining -= 1;
                }
                history.push(Message::Tool {
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
        if !lifecycle.can_start_operation() {
            return Err(limit(LimitKind::Events));
        }
        let spec = execution.spec;
        let started = telemetry::now();
        let model = execution.root.with_span(
            self.tracer
                .span_builder(format!("chat {}", spec.model))
                .with_kind(SpanKind::Client)
                .with_start_time(started)
                .with_attributes([
                    KeyValue::new("gen_ai.operation.name", "chat"),
                    KeyValue::new("gen_ai.provider.name", execution.provider.name()),
                    KeyValue::new("gen_ai.request.model", spec.model.clone()),
                    KeyValue::new(
                        "gen_ai.request.max_tokens",
                        i64::from(spec.limits.max_output_tokens),
                    ),
                    KeyValue::new("gen_ai.request.stream", true),
                    KeyValue::new("gen_ai.conversation.id", lifecycle.session_id.clone()),
                    KeyValue::new("pablo.run.id", lifecycle.run_id.clone()),
                ])
                .start_with_context(&self.tracer, execution.root),
        );
        let parent = execution.root.span().span_context().span_id().to_string();
        let mut result = match lifecycle.emit(
            EventKind::ModelStarted {
                provider: execution.provider.name().into(),
                model: spec.model.clone(),
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
            if let Err(outcome) = execution.ledger.settle(
                &mut lifecycle.accounting,
                &progress.usage,
                progress.cost_microusd,
                not_sent,
            ) {
                result = Err(outcome);
            }
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
    history: &'a [Message],
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
    let parent = execution.root.span().span_context().span_id().to_string();
    let request = ModelRequest {
        model: &spec.model,
        input: &spec.input,
        instructions: &spec.instructions,
        messages: input.history,
        tools: execution.tools.descriptors(),
        allow_tool_calls: input.allow_tool_calls,
        max_output_tokens: spec.limits.max_output_tokens,
        deadline: execution.deadline,
        context: model.clone(),
        cancellation: execution.cancellation.clone(),
    };
    if let Some(outcome) = execution.stop() {
        return Err(outcome);
    }
    execution.ledger.reserve(&mut lifecycle.accounting)?;
    lifecycle.accounting.model_calls = lifecycle
        .accounting
        .model_calls
        .checked_add(1)
        .ok_or_else(|| limit(LimitKind::ModelCalls))?;
    progress.dispatched = true;
    lifecycle.delivery = DeliveryCertainty::MayHaveBeenSent;
    let opened = tokio::select! {
        biased;
        _ = execution.cancellation.cancelled() => return Err(RunOutcome::Cancelled),
        _ = sleep_until(execution.deadline) => return Err(RunOutcome::TimedOut),
        result = execution.provider.stream(request) => result,
    };
    let mut stream = opened.map_err(|error| RunOutcome::Failed {
        code: error.code,
        delivery: error.delivery,
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
            _ = sleep_until(execution.deadline) => return Err(RunOutcome::TimedOut),
            result = stream.next() => result,
        };
        if matches!(next, Some(Ok(_))) {
            lifecycle.delivery = DeliveryCertainty::ResponseReceived;
        }
        if next.is_some() {
            frames += 1;
            if frames > spec.limits.max_events {
                return Err(limit(LimitKind::Events));
            }
        }
        match next {
            Some(Err(error)) => {
                return Err(RunOutcome::Failed {
                    code: error.code,
                    delivery: error.delivery,
                });
            }
            Some(Ok(_)) if progress.finish_reason.is_some() => return Err(malformed()),
            Some(Ok(ProviderEvent::Cost { microusd })) => {
                if progress.cost_microusd.replace(microusd).is_some() {
                    return Err(malformed());
                }
            }
            Some(Ok(ProviderEvent::TextDelta(text))) => {
                if text.len()
                    > spec
                        .limits
                        .max_output_bytes
                        .saturating_sub(progress.output.len())
                {
                    return Err(limit(LimitKind::OutputBytes));
                }
                lifecycle.emit(
                    EventKind::TextDelta { text: text.clone() },
                    model,
                    Some(&parent),
                    telemetry::now(),
                    false,
                )?;
                progress.output.push_str(&text);
                progress.chunks += 1;
            }
            Some(Ok(ProviderEvent::ToolCallStart { id, name })) => {
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
                        .is_some_and(|n| n > u64::from(spec.limits.max_output_tokens))
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

fn validate(spec: &RunSpec, provider: &dyn Provider) -> Result<(), RunError> {
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
    validate(spec, provider)?;
    crate::task::Ledger::new(spec, provider).map_err(RunError::InvalidSpec)?;
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
        self.seq < self.max_events - 2
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
        if !closing && self.seq >= self.max_events - 2 {
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
