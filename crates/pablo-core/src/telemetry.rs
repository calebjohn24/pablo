//! Native OTel instrumentation, with no SDK installation or global mutation.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::{
    Context, InstrumentationScope, KeyValue,
    trace::{Status, TraceContextExt, TracerProvider},
};

use crate::{RunOutcome, Usage};

pub const SEMCONV_REVISION: &str = "fee465db333bdd6a7d2faa320edab5cf3101a4f4";
pub const SCHEMA_URL: &str = "https://opentelemetry.io/schemas/gen-ai-dev/1.42.0-dev";
pub const MAPPING_VERSION: &str = "c1.2";

pub fn tracer<P: TracerProvider>(provider: &P) -> P::Tracer {
    provider.tracer_with_scope(
        InstrumentationScope::builder("pablo")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_schema_url(SCHEMA_URL)
            .build(),
    )
}

/// Truncate once; spans and native records use this exact same instant.
pub(crate) fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_micros(micros(SystemTime::now()))
}

pub(crate) fn micros(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros(),
    )
    .unwrap_or(u64::MAX)
}

pub(crate) fn outcome(context: &Context, outcome: &RunOutcome) {
    let span = context.span();
    span.set_attribute(KeyValue::new("pablo.run.outcome", outcome.label()));
    if let RunOutcome::PolicyDenied {
        rule: crate::PolicyRule::Configured { id },
    } = outcome
    {
        span.set_attribute(KeyValue::new("pablo.policy.rule_id", id.to_string()));
    }
    if !outcome.is_completed() && !matches!(outcome, RunOutcome::Cancelled) {
        let error = match outcome {
            RunOutcome::TimedOut => "timeout",
            RunOutcome::PolicyDenied { .. } => "policy_denied",
            RunOutcome::LimitExceeded { .. } => "limit_exceeded",
            RunOutcome::Failed { code, delivery } => {
                span.set_attribute(KeyValue::new(
                    "pablo.delivery.certainty",
                    match delivery {
                        crate::DeliveryCertainty::NotSent => "not_sent",
                        crate::DeliveryCertainty::MayHaveBeenSent => "may_have_been_sent",
                        crate::DeliveryCertainty::ResponseReceived => "response_received",
                    },
                ));
                match code {
                    crate::FailureCode::ContextOverflow => "context_overflow",
                    crate::FailureCode::CompactionFailed => "compaction_failed",
                    crate::FailureCode::OutputValidationFailed => "output_validation_failed",
                    crate::FailureCode::ModelAttemptTimedOut => "model_attempt_timed_out",
                    crate::FailureCode::ContinuationIncompatible => "continuation_incompatible",
                    crate::FailureCode::UnsupportedProviderContent => {
                        "unsupported_provider_content"
                    }
                    crate::FailureCode::ProviderRejected => "provider_rejected",
                    crate::FailureCode::ProviderTransport => "provider_transport",
                    crate::FailureCode::MalformedStream => "malformed_stream",
                    crate::FailureCode::EventSinkIo => "event_sink_io",
                    crate::FailureCode::InvalidToolArguments => "invalid_tool_arguments",
                    crate::FailureCode::ToolExecution => "tool_execution",
                    crate::FailureCode::ToolCleanup => "tool_cleanup",
                    crate::FailureCode::AccountingBoundViolated => "accounting_bound_violated",
                }
            }
            RunOutcome::Completed { .. } | RunOutcome::Cancelled => unreachable!(),
        };
        span.set_status(Status::error(error));
        span.set_attribute(KeyValue::new("error.type", error));
    }
}

pub(crate) fn usage(context: &Context, usage: &Usage) {
    for (key, value) in [
        ("gen_ai.usage.input_tokens", usage.input_tokens),
        ("gen_ai.usage.output_tokens", usage.output_tokens),
        (
            "gen_ai.usage.cache_read.input_tokens",
            usage.cache_read_input_tokens,
        ),
        (
            "gen_ai.usage.cache_write.input_tokens",
            usage.cache_write_input_tokens,
        ),
    ] {
        // OTel integers are signed. Do not turn a malformed huge counter negative.
        if let Some(value) = value.and_then(|v| i64::try_from(v).ok()) {
            context.span().set_attribute(KeyValue::new(key, value));
        }
    }
}
