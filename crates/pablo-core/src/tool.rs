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
    AdmissionFailed,
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
    /// Serialized projection of runtime-owned child handles; never admission input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<Box<Value>>,
    pub status: ToolStatus,
    pub policy_rule: Option<PolicyRule>,
    pub shell: Option<ShellResult>,
    #[cfg(unix)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<Box<crate::mcp::tool::McpResult>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<Box<crate::filesystem::FilesystemResult>>,
    #[cfg(unix)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<Box<crate::skills::tool::SkillResult>>,
    #[serde(default, skip_serializing_if = "<[_]>::is_empty")]
    pub policy_decisions: Box<[String]>,
}

impl ToolResult {
    pub fn status(status: ToolStatus) -> Self {
        Self {
            subagent: None,
            status,
            policy_rule: None,
            shell: None,
            #[cfg(unix)]
            mcp: None,
            filesystem: None,
            #[cfg(unix)]
            skill: None,
            policy_decisions: Box::default(),
        }
    }
    pub fn denied(rule: PolicyRule) -> Self {
        Self {
            subagent: None,
            status: ToolStatus::PolicyDenied,
            policy_rule: Some(rule),
            shell: None,
            #[cfg(unix)]
            mcp: None,
            filesystem: None,
            #[cfg(unix)]
            skill: None,
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
    #[cfg(unix)]
    skills: Option<crate::skills::tool::ResourceTool>,
    shell: Option<crate::shell::ShellTool>,
    filesystem: Vec<crate::filesystem::FilesystemTool>,
    policy: std::sync::Arc<crate::policy::PolicySet>,
    descriptors: Vec<ToolDescriptor>,
    admission_deadline: Option<Instant>,
    mcp_omissions: std::collections::BTreeMap<String, &'static str>,
    #[cfg(unix)]
    mcp_claimed: std::sync::atomic::AtomicBool,
    #[cfg(unix)]
    mcp: Vec<crate::mcp::tool::McpTool>,
    #[cfg(unix)]
    sessions: Vec<std::sync::Arc<tokio::sync::Mutex<crate::mcp::stdio::StdioSession>>>,
}

impl ToolRegistry {
    #[cfg(unix)]
    pub fn with_activated_skills(
        mut self,
        active: crate::skills::activation::ActivatedSkills,
    ) -> Result<Self, ToolSetupError> {
        if self.skills.is_some() {
            return Err(ToolSetupError);
        }
        if !active.records().is_empty() {
            let tool = crate::skills::tool::ResourceTool::new(active)?;
            self.descriptors.push(tool.descriptor());
            self.skills = Some(tool);
        }
        Ok(self)
    }
    #[cfg(unix)]
    pub fn activated_skills(&self) -> Option<&crate::skills::activation::ActivatedSkills> {
        self.skills.as_ref().map(|tool| &tool.active)
    }
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
    #[cfg(unix)]
    pub(crate) async fn attach_mcp(
        &mut self,
        server: &str,
        mut session: crate::mcp::stdio::StdioSession,
        admitted: Vec<(String, Vec<String>)>,
    ) -> Result<(), crate::mcp::stdio::Error> {
        let prepared = (|| {
            let mut descriptors = Vec::new();
            let mut identities = crate::mcp::Identities::default();
            let reserved = [
                "shell_run",
                "fs_read",
                "fs_list",
                "fs_search",
                "fs_write",
                "fs_edit",
                "skill_read",
            ]
            .into_iter()
            .map(String::from)
            .collect();
            for tool in &self.mcp {
                identities
                    .insert(&tool.server, &tool.name, &reserved)
                    .map_err(|_| ToolSetupError)?;
            }
            for (name, decisions) in admitted {
                identities
                    .insert(server, &name, &reserved)
                    .map_err(|_| ToolSetupError)?;
                let source = session
                    .tools()
                    .iter()
                    .find(|tool| tool.name == name)
                    .ok_or(ToolSetupError)?;
                let descriptor = ToolDescriptor {
                    name: crate::mcp::qualified(server, &name).map_err(|_| ToolSetupError)?,
                    description: source.description.clone(),
                    input_schema: source.input_schema.clone(),
                };
                descriptors.push((descriptor, name, decisions));
            }
            Ok::<_, ToolSetupError>(descriptors)
        })();
        let descriptors = match prepared {
            Ok(value) => value,
            Err(error) => {
                session.close().await?;
                let _ = error;
                return Err(crate::mcp::stdio::Error::Catalog);
            }
        };
        let session = std::sync::Arc::new(tokio::sync::Mutex::new(session));
        for (descriptor, name, decisions) in descriptors {
            self.descriptors.push(descriptor.clone());
            self.mcp.push(crate::mcp::tool::McpTool {
                descriptor,
                server: server.into(),
                name,
                session: session.clone(),
                decisions: decisions.into_boxed_slice(),
            });
        }
        self.sessions.push(session);
        Ok(())
    }
    pub(crate) fn claim(&self) -> bool {
        #[cfg(unix)]
        if !self.sessions.is_empty() {
            return self
                .mcp_claimed
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok();
        }
        true
    }
    /// Join all per-run MCP resources before returning a terminal outcome.
    pub async fn close(&self) -> Result<(), ToolSetupError> {
        #[cfg(unix)]
        {
            if !self.sessions.is_empty() {
                self.mcp_claimed
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            let mut failed = false;
            for session in &self.sessions {
                failed |= session.lock().await.close().await.is_err();
            }
            if failed {
                return Err(ToolSetupError);
            }
        }
        Ok(())
    }
    pub(crate) fn omit_mcp(&mut self, server: &str, reason: &'static str) {
        self.mcp_omissions.insert(server.into(), reason);
    }
    pub fn mcp_omissions(&self) -> &std::collections::BTreeMap<String, &'static str> {
        &self.mcp_omissions
    }
    pub(crate) fn constrain_deadline(&mut self, deadline: Instant) {
        self.admission_deadline = Some(deadline);
    }
    pub(crate) fn deadline(&self, deadline: Instant) -> Instant {
        self.admission_deadline
            .map_or(deadline, |admitted| admitted.min(deadline))
    }
    pub fn descriptors(&self) -> &[ToolDescriptor] {
        &self.descriptors
    }
    /// Remove executable capabilities as well as their model-visible entries.
    /// Skill instructions and owned MCP sessions remain alive for context/cleanup.
    pub(crate) fn retain_tools(&mut self, names: &std::collections::BTreeSet<String>) {
        if !names.contains("shell.run") {
            self.shell = None;
        }
        self.filesystem
            .retain(|tool| names.contains(&tool.descriptor().name));
        #[cfg(unix)]
        self.mcp
            .retain(|tool| names.contains(&tool.descriptor.name));
        self.descriptors.retain(|tool| names.contains(&tool.name));
    }
    pub(crate) fn has_filesystem(&self) -> bool {
        !self.filesystem.is_empty()
    }
    pub(crate) fn policy(&self) -> &crate::policy::PolicySet {
        &self.policy
    }
    pub(crate) fn get(&self, name: &str) -> Option<&dyn Tool> {
        #[cfg(unix)]
        if name == "skill.read" {
            if !self
                .descriptors
                .iter()
                .any(|descriptor| descriptor.name == name)
            {
                return None;
            }
            return self.skills.as_ref().map(|tool| tool as &dyn Tool);
        }
        if name == "shell.run" {
            return self.shell.as_ref().map(|tool| tool as &dyn Tool);
        }
        #[cfg(unix)]
        if let Some(tool) = self.mcp.iter().find(|tool| tool.descriptor.name == name) {
            return Some(tool as &dyn Tool);
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
