//! Noninteractive shell execution. A contained cwd is policy, not a sandbox.

use crate::{PolicyRule, tool::*};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub struct ShellTool {
    schema: Value,
    validator: jsonschema::Validator,
    policy: crate::shell_policy::ShellPolicy,
}

impl ShellTool {
    pub fn new() -> Result<Self, ToolSetupError> {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object", "additionalProperties": false,
            "required": ["command", "cwd"],
            "properties": {
                "command": {"type":"string", "minLength":1, "maxLength":65536},
                "cwd": {"type":"string", "minLength":1, "maxLength":4096},
                "env": {"type":"object", "maxProperties":32,
                    "propertyNames":{"pattern":"^[A-Za-z_][A-Za-z0-9_]{0,127}$"},
                    "additionalProperties":{"type":"string", "maxLength":8192}},
                "timeout_ms": {"type":"integer", "minimum":1},
                "max_output_bytes": {"type":"integer", "minimum":1}
            }
        });
        let validator = compile_schema(&schema)?;
        Ok(Self {
            schema,
            validator,
            policy: Default::default(),
        })
    }
    pub(crate) fn configure(
        &mut self,
        settings: crate::shell_policy::ShellSettings,
        ceilings: Vec<crate::shell_policy::ShellRestriction>,
    ) -> Result<(), ToolSetupError> {
        self.policy = crate::shell_policy::ShellPolicy::new(settings, ceilings)
            .map_err(|_| ToolSetupError)?;
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: String,
    cwd: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    max_output_bytes: Option<usize>,
}

