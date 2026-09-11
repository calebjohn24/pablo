//! Metadata-only projection; remote content remains in explicitly inspected results.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub operation: String,
    pub context_id: Option<String>,
    pub task_id: Option<String>,
    pub state: Option<super::wire::TaskState>,
    pub artifact_ids: Vec<String>,
    pub remote_reported_usage: Option<super::usage::ReportedUsage>,
}
impl Record {
    pub fn update(kind: super::lifecycle::Update, result: &super::lifecycle::ResultData) -> Self {
        Self {
            operation: match kind {
                super::lifecycle::Update::Message => "message",
                super::lifecycle::Update::Task => "task",
                super::lifecycle::Update::Status => "status",
                super::lifecycle::Update::Artifact => "artifact",
            }
            .into(),
            context_id: result.remote.context_id.clone(),
            task_id: result.remote.task_id.clone(),
            state: result.status,
            artifact_ids: result
                .artifacts
                .iter()
                .map(|a| a.artifact_id.clone())
                .collect(),
            remote_reported_usage: result.remote_reported_usage,
        }
    }
}
