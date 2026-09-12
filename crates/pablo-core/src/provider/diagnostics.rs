//! One bounded metadata record per model call; no task content or transport secrets.
use crate::{ReasoningConfig, ReasoningEffort};
use opentelemetry::{Context, KeyValue, trace::TraceContextExt};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDiagnostics {
    pub requested_reasoning: ReasoningConfig,
    pub reported_reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_tokens: Option<u64>,
    pub http_version: Option<HttpVersion>,
    pub preparation_us: Option<u64>,
    pub dispatch_us: Option<u64>,
    pub headers_us: Option<u64>,
    pub first_data_us: Option<u64>,
    pub first_text_us: Option<u64>,
    pub first_tool_delta_us: Option<u64>,
    pub terminal_us: Option<u64>,
    pub complete_us: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpVersion {
    #[serde(rename = "HTTP/1.0")]
    Http10,
    #[serde(rename = "HTTP/1.1")]
    Http11,
    #[serde(rename = "HTTP/2")]
    Http2,
    #[serde(rename = "HTTP/3")]
    Http3,
}

#[derive(Clone)]
pub struct CallDiagnostics {
    started: tokio::time::Instant,
    data: Arc<Mutex<ModelDiagnostics>>,
}
#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Preparation,
    Dispatch,
    Headers,
    FirstData,
    FirstText,
    FirstTool,
    Terminal,
    Complete,
}
impl CallDiagnostics {
    pub(crate) fn new(reasoning: ReasoningConfig) -> Self {
        Self {
            started: tokio::time::Instant::now(),
            data: Arc::new(Mutex::new(ModelDiagnostics {
                requested_reasoning: reasoning,
                ..ModelDiagnostics::default()
            })),
        }
    }
    fn update(&self, f: impl FnOnce(&mut ModelDiagnostics)) {
        f(&mut self.data.lock().unwrap_or_else(|e| e.into_inner()));
    }
    pub(crate) fn mark(&self, phase: Phase) {
        self.update(|d| {
            let value = match phase {
                Phase::Preparation => &mut d.preparation_us,
                Phase::Dispatch => &mut d.dispatch_us,
                Phase::Headers => &mut d.headers_us,
                Phase::FirstData => &mut d.first_data_us,
                Phase::FirstText => &mut d.first_text_us,
                Phase::FirstTool => &mut d.first_tool_delta_us,
                Phase::Terminal => &mut d.terminal_us,
                Phase::Complete => &mut d.complete_us,
            };
            if value.is_none() {
                *value = Some(self.started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
            }
        });
    }
    pub(crate) fn http(&self, version: reqwest::Version) {
        self.update(|d| {
            d.http_version = match version {
                reqwest::Version::HTTP_10 => Some(HttpVersion::Http10),
                reqwest::Version::HTTP_11 => Some(HttpVersion::Http11),
                reqwest::Version::HTTP_2 => Some(HttpVersion::Http2),
                reqwest::Version::HTTP_3 => Some(HttpVersion::Http3),
                _ => None,
            }
        });
    }
    pub(crate) fn reasoning_tokens(&self, tokens: Option<u64>) {
        self.update(|d| d.reasoning_tokens = tokens);
    }
    pub(crate) fn reported_effort(&self, effort: &str) {
        if let Ok(effort) = effort.parse::<ReasoningEffort>() {
            self.update(|d| d.reported_reasoning_effort = Some(effort));
        }
    }
    pub(crate) fn snapshot(&self) -> ModelDiagnostics {
        self.data.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub(crate) fn export(&self, context: &Context) {
        let d = self.snapshot();
        for (key, value) in [
            ("preparation", d.preparation_us),
            ("dispatch", d.dispatch_us),
            ("headers", d.headers_us),
            ("first_data", d.first_data_us),
            ("first_text", d.first_text_us),
            ("first_tool_delta", d.first_tool_delta_us),
            ("terminal", d.terminal_us),
            ("complete", d.complete_us),
        ] {
            if let Some(value) = value {
                context.span().set_attribute(KeyValue::new(
                    format!("pablo.model.timing.{key}_us"),
                    value.min(i64::MAX as u64) as i64,
                ));
            }
        }
        match d.requested_reasoning {
            ReasoningConfig::Effort(e) => context.span().set_attribute(KeyValue::new(
                "pablo.model.reasoning.requested_effort",
                e.name(),
            )),
            ReasoningConfig::Budget { budget_tokens } => {
                context.span().set_attribute(KeyValue::new(
                    "pablo.model.reasoning.requested_budget_tokens",
                    i64::from(budget_tokens),
                ))
            }
        }
        if let Some(effort) = d.reported_reasoning_effort {
            context.span().set_attribute(KeyValue::new(
                "pablo.model.reasoning.reported_effort",
                effort.name(),
            ));
        }
        if let Some(tokens) = d.reasoning_tokens {
            context.span().set_attribute(KeyValue::new(
                "pablo.model.reasoning.tokens",
                tokens.min(i64::MAX as u64) as i64,
            ));
        }
        if let Some(version) = d.http_version {
            let version = match version {
                HttpVersion::Http10 => "1.0",
                HttpVersion::Http11 => "1.1",
                HttpVersion::Http2 => "2",
                HttpVersion::Http3 => "3",
            };
            context
                .span()
                .set_attribute(KeyValue::new("network.protocol.version", version));
        }
    }
}
