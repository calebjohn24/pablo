//! One selected remote response lifecycle. No URL fetching, file writes or replay.
use super::{
    RemoteIdentity,
    wire::{self, Artifact, Message, Reply, TaskState},
};
use serde::Serialize;
use std::collections::BTreeSet;

pub const MAX_RESULT_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Wire(wire::Error),
    Identity,
    Sequence,
    Incomplete,
    Bound,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Completed,
    Failed,
    Canceled,
    Rejected,
    InputRequired,
    AuthRequired,
}
impl TaskState {
    pub fn disposition(self) -> Option<Disposition> {
        match self {
            Self::Submitted | Self::Working => None,
            Self::Completed => Some(Disposition::Completed),
            Self::Failed => Some(Disposition::Failed),
            Self::Canceled => Some(Disposition::Canceled),
            Self::Rejected => Some(Disposition::Rejected),
            Self::InputRequired => Some(Disposition::InputRequired),
            Self::AuthRequired => Some(Disposition::AuthRequired),
        }
    }
}
/// Remote content is inert selected data, not local accounting or authority.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ResultData {
    pub remote: RemoteIdentity,
    pub disposition: Option<Disposition>,
    pub status: Option<TaskState>,
    pub message: Option<Message>,
    pub artifacts: Vec<Artifact>,
    pub remote_reported_usage: Option<super::usage::ReportedUsage>,
}
impl ResultData {
    /// Logical output bytes, independent of the larger serialized receipt bound.
    pub fn output_bytes(&self) -> Result<usize, wire::Error> {
        self.artifacts
            .iter()
            .flat_map(|artifact| artifact.parts.iter())
            .chain(self.message.iter().flat_map(|message| message.parts.iter()))
            .try_fold(0usize, |sum, part| {
                sum.checked_add(part.input_bytes()?)
                    .ok_or(wire::Error::Bound)
            })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    Message,
    Task,
    Status,
    Artifact,
}
#[derive(Default)]
pub struct Lifecycle {
    result: ResultData,
    observed: RemoteIdentity,
    open_artifacts: BTreeSet<String>,
    budget: wire::StreamBudget,
    failed: bool,
}
impl Lifecycle {
    pub fn observed(&self) -> &RemoteIdentity {
        &self.observed
    }
    pub fn result(&self) -> &ResultData {
        &self.result
    }
    /// Every input is decoded and charged here; public wire structs cannot bypass
    /// validation. A rejected update poisons the lifecycle but retains the last
    /// accepted identity for a best-effort cancellation exchange by the owner.
    pub fn ingest(
        &mut self,
        bytes: &[u8],
        rpc_id: &str,
        mode: wire::Mode,
        trace: bool,
    ) -> Result<Update, Error> {
        if self.failed || self.result.disposition.is_some() {
            self.failed = true;
            return Err(Error::Sequence);
        }
        let outcome = self.apply(bytes, rpc_id, mode, trace);
        if outcome.is_err() {
            self.failed = true;
        }
        outcome
    }
    fn apply(
        &mut self,
        bytes: &[u8],
        rpc_id: &str,
        mode: wire::Mode,
        trace: bool,
    ) -> Result<Update, Error> {
        if mode == wire::Mode::Cancel {
            return Err(Error::Sequence);
        }
        self.budget.charge(bytes.len()).map_err(Error::Wire)?;
        let reply = wire::decode(bytes, rpc_id, mode, trace).map_err(Error::Wire)?;
        let mut next = self.result.clone();
        if let Some(usage) = reply.reported_usage() {
            next.remote_reported_usage = Some(usage);
        }
        let mut open = self.open_artifacts.clone();
        let (context, task) = match &reply {
            Reply::Message(v) => (v.context_id.as_str(), v.task_id.as_deref()),
            Reply::Task(v) => (v.context_id.as_str(), Some(v.id.as_str())),
            Reply::Status(v) => (v.context_id.as_str(), Some(v.task_id.as_str())),
            Reply::Artifact(v) => (v.context_id.as_str(), Some(v.task_id.as_str())),
        };
        if next
            .remote
            .context_id
            .as_deref()
            .is_some_and(|old| old != context)
            || next
                .remote
                .task_id
                .as_deref()
                .zip(task)
                .is_some_and(|(old, new)| old != new)
        {
            return Err(Error::Identity);
        }
        next.remote.context_id.get_or_insert_with(|| context.into());
        if let Some(task) = task {
            next.remote.task_id.get_or_insert_with(|| task.into());
        }
        self.observed = next.remote.clone();
        let update = match reply {
            Reply::Message(message) => {
                // Message is the immediate-result alternative, not a task update.
                if self.result.remote.task_id.is_some() || !next.artifacts.is_empty() {
                    return Err(Error::Sequence);
                }
                next.message = Some(message);
                next.disposition = Some(Disposition::Completed);
                Update::Message
            }
            Reply::Task(task) => {
                next.status = Some(task.status.state);
                next.disposition = task.status.state.disposition();
                next.message = task.status.message;
                // A Task is a full snapshot; its artifacts supersede prior deltas.
                next.artifacts = task.artifacts;
                open.clear();
                Update::Task
            }
            Reply::Status(status) => {
                next.status = Some(status.status.state);
                next.disposition = status.status.state.disposition();
                if status.status.message.is_some() {
                    next.message = status.status.message;
                }
                Update::Status
            }
            Reply::Artifact(update) => {
                let artifact = update.artifact;
                let position = next
                    .artifacts
                    .iter()
                    .position(|old| old.artifact_id == artifact.artifact_id);
                if update.append {
                    let index = position.ok_or(Error::Sequence)?;
                    if !open.contains(&artifact.artifact_id) {
                        return Err(Error::Sequence);
                    }
                    let old = &mut next.artifacts[index];
                    // SDK append semantics extend Parts while retaining the
                    // original artifact name/description.
                    if artifact.parts.len() > wire::MAX_PARTS.saturating_sub(old.parts.len()) {
                        return Err(Error::Bound);
                    }
                    old.parts.extend(artifact.parts);
                } else if let Some(index) = position {
                    next.artifacts[index] = artifact.clone();
                } else {
                    if next.artifacts.len() >= wire::MAX_ARTIFACTS {
                        return Err(Error::Bound);
                    }
                    next.artifacts.push(artifact.clone());
                }
                if update.last_chunk {
                    open.remove(&artifact.artifact_id);
                } else {
                    open.insert(artifact.artifact_id);
                }
                Update::Artifact
            }
        };
        if next.disposition == Some(Disposition::Completed) && !open.is_empty() {
            return Err(Error::Incomplete);
        }
        if serde_json::to_vec(&next).map_or(true, |bytes| bytes.len() > MAX_RESULT_BYTES) {
            return Err(Error::Bound);
        }
        self.result = next;
        self.open_artifacts = open;
        Ok(update)
    }
    /// A disconnected or input-required stream is never silently successful.
    pub fn finish(self) -> Result<ResultData, Error> {
        if self.failed || self.result.disposition.is_none() {
            return Err(Error::Incomplete);
        }
        Ok(self.result)
    }
}
