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
    RecoverableError,
    WorkLimit,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<Box<crate::filesystem::FilesystemResult>>,
    #[serde(default, skip_serializing_if = "<[_]>::is_empty")]
    pub policy_decisions: Box<[String]>,
}

impl ToolResult {
    pub fn status(status: ToolStatus) -> Self {
        Self {
            status,
            policy_rule: None,
            shell: None,
            filesystem: None,
            policy_decisions: Box::default(),
        }
    }
    pub fn denied(rule: PolicyRule) -> Self {
        Self {
            status: ToolStatus::PolicyDenied,
            policy_rule: Some(rule),
            shell: None,
            filesystem: None,
            policy_decisions: Box::default(),
        }
    }
}

pub struct ToolContext<'a> {
    pub policy_decisions: &'a [String],
    pub workspace: &'a Path,
    pub filesystem: Option<&'a crate::filesystem::Workspace>,
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
    filesystem: Vec<crate::filesystem::FilesystemTool>,
    policy: std::sync::Arc<crate::policy::PolicySet>,
    descriptors: Vec<ToolDescriptor>,
}

impl ToolRegistry {
    /// Opt in to shell execution; an empty/default registry grants no tools.
    pub fn with_shell() -> Result<Self, ToolSetupError> {
        let shell = crate::shell::ShellTool::new()?;
        Ok(Self {
            descriptors: vec![shell.descriptor()],
            shell: Some(shell),
            ..Self::default()
        })
    }
    /// Compile the explicit built-in catalog once; empty options grant no tools.
    pub fn configured(
        shell: bool,
        filesystem: bool,
        policy: crate::policy::Policy,
    ) -> Result<Self, ToolSetupError> {
        Self::configured_with_writes(shell, filesystem, false, policy)
    }
    pub fn configured_with_writes(
        shell: bool,
        filesystem: bool,
        writes: bool,
        policy: crate::policy::Policy,
    ) -> Result<Self, ToolSetupError> {
        Self::configured_with_policy_set(shell, filesystem, writes, policy.into())
    }
    /// Configure the same built-in tools with independently enforced ceilings.
    pub fn configured_with_policy_set(
        shell: bool,
        filesystem: bool,
        writes: bool,
        policy: crate::policy::PolicySet,
    ) -> Result<Self, ToolSetupError> {
        if writes && !filesystem {
            return Err(ToolSetupError);
        }
        policy.validate().map_err(|_| ToolSetupError)?;
        let policy = std::sync::Arc::new(policy);
        let mut registry = Self {
            policy: policy.clone(),
            ..Self::default()
        };
        if shell {
            let tool = crate::shell::ShellTool::new()?;
            registry.descriptors.push(tool.descriptor());
            registry.shell = Some(tool);
        }
        if filesystem {
            use crate::filesystem::{FilesystemTool, Operation};
            let mut operations = vec![Operation::Read, Operation::List, Operation::Search];
            if writes {
                operations.extend([Operation::Write, Operation::Edit]);
            }
            for operation in operations {
                let tool = FilesystemTool::new(operation, policy.clone())?;
                registry.descriptors.push(tool.descriptor());
                registry.filesystem.push(tool);
            }
        }
        Ok(registry)
    }
    /// Configure shell restrictions without changing the independent tool/launcher policy.
    pub fn with_shell_configuration(
        mut self,
        settings: crate::shell_policy::ShellSettings,
        ceilings: Vec<crate::shell_policy::ShellRestriction>,
    ) -> Result<Self, ToolSetupError> {
        crate::shell_policy::validate(&settings, &ceilings, &mut self.policy.rule_ids())
            .map_err(|_| ToolSetupError)?;
        if let Some(shell) = self.shell.as_mut() {
            shell.configure(settings, ceilings)?;
            self.descriptors[0] = shell.descriptor();
        }
        Ok(self)
    }
    pub fn with_filesystem_reads() -> Result<Self, ToolSetupError> {
        Self::configured(false, true, crate::policy::Policy::default())
    }
    pub fn descriptors(&self) -> &[ToolDescriptor] {
        &self.descriptors
    }
    pub(crate) fn has_filesystem(&self) -> bool {
        !self.filesystem.is_empty()
    }
    pub(crate) fn policy(&self) -> &crate::policy::PolicySet {
        &self.policy
    }
    pub(crate) fn get(&self, name: &str) -> Option<&dyn Tool> {
        if name == "shell.run" {
            return self.shell.as_ref().map(|tool| tool as &dyn Tool);
        }
        self.descriptors
            .iter()
            .position(|d| d.name == name)
            .and_then(|index| {
                self.filesystem
                    .get(index - usize::from(self.shell.is_some()))
            })
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
