//! One machine-readable task projection, derived from native terminal truth.
use crate::{EventKind, RunEvent, RunOutcome, Usage};
use serde::{Deserialize, Serialize};
pub const TASK_SCHEMA_VERSION: &str = "c2.3";

/// Arithmetic stays u64; the new wire accounting object uses decimal strings
/// so an ordinary JavaScript JSON parser cannot round counters or currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounting {
    #[serde(with = "decimal")]
    pub model_calls: u64,
    #[serde(with = "decimal")]
    pub tool_calls: u64,
    #[serde(with = "usage_decimal")]
    pub usage: Usage,
    #[serde(with = "optional_decimal")]
    pub cost_microusd: Option<u64>,
    #[serde(with = "optional_decimal")]
    pub charged_tokens: Option<u64>,
    #[serde(with = "optional_decimal")]
    pub charged_cost_microusd: Option<u64>,
}
impl Default for Accounting {
    fn default() -> Self {
        Self {
            model_calls: 0,
            tool_calls: 0,
            usage: zero_usage(),
            cost_microusd: Some(0),
            charged_tokens: None,
            charged_cost_microusd: None,
        }
    }
}
pub(crate) fn zero_usage() -> Usage {
    Usage {
        input_tokens: Some(0),
        output_tokens: Some(0),
        cache_read_input_tokens: Some(0),
        cache_write_input_tokens: Some(0),
    }
}
impl Accounting {
    pub(crate) fn add_usage(&mut self, next: &Usage) {
        for (total, next) in [
            (&mut self.usage.input_tokens, next.input_tokens),
            (&mut self.usage.output_tokens, next.output_tokens),
            (
                &mut self.usage.cache_read_input_tokens,
                next.cache_read_input_tokens,
            ),
            (
                &mut self.usage.cache_write_input_tokens,
                next.cache_write_input_tokens,
            ),
        ] {
            *total = total.and_then(|old| next.and_then(|n| old.checked_add(n)));
        }
    }
}
/// One run-local ledger. Reservations are charged before the delivery boundary.
pub(crate) struct Ledger {
    bounds: crate::provider::AccountingBounds,
    token_cap: Option<u64>,
    cost_cap: Option<u64>,
}
impl Ledger {
    pub(crate) fn new(
        spec: &crate::RunSpec,
        provider: &dyn crate::Provider,
    ) -> Result<Self, &'static str> {
        let bounds = provider.accounting_bounds(&spec.model, spec.limits.max_output_tokens);
        if (spec.limits.max_total_tokens.is_some() && bounds.tokens.is_none())
            || (spec.limits.max_cost_microusd.is_some() && bounds.cost_microusd.is_none())
        {
            return Err("provider cannot attest requested hard accounting ceilings");
        }
        Ok(Self {
            bounds,
            token_cap: spec.limits.max_total_tokens,
            cost_cap: spec.limits.max_cost_microusd,
        })
    }
    pub(crate) fn initialize(&self, a: &mut Accounting) {
        a.charged_tokens = self.token_cap.map(|_| 0);
        a.charged_cost_microusd = self.cost_cap.map(|_| 0);
    }
    pub(crate) fn reserve(&self, a: &mut Accounting) -> Result<(), crate::RunOutcome> {
        let reserve = |total: Option<u64>, bound: Option<u64>, cap: Option<u64>, limit| {
            if let Some(cap) = cap {
                let next = total.and_then(|n| bound.and_then(|b| n.checked_add(b)));
                next.filter(|n| *n <= cap)
                    .map(Some)
                    .ok_or(crate::RunOutcome::LimitExceeded { limit })
            } else {
                Ok(None)
            }
        };
        // Check both before mutating either. Tokens have deterministic precedence.
        let tokens = reserve(
            a.charged_tokens,
            self.bounds.tokens,
            self.token_cap,
            crate::LimitKind::TotalTokens,
        )?;
        let cost = reserve(
            a.charged_cost_microusd,
            self.bounds.cost_microusd,
            self.cost_cap,
            crate::LimitKind::Cost,
        )?;
        a.charged_tokens = tokens;
        a.charged_cost_microusd = cost;
        Ok(())
    }
    pub(crate) fn settle(
        &self,
        a: &mut Accounting,
        usage: &Usage,
        cost: Option<u64>,
        not_sent: bool,
    ) -> Result<(), crate::RunOutcome> {
        let usage = if not_sent {
            zero_usage()
        } else {
            usage.clone()
        };
        let cost = if not_sent { Some(0) } else { cost };
        let tokens = usage
            .input_tokens
            .and_then(|i| usage.output_tokens.and_then(|o| i.checked_add(o)));
        let violated = (self.bounds.tokens.is_some()
            && usage.input_tokens.is_some()
            && usage.output_tokens.is_some()
            && tokens.is_none())
            || [
                (tokens, self.bounds.tokens),
                (cost, self.bounds.cost_microusd),
            ]
            .into_iter()
            .any(|(actual, bound)| actual.zip(bound).is_some_and(|(a, b)| a > b));
        a.add_usage(&usage);
        a.cost_microusd = a
            .cost_microusd
            .and_then(|old| cost.and_then(|n| old.checked_add(n)));
        // On violation or unknown actuals retain reservations; never release
        // an uncertain liability or wrap an aggregate field.
        if !violated {
            for (charged, bound, actual) in [
                (&mut a.charged_tokens, self.bounds.tokens, tokens),
                (
                    &mut a.charged_cost_microusd,
                    self.bounds.cost_microusd,
                    cost,
                ),
            ] {
                if let (Some(total), Some(bound), Some(actual)) = (*charged, bound, actual) {
                    *charged = total.checked_sub(bound).and_then(|n| n.checked_add(actual));
                }
            }
        }
        if violated {
            Err(crate::RunOutcome::Failed {
                code: crate::FailureCode::AccountingBoundViolated,
                delivery: crate::DeliveryCertainty::ResponseReceived,
            })
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorCode {
    InvalidArguments,
    InvalidConfiguration,
    CredentialUnavailable,
    TraceSetupFailed,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskError {
    pub code: TaskErrorCode,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskResult {
    pub schema_version: String,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub trace_id: Option<String>,
    pub outcome: Option<RunOutcome>,
    pub accounting: Option<Box<Accounting>>,
    pub error: Option<TaskError>,
}
impl TaskResult {
    pub fn rejected(code: TaskErrorCode) -> Self {
        Self {
            schema_version: TASK_SCHEMA_VERSION.into(),
            run_id: None,
            session_id: None,
            trace_id: None,
            outcome: None,
            accounting: None,
            error: Some(TaskError { code }),
        }
    }
    pub fn from_terminal(event: &RunEvent) -> Option<Self> {
        let EventKind::RunFinished { outcome } = &event.kind else {
            return None;
        };
        Some(Self {
            schema_version: TASK_SCHEMA_VERSION.into(),
            run_id: Some(event.run_id.clone()),
            session_id: Some(event.session_id.clone()),
            trace_id: Some(event.trace_id.clone()),
            outcome: Some(outcome.clone()),
            accounting: Some(event.accounting.clone()?),
            error: None,
        })
    }
    /// Serialization includes escaping but never accumulates event history.
    /// Size is checked without allocating a second output-sized buffer.
    pub fn write_json(
        &self,
        mut writer: impl std::io::Write,
        output_limit: usize,
    ) -> std::io::Result<()> {
        let limit = output_limit
            .checked_mul(6)
            .and_then(|n| n.checked_add(8192))
            .ok_or(std::io::ErrorKind::FileTooLarge)?;
        if !crate::filesystem::fits(self, limit) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        serde_json::to_writer(&mut writer, self).map_err(std::io::Error::other)?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
}
mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse(&text)
            .ok_or_else(|| serde::de::Error::custom("expected a canonical unsigned decimal string"))
    }
    pub(super) fn parse(text: &str) -> Option<u64> {
        if text.is_empty()
            || (text.len() > 1 && text.starts_with('0'))
            || !text.bytes().all(|b| b.is_ascii_digit())
        {
            None
        } else {
            text.parse().ok()
        }
    }
}
mod optional_decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(n) => serializer.collect_str(n),
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|s| {
                super::decimal::parse(&s).ok_or_else(|| {
                    serde::de::Error::custom("expected a canonical unsigned decimal string")
                })
            })
            .transpose()
    }
}
mod usage_decimal {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WireUsage {
        #[serde(with = "optional_decimal")]
        input_tokens: Option<u64>,
        #[serde(with = "optional_decimal")]
        output_tokens: Option<u64>,
        #[serde(with = "optional_decimal")]
        cache_read_input_tokens: Option<u64>,
        #[serde(with = "optional_decimal")]
        cache_write_input_tokens: Option<u64>,
    }
    pub fn serialize<S: serde::Serializer>(
        usage: &Usage,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        WireUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            cache_write_input_tokens: usage.cache_write_input_tokens,
        }
        .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Usage, D::Error> {
        let w = WireUsage::deserialize(deserializer)?;
        Ok(Usage {
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            cache_read_input_tokens: w.cache_read_input_tokens,
            cache_write_input_tokens: w.cache_write_input_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_round_trip_and_overflow_remain_exact() {
        let mut a = Accounting::default();
        a.add_usage(&Usage {
            input_tokens: Some(u64::MAX),
            ..zero_usage()
        });
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("18446744073709551615"));
        assert_eq!(serde_json::from_str::<Accounting>(&json).unwrap(), a);
        for bad in ["00", "-1", "18446744073709551616", "1.5"] {
            assert!(
                serde_json::from_str::<Accounting>(&json.replace("18446744073709551615", bad))
                    .is_err()
            );
        }
        a.add_usage(&Usage {
            input_tokens: Some(1),
            ..zero_usage()
        });
        assert_eq!(a.usage.input_tokens, None);
    }
    #[test]
    fn envelope_bounds_include_escaping_and_writer_failure_is_not_retried() {
        let mut task = TaskResult::rejected(TaskErrorCode::InvalidConfiguration);
        task.outcome = Some(RunOutcome::Completed {
            output: "\0".repeat(4096),
            finish_reason: crate::FinishReason::Stop,
            usage: Usage::default(),
        });
        let mut bytes = vec![];
        task.write_json(&mut bytes, 4096).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(bytes.len() > 4096 * 6);
        let mut refused = vec![];
        assert!(task.write_json(&mut refused, 0).is_err());
        assert!(refused.is_empty());
        struct Closed(usize);
        impl std::io::Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                panic!("must not flush failed output")
            }
        }
        let mut closed = Closed(0);
        assert!(task.write_json(&mut closed, 4096).is_err());
        assert_eq!(closed.0, 1);
    }
}