impl Tool for ShellTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "shell.run".into(),
            description: if self.policy.restricted() {
                "Run one literal command in an explicit workspace cwd. Command policy checks the resolved executable and exact arguments; use quoting for literal arguments. Pipelines, substitutions, expansions and redirections are unsupported. Environment additions must use PABLO_TASK_ names.".into()
            } else {
                "Run a bounded noninteractive /bin/sh command in an explicit workspace cwd. Environment additions must use PABLO_TASK_ names.".into()
            },
            input_schema: self.schema.clone(),
        }
    }

    fn execute<'a>(
        &'a self,
        arguments: Value,
        context: ToolContext<'a>,
    ) -> BoxFuture<'a, ToolResult> {
        Box::pin(async move {
            if !self.validator.is_valid(&arguments) {
                return ToolResult::status(ToolStatus::InvalidArguments);
            }
            let Ok(mut args) = serde_json::from_value::<Arguments>(arguments) else {
                return ToolResult::status(ToolStatus::InvalidArguments);
            };
            if args.command.contains('\0')
                || args.cwd.contains('\0')
                || args.env.values().any(|value| value.contains('\0'))
            {
                return ToolResult::status(ToolStatus::InvalidArguments);
            }
            if args
                .env
                .keys()
                .any(|name| !name.starts_with("PABLO_TASK_") || name.len() <= "PABLO_TASK_".len())
            {
                return ToolResult::denied(PolicyRule::Environment);
            }
            if context.cancellation.is_cancelled() {
                return ToolResult::status(ToolStatus::Cancelled);
            }
            let Ok(workspace) = context.workspace.canonicalize() else {
                return ToolResult::denied(PolicyRule::Workspace);
            };
            let Ok(cwd) = workspace.join(&args.cwd).canonicalize() else {
                return ToolResult::denied(PolicyRule::Workspace);
            };
            if !cwd.is_dir() || !cwd.starts_with(&workspace) {
                return ToolResult::denied(PolicyRule::Workspace);
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                let admission = match self.policy.admit(
                    &args.command,
                    cwd.strip_prefix(&workspace).expect("contained cwd"),
                    &mut args.env,
                    context.policy_decisions,
                ) {
                    Ok(admission) => admission,
                    Err(denial) => {
                        let mut result = ToolResult::denied(denial.rule);
                        result.policy_decisions = denial.decisions.into_boxed_slice();
                        return result;
                    }
                };
                let mut result = unix::execute(args, cwd, context, admission.invocation).await;
                result.policy_decisions = admission.decisions.into_boxed_slice();
                result
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                ToolResult::denied(PolicyRule::UnsupportedPlatform)
            }
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix {
    use super::*;
    use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
    use std::os::unix::process::ExitStatusExt;
    use std::{
        io,
        path::PathBuf,
        process::{ExitStatus, Stdio},
        time::Duration,
    };
    use tokio::{
        io::{AsyncRead, AsyncReadExt},
        process::{Child, Command},
        time::{Instant, interval, sleep_until, timeout},
    };

    const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

    struct OwnedGroup {
        child: Child,
        pid: Pid,
        armed: bool,
    }
    impl OwnedGroup {
        fn kill_group(&mut self) -> io::Result<()> {
            // The leader has not been reaped: its PID cannot be recycled into
            // an unrelated process group between exit detection and this kill.
            let result = kill_process_group(self.pid, Signal::KILL);
            self.armed = false;
            match result {
                Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                // Darwin can return EPERM for a zombie-only group. This is
                // provisional: cleanup must still reap and observe the entire
                // group disappear; a live inaccessible member fails that check.
                Err(rustix::io::Errno::PERM) => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }
    impl Drop for OwnedGroup {
        fn drop(&mut self) {
            if self.armed {
                let _ = kill_process_group(self.pid, Signal::KILL);
            }
            // Child's kill_on_drop is an additional fallback; explicit paths
            // below also await reaping. Dropped futures cannot await cleanup.
        }
    }

    #[derive(Default)]
    struct Capture {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_truncated: bool,
        stderr_truncated: bool,
    }
    impl Capture {
        fn append(&mut self, bytes: &[u8], stderr: bool, cap: usize) -> bool {
            let remaining = cap.saturating_sub(self.stdout.len() + self.stderr.len());
            let count = remaining.min(bytes.len());
            let (target, truncated) = if stderr {
                (&mut self.stderr, &mut self.stderr_truncated)
            } else {
                (&mut self.stdout, &mut self.stdout_truncated)
            };
            target.extend_from_slice(&bytes[..count]);
            *truncated |= count < bytes.len();
            count < bytes.len()
        }
        fn finish(self, status: Option<ExitStatus>) -> ShellResult {
            let stdout_lossy = std::str::from_utf8(&self.stdout).is_err();
            let stderr_lossy = std::str::from_utf8(&self.stderr).is_err();
            ShellResult {
                stdout: String::from_utf8_lossy(&self.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&self.stderr).into_owned(),
                exit_code: status.and_then(|status| status.code()),
                signal: status.and_then(|status| status.signal()),
                stdout_truncated: self.stdout_truncated,
                stderr_truncated: self.stderr_truncated,
                stdout_lossy,
                stderr_lossy,
            }
        }
    }

    pub(super) async fn execute(
        args: Arguments,
        cwd: PathBuf,
        context: ToolContext<'_>,
        invocation: Option<crate::shell_policy::Invocation>,
    ) -> ToolResult {
        let tool_duration = Duration::from_millis(
            args.timeout_ms
                .unwrap_or(context.limits.max_tool_duration_ms)
                .min(context.limits.max_tool_duration_ms),
        );
        let deadline = Instant::now()
            .checked_add(tool_duration)
            .unwrap_or(context.deadline)
            .min(context.deadline);
        if context.cancellation.is_cancelled() {
            return ToolResult::status(ToolStatus::Cancelled);
        }
        if Instant::now() >= deadline {
            return ToolResult::status(ToolStatus::TimedOut);
        }
        let cap = args
            .max_output_bytes
            .unwrap_or(context.limits.max_tool_output_bytes)
            .min(context.limits.max_tool_output_bytes);
        let mut command = Command::new("/bin/sh");
        command.arg("-c");
        if let Some(invocation) = invocation {
            command
                .arg("exec \"$@\"")
                .arg("pablo-literal")
                .arg(invocation.program)
                .args(invocation.args);
        } else {
            command.arg(&args.command);
        }
        command
            .current_dir(cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C")
            .envs(args.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let Ok(mut child) = command.spawn() else {
            return ToolResult::status(ToolStatus::SpawnFailed);
        };
        let Some(pid) = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw)
        else {
            let _ = child.kill().await;
            return ToolResult::status(ToolStatus::SpawnFailed);
        };
        let mut stdout = child.stdout.take().expect("stdout configured as piped");
        let mut stderr = child.stderr.take().expect("stderr configured as piped");
        let mut group = OwnedGroup {
            child,
            pid,
            armed: true,
        };
        let mut capture = Capture::default();
        let mut out_open = true;
        let mut err_open = true;
        let mut out_buf = [0u8; 4096];
        let mut err_buf = [0u8; 4096];
        let mut poll = interval(Duration::from_millis(5));
        let mut status = if !(context.on_started)(pid.as_raw_nonzero().get() as u32) {
            ToolStatus::EventSinkFailed
        } else {
            loop {
                tokio::select! {
                    biased;
                    _ = context.cancellation.cancelled() => break ToolStatus::Cancelled,
                    _ = sleep_until(deadline) => break ToolStatus::TimedOut,
                    _ = poll.tick() => {
                        match waitid(WaitId::Pid(pid), WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT) {
                            Ok(Some(_)) => break ToolStatus::Completed,
                            Ok(None) | Err(rustix::io::Errno::INTR) => {},
                            Err(rustix::io::Errno::CHILD) => {
                                // An external reaper violated ownership. Never signal a reused PID.
                                group.armed = false;
                                break ToolStatus::CleanupFailed;
                            }
                            Err(_) => break ToolStatus::IoFailed,
                        }
                    }
                    read = stdout.read(&mut out_buf), if out_open => match read {
                        Ok(0) => out_open = false,
                        Ok(n) => if capture.append(&out_buf[..n], false, cap) { break ToolStatus::OutputLimit; },
                        Err(_) => break ToolStatus::IoFailed,
                    },
                    read = stderr.read(&mut err_buf), if err_open => match read {
                        Ok(0) => err_open = false,
                        Ok(n) => if capture.append(&err_buf[..n], true, cap) { break ToolStatus::OutputLimit; },
                        Err(_) => break ToolStatus::IoFailed,
                    },
                }
            }
        };
        // Always clean the group, including after successful leader exit: a
        // command may have left background children, even with closed pipes.
        let killed = if group.armed {
            group.kill_group().is_ok()
        } else {
            false
        };
        let cleanup = async {
            let reaped = group.child.wait().await?;
            drain(&mut stdout, &mut stderr, &mut capture, cap).await?;
            loop {
                // Signal zero only probes. Never send a real signal after
                // reaping the leader, because the numeric group ID can be reused.
                match rustix::process::test_kill_process_group(pid) {
                    Err(rustix::io::Errno::SRCH) => break,
                    Ok(()) | Err(rustix::io::Errno::PERM) => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Ok::<_, io::Error>(reaped)
        };
        let exit = match timeout(CLEANUP_TIMEOUT, cleanup).await {
            Ok(Ok(exit)) if killed => Some(exit),
            _ => {
                status = ToolStatus::CleanupFailed;
                None
            }
        };
        if status == ToolStatus::Completed && context.cancellation.is_cancelled() {
            status = ToolStatus::Cancelled;
        }
        if status == ToolStatus::Completed && (capture.stdout_truncated || capture.stderr_truncated)
        {
            status = ToolStatus::OutputLimit;
        }
        fit_result(
            ToolResult {
                subagent: None,
                status,
                policy_rule: None,
                shell: Some(capture.finish(exit)),
                filesystem: None,
                mcp: None,
                skill: None,
                policy_decisions: context.policy_decisions.into(),
            },
            context.limits.max_tool_output_bytes,
        )
    }

    async fn drain(
        stdout: &mut (impl AsyncRead + Unpin),
        stderr: &mut (impl AsyncRead + Unpin),
        capture: &mut Capture,
        cap: usize,
    ) -> io::Result<()> {
        let mut out_open = true;
        let mut err_open = true;
        let mut out_buf = [0u8; 4096];
        let mut err_buf = [0u8; 4096];
        while out_open || err_open {
            tokio::select! {
                read = stdout.read(&mut out_buf), if out_open => match read? {
                    0 => out_open = false,
                    n => { capture.append(&out_buf[..n], false, cap); },
                },
                read = stderr.read(&mut err_buf), if err_open => match read? {
                    0 => err_open = false,
                    n => { capture.append(&err_buf[..n], true, cap); },
                },
            }
        }
        Ok(())
    }

    fn fit_result(mut result: ToolResult, max: usize) -> ToolResult {
        // Raw retention is bounded; JSON escaping and invalid UTF-8 replacement
        // can expand it. Shrink on character boundaries until the complete tool
        // result fits, without turning cancellation/timeout into success.
        while serde_json::to_vec(&result)
            .expect("serializable tool result")
            .len()
            > max
        {
            if result.status == ToolStatus::Completed {
                result.status = ToolStatus::OutputLimit;
            }
            let shell = result.shell.as_mut().expect("shell result exists");
            let (text, truncated) = if shell.stdout.len() >= shell.stderr.len() {
                (&mut shell.stdout, &mut shell.stdout_truncated)
            } else {
                (&mut shell.stderr, &mut shell.stderr_truncated)
            };
            if text.is_empty() {
                return ToolResult::status(ToolStatus::OutputLimit);
            }
            let mut new_len = text.len() / 2;
            while !text.is_char_boundary(new_len) {
                new_len -= 1;
            }
            text.truncate(new_len);
            *truncated = true;
        }
        result
    }
}
