//! Bounded A2A 1.0 JSONRPC profile. No transport, replay or local authority.
use super::{MAX_REMOTE_ID_BYTES, TRACE_EXTENSION};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};

pub const MAX_INPUT_BYTES: usize = 32 * 1024;
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_STREAM_BYTES: usize = 1024 * 1024;
pub const MAX_STREAM_UPDATES: usize = 256;
pub const MAX_PARTS: usize = 32;
pub const MAX_ARTIFACTS: usize = 16;
pub const MAX_TASK_MS: u64 = 900_000;
pub const MAX_IDLE_MS: u64 = 10_000;
pub const MAX_CANCEL_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Bound,
    Invalid,
    UnsupportedContent,
    UnsupportedExtension,
    Remote(i64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Send,
    Stream,
    Cancel,
}
fn id(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > MAX_REMOTE_ID_BYTES || value.chars().any(char::is_control)
    {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}
fn encode(value: Value) -> Result<Vec<u8>, Error> {
    let bytes = serde_json::to_vec(&value).map_err(|_| Error::Invalid)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(Error::Bound);
    }
    Ok(bytes)
}
/// Only explicitly selected text enters a new task. No RunSpec/transcript input.
pub fn send_request(
    rpc_id: &str,
    message_id: &str,
    input: &str,
    stream: bool,
) -> Result<Vec<u8>, Error> {
    id(rpc_id)?;
    id(message_id)?;
    if input.len() > MAX_INPUT_BYTES {
        return Err(Error::Bound);
    }
    encode(
        json!({"jsonrpc":"2.0","id":rpc_id,"method":if stream {"SendStreamingMessage"} else {"SendMessage"},"params":{"message":{"messageId":message_id,"role":"ROLE_USER","parts":[{"text":input}]},"configuration":{"acceptedOutputModes":["text/plain","application/json"],"historyLength":0}}}),
    )
}
pub fn cancel_request(rpc_id: &str, task_id: &str) -> Result<Vec<u8>, Error> {
    id(rpc_id)?;
    id(task_id)?;
    encode(json!({"jsonrpc":"2.0","id":rpc_id,"method":"CancelTask","params":{"id":task_id}}))
}
#[derive(Debug, Deserialize)]
struct Envelope {
    jsonrpc: String,
    id: String,
    result: Option<Box<RawValue>>,
    error: Option<RpcError>,
}
#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ResultBody {
    message: Option<Message>,
    task: Option<Task>,
    status_update: Option<StatusUpdate>,
    artifact_update: Option<ArtifactUpdate>,
}
#[derive(Debug, Serialize)]
pub enum Reply {
    Message(Message),
    Task(Task),
    Status(StatusUpdate),
    Artifact(ArtifactUpdate),
}
pub fn decode(
    bytes: &[u8],
    expected_id: &str,
    mode: Mode,
    trace_context: bool,
) -> Result<Reply, Error> {
    id(expected_id)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(Error::Bound);
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    if envelope.jsonrpc != "2.0" || envelope.id != expected_id {
        return Err(Error::Invalid);
    }
    match (envelope.result, envelope.error) {
        (None, Some(error)) => {
            if error.message.len() > 1024 {
                return Err(Error::Bound);
            }
            // No untrusted message/data is promoted into local diagnostics.
            Err(Error::Remote(error.code))
        }
        (Some(result), None) => {
            if mode == Mode::Cancel {
                let task: Task = serde_json::from_str(result.get()).map_err(|_| Error::Invalid)?;
                task.validate(trace_context)?;
                return Ok(Reply::Task(task));
            }
            let body: ResultBody =
                serde_json::from_str(result.get()).map_err(|_| Error::Invalid)?;
            if [
                body.message.is_some(),
                body.task.is_some(),
                body.status_update.is_some(),
                body.artifact_update.is_some(),
            ]
            .into_iter()
            .filter(|v| *v)
            .count()
                != 1
            {
                return Err(Error::Invalid);
            }
            if let Some(message) = body.message {
                message.validate(trace_context)?;
                return Ok(Reply::Message(message));
            }
            if let Some(task) = body.task {
                task.validate(trace_context)?;
                return Ok(Reply::Task(task));
            }
            if mode != Mode::Stream {
                return Err(Error::Invalid);
            }
            if let Some(status) = body.status_update {
                id(&status.task_id)?;
                id(&status.context_id)?;
                status.status.validate(trace_context)?;
                return Ok(Reply::Status(status));
            }
            let artifact = body.artifact_update.ok_or(Error::Invalid)?;
            id(&artifact.task_id)?;
            id(&artifact.context_id)?;
            artifact.artifact.validate()?;
            Ok(Reply::Artifact(artifact))
        }
        _ => Err(Error::Invalid),
    }
}
// Unlike Option's default decoder, this preserves explicit protobuf data:null.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub text: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    raw: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    url: Option<String>,
}
fn parts(parts: &[Part]) -> Result<(), Error> {
    if parts.is_empty() || parts.len() > MAX_PARTS {
        return Err(Error::Bound);
    }
    for part in parts {
        if part.raw.is_some() || part.url.is_some() {
            return Err(Error::UnsupportedContent);
        }
        if part.text.is_some() == part.data.is_some() {
            return Err(Error::Invalid);
        }
        if part.filename.as_ref().is_some_and(|v| v.len() > 256) {
            return Err(Error::Bound);
        }
        if part.media_type.as_deref().is_some_and(|v| {
            v != if part.text.is_some() {
                "text/plain"
            } else {
                "application/json"
            }
        }) {
            return Err(Error::UnsupportedContent);
        }
    }
    Ok(())
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub message_id: String,
    pub context_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    role: String,
    pub parts: Vec<Part>,
    #[serde(default)]
    extensions: Vec<String>,
}
impl Message {
    fn validate(&self, trace: bool) -> Result<(), Error> {
        id(&self.message_id)?;
        id(&self.context_id)?;
        if let Some(task) = &self.task_id {
            id(task)?;
        }
        if self.role != "ROLE_AGENT" {
            return Err(Error::Invalid);
        }
        if self.extensions.len() > 1
            || self
                .extensions
                .iter()
                .any(|e| !trace || e != TRACE_EXTENSION)
        {
            return Err(Error::UnsupportedExtension);
        }
        parts(&self.parts)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum TaskState {
    #[serde(rename = "TASK_STATE_SUBMITTED")]
    Submitted,
    #[serde(rename = "TASK_STATE_WORKING")]
    Working,
    #[serde(rename = "TASK_STATE_COMPLETED")]
    Completed,
    #[serde(rename = "TASK_STATE_FAILED")]
    Failed,
    #[serde(rename = "TASK_STATE_CANCELED")]
    Canceled,
    #[serde(rename = "TASK_STATE_REJECTED")]
    Rejected,
    #[serde(rename = "TASK_STATE_INPUT_REQUIRED")]
    InputRequired,
    #[serde(rename = "TASK_STATE_AUTH_REQUIRED")]
    AuthRequired,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(default)]
    pub message: Option<Message>,
}
impl TaskStatus {
    fn validate(&self, trace: bool) -> Result<(), Error> {
        self.message.as_ref().map_or(Ok(()), |m| m.validate(trace))
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
impl Artifact {
    fn validate(&self) -> Result<(), Error> {
        id(&self.artifact_id)?;
        if self.name.as_ref().is_some_and(|v| v.len() > 256)
            || self.description.as_ref().is_some_and(|v| v.len() > 8192)
        {
            return Err(Error::Bound);
        }
        parts(&self.parts)
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub context_id: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing)]
    history: Vec<Value>,
}
impl Task {
    fn validate(&self, trace: bool) -> Result<(), Error> {
        id(&self.id)?;
        id(&self.context_id)?;
        self.status.validate(trace)?;
        if !self.history.is_empty() || self.artifacts.len() > MAX_ARTIFACTS {
            return Err(Error::Bound);
        }
        let mut ids = std::collections::BTreeSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            if !ids.insert(&artifact.artifact_id) {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusUpdate {
    pub task_id: String,
    pub context_id: String,
    pub status: TaskStatus,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactUpdate {
    pub task_id: String,
    pub context_id: String,
    pub artifact: Artifact,
    #[serde(default)]
    pub append: bool,
    #[serde(default)]
    pub last_chunk: bool,
}
/// Charge each decoded SSE data envelope before parsing; transport framing and
/// total wire bytes must also be bounded by the execution checkpoint's reader.
#[derive(Default)]
pub struct StreamBudget {
    bytes: usize,
    updates: usize,
}
impl StreamBudget {
    pub fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        if bytes > MAX_RESPONSE_BYTES
            || bytes > MAX_STREAM_BYTES.saturating_sub(self.bytes)
            || self.updates >= MAX_STREAM_UPDATES
        {
            return Err(Error::Bound);
        }
        self.bytes += bytes;
        self.updates += 1;
        Ok(())
    }
}
