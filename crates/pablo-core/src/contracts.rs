use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Checkpoint-local contract revision; not the full 0.1 protocol contract.
pub const SCHEMA_VERSION: &str = "c3.33";

/// Model-visible configuration never contains provider or exporter credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub input: String,
    pub instructions: String,
    pub workspace: PathBuf,
    pub model: String,
    #[serde(default, skip_serializing_if = "crate::ReasoningConfig::is_default")]
    pub reasoning: crate::ReasoningConfig,
    pub session_id: Option<String>,
    pub limits: RunLimits,
    #[serde(default)]
    pub context: crate::context::ContextSettings,
    #[serde(default)]
    pub output: Option<crate::output::OutputSettings>,
    pub trace: TraceSettings,
}

impl RunSpec {
    pub fn new(input: impl Into<String>, workspace: PathBuf, model: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            instructions: String::new(),
            workspace,
            model: model.into(),
            reasoning: crate::ReasoningConfig::default(),
            session_id: None,
            limits: RunLimits::default(),
            context: crate::context::ContextSettings::default(),
            output: None,
            trace: TraceSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLimits {
    #[serde(default)]
    pub max_total_tokens: Option<u64>,
    #[serde(default)]
    pub max_cost_microusd: Option<u64>,
    #[serde(default)]
    pub filesystem: crate::filesystem::FilesystemLimits,
    /// None means no call-count limit; Some(0) disables model calls.
    pub max_model_calls: Option<u32>,
    /// None means no call-count limit; Some(0) disables tool calls.
    pub max_tool_calls: Option<u32>,
    pub max_tool_duration_ms: u64,
    pub max_tool_input_bytes: usize,
    /// Maximum serialized tool result, including metadata and JSON escaping.
    pub max_tool_output_bytes: usize,
    pub max_context_bytes: usize,
    pub max_run_duration_ms: u64,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    /// Passed to the provider; the core cannot infer token counts from text.
    pub max_output_tokens: u32,
    /// Includes lifecycle records and the terminal event.
    pub max_events: u64,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_total_tokens: None,
            max_cost_microusd: None,
            filesystem: crate::filesystem::FilesystemLimits::default(),
            max_model_calls: None,
            max_tool_calls: None,
            max_tool_duration_ms: 15 * 60 * 1000,
            max_tool_input_bytes: 1024 * 1024,
            max_tool_output_bytes: 8 * 1024 * 1024,
            max_context_bytes: 32 * 1024 * 1024,
            max_run_duration_ms: 60 * 60 * 1000,
            max_input_bytes: 1024 * 1024,
            max_output_bytes: 4 * 1024 * 1024,
            max_output_tokens: 65_536,
            max_events: 1_000_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSettings {
    pub capture_content: bool,
    pub max_bytes: usize,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self {
            capture_content: false,
            max_bytes: 256 * 1024 * 1024,
        }
    }
}

/// None means unknown, including for cache usage; never infer a zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryCertainty {
    NotSent,
    MayHaveBeenSent,
    ResponseReceived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    RemoteTask,
    ChildAdmission,
    ContextOverflow,
    CompactionFailed,
    OutputValidationFailed,
    ModelAttemptTimedOut,
    ContinuationIncompatible,
    UnsupportedProviderContent,
    ProviderRejected,
    ProviderTransport,
    MalformedStream,
    EventSinkIo,
    InvalidToolArguments,
    ToolExecution,
    ToolCleanup,
    AccountingBoundViolated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    InputBytes,
    OutputBytes,
    OutputTokens,
    ModelCalls,
    Events,
    TraceBytes,
    ToolCalls,
    ToolInputBytes,
    ToolOutputBytes,
    ContextBytes,
    FilesystemWork,
    TotalTokens,
    Cost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunOutcome {
    Completed {
        output: String,
        finish_reason: FinishReason,
        usage: Usage,
    },
    TimedOut,
    Cancelled,
    PolicyDenied {
        rule: PolicyRule,
    },
    LimitExceeded {
        limit: LimitKind,
    },
    Failed {
        code: FailureCode,
        delivery: DeliveryCertainty,
    },
}

impl RunOutcome {
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "completed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::PolicyDenied { .. } => "policy_denied",
            Self::LimitExceeded { .. } => "limit_exceeded",
            Self::Failed { .. } => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRule {
    ToolUnavailable,
    Symlink,
    Configured { id: Box<str> },
    Workspace,
    Environment,
    UnsupportedPlatform,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        result: crate::tool::ToolResult,
    },
}

/// Live events contain content. JsonlSink applies its independent capture policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderIdentity {
    pub protocol: String,
    pub revision: String,
    pub capability_profile: String,
    pub endpoint: String,
    pub requested_model: String,
    pub resolved_model: Option<String>,
}

/// Bounded latest route selection; finished events form the streamed attempt ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRouteRecord {
    pub schema_version: String,
    pub route: String,
    pub entry: String,
    pub entry_index: usize,
    pub provider: String,
    pub model: String,
    pub operation: String,
    pub attempt: usize,
    pub selection_reason: String,
    pub phase: String,
    pub dispatched: bool,
    pub delivery: DeliveryCertainty,
    pub status: Option<String>,
    pub failure_code: Option<FailureCode>,
    pub limit: Option<LimitKind>,
    pub retry_class: Option<String>,
    pub accounting: Option<Box<crate::task::Accounting>>,
}

/// Live events contain content. JsonlSink applies its independent capture policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvent {
    /// Root consumer order; seq remains the executing agent's original sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Box<crate::children::AgentIdentity>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_repair: Option<Box<crate::output::OutputRepair>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<Box<crate::context::CompactionRecord>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_validation: Option<Box<crate::output::OutputValidation>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_route: Option<Box<ModelRouteRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<ProviderIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<crate::deployment::DeploymentIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounting: Option<Box<crate::task::Accounting>>,
    pub schema_version: String,
    pub seq: u64,
    /// UTC Unix microseconds, shared exactly with OTel lifecycle timestamps.
    pub timestamp_unix_micros: u64,
    pub run_id: String,
    pub session_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub trace_flags: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventKind {
    #[serde(rename = "a2a.update")]
    A2aUpdate { remote: crate::a2a::events::Record },
    #[cfg(unix)]
    #[serde(rename = "skill.activated")]
    SkillActivated {
        skill: crate::skills::activation::ActivationRecord,
        instructions: Option<String>,
    },
    #[serde(rename = "context.compaction.started")]
    CompactionStarted,
    #[serde(rename = "context.compaction.finished")]
    CompactionFinished {
        summary: Option<String>,
        summary_bytes: usize,
    },
    #[serde(rename = "run.started")]
    RunStarted,
    #[serde(rename = "model.started")]
    ModelStarted { provider: String, model: String },
    #[serde(rename = "assistant.text.delta")]
    TextDelta { text: String },
    #[serde(rename = "model.finished")]
    ModelFinished {
        status: String,
        finish_reason: Option<FinishReason>,
        usage: Usage,
        output_bytes: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diagnostics: Option<Box<crate::provider::diagnostics::ModelDiagnostics>>,
    },
    #[serde(rename = "tool.started")]
    ToolStarted { call: ToolCall },
    #[serde(rename = "shell.started")]
    ShellStarted { call_id: String, process_id: u32 },
    #[serde(rename = "tool.finished")]
    ToolFinished {
        call_id: String,
        name: String,
        result: crate::tool::ToolResult,
    },
    #[serde(rename = "run.finished")]
    RunFinished { outcome: RunOutcome },
}
