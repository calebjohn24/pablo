//! A small explicit capability catalog. C1.2 registers only the built-in shell.

use crate::{PolicyRule, RunLimits};
use futures_util::future::BoxFuture;
use opentelemetry::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Completed,
    Cancelled,
    TimedOut,
    OutputLimit,
    InvalidArguments,
    PolicyDenied,
    SpawnFailed,
    IoFailed,
    CleanupFailed,
    EventSinkFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub stdout_lossy: bool,
    pub stderr_lossy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub status: ToolStatus,
    pub policy_rule: Option<PolicyRule>,
    pub shell: Option<ShellResult>,
}

impl ToolResult {
    pub fn status(status: ToolStatus) -> Self {
        Self {
            status,
            policy_rule: None,
            shell: None,
        }
    }
    pub fn denied(rule: PolicyRule) -> Self {
        Self {
            status: ToolStatus::PolicyDenied,
            policy_rule: Some(rule),
            shell: None,
        }
    }
}

pub struct ToolContext<'a> {
    pub workspace: &'a Path,
    pub deadline: Instant,
    pub limits: &'a RunLimits,
    pub cancellation: &'a CancellationToken,
    pub context: Context,
    /// False stops the tool, including process cleanup, after event delivery fails.
    pub on_started: &'a mut (dyn FnMut(u32) -> bool + Send),
}

/// Tool futures must honor cancellation/deadlines and finish owned cleanup before
/// returning. The runtime does not drop an executing tool to implement cancellation.
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn execute<'a>(
        &'a self,
        arguments: Value,
        context: ToolContext<'a>,
    ) -> BoxFuture<'a, ToolResult>;
}

#[derive(Debug)]
pub struct ToolSetupError;
impl std::fmt::Display for ToolSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("built-in tool schema could not be compiled")
    }
}
impl std::error::Error for ToolSetupError {}

#[derive(Default)]
pub struct ToolRegistry {
    shell: Option<crate::shell::ShellTool>,
    descriptors: Vec<ToolDescriptor>,
}

impl ToolRegistry {
    /// Opt in to shell execution; an empty/default registry grants no tools.
    pub fn with_shell() -> Result<Self, ToolSetupError> {
        let shell = crate::shell::ShellTool::new()?;
        Ok(Self {
            descriptors: vec![shell.descriptor()],
            shell: Some(shell),
        })
    }
    pub fn descriptors(&self) -> &[ToolDescriptor] {
        &self.descriptors
    }
    pub(crate) fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.shell
            .as_ref()
            .filter(|_| name == "shell.run")
            .map(|tool| tool as &dyn Tool)
    }
}

struct NoRetrieval;
impl jsonschema::Retrieve for NoRetrieval {
    fn retrieve(
        &self,
        _: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval is disabled".into())
    }
}

pub(crate) fn compile_schema(schema: &Value) -> Result<jsonschema::Validator, ToolSetupError> {
    jsonschema::draft202012::options()
        .with_retriever(NoRetrieval)
        .build(schema)
        .map_err(|_| ToolSetupError)
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_retrieval_cannot_read_network_or_local_files() {
        for uri in [
            "https://example.invalid/schema",
            "file:///private/forbidden-schema.json",
        ] {
            assert!(super::compile_schema(&serde_json::json!({"$ref": uri})).is_err());
        }
    }
}
