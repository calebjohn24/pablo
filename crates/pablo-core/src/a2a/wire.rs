//! Bounded A2A 1.0 JSONRPC profile. No transport, replay or local authority.
use super::{MAX_REMOTE_ID_BYTES, TRACE_EXTENSION};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};

pub const MAX_FILE_BYTES: usize = 32 * 1024;
pub const MAX_URL_BYTES: usize = 4096;
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
/// Only explicitly selected parts enter a new task; no file/URL is read here.
pub struct SendOptions<'a> {
    pub stream: bool,
    pub accepted_output_modes: &'a [&'a str],
    pub trace: Option<&'a super::trace::TraceContext>,
    pub trace_negotiated: bool,
}
impl Default for SendOptions<'_> {
    fn default() -> Self {
        Self {
            stream: false,
            accepted_output_modes: &["text/plain", "application/json"],
            trace: None,
            trace_negotiated: false,
        }
    }
}
pub struct EncodedRequest {
    pub body: Vec<u8>,
    pub headers: reqwest::header::HeaderMap,
}
pub fn send_request(
    rpc_id: &str,
    message_id: &str,
    input: &str,
    stream: bool,
) -> Result<Vec<u8>, Error> {
    Ok(send_parts_request(
        rpc_id,
        message_id,
        &[Part::text(input)],
        &SendOptions {
            stream,
            ..Default::default()
        },
    )?
    .body)
}
pub fn send_parts_request(
    rpc_id: &str,
    message_id: &str,
    selected: &[Part],
    options: &SendOptions<'_>,
) -> Result<EncodedRequest, Error> {
    id(rpc_id)?;
    id(message_id)?;
    parts(selected)?;
    let mut size = 0usize;
    for part in selected {
        size = size.checked_add(part.input_bytes()?).ok_or(Error::Bound)?;
        if size > MAX_INPUT_BYTES {
            return Err(Error::Bound);
        }
    }
    if options.accepted_output_modes.is_empty()
        || options.accepted_output_modes.len() > 16
        || options.accepted_output_modes.iter().any(|s| !media_type(s))
    {
        return Err(Error::UnsupportedContent);
    }
    let mut request = json!({"jsonrpc":"2.0","id":rpc_id,"method":if options.stream {"SendStreamingMessage"} else {"SendMessage"},"params":{"message":{"messageId":message_id,"role":"ROLE_USER","parts":selected},"configuration":{"acceptedOutputModes":options.accepted_output_modes,"historyLength":0}}});
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "a2a-version",
        reqwest::header::HeaderValue::from_static(super::PROTOCOL_VERSION),
    );
    headers.insert(
        "content-type",
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        "accept",
        reqwest::header::HeaderValue::from_static(if options.stream {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    // Generic peers continue without the optional extension or any private data.
    if options.trace_negotiated
        && let Some(trace) = options.trace
    {
        request["params"]["metadata"] = trace.metadata();
        request["params"]["message"]["extensions"] = json!([TRACE_EXTENSION]);
        headers.extend(trace.headers());
    }
    Ok(EncodedRequest {
        body: encode(request)?,
        headers,
    })
}
pub(crate) fn media_type(value: &str) -> bool {
    if value.len() > 128 || !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return false;
    }
    // Media labels are descriptive data; parameters are retained, not executed.
    let base = value.split(';').next().unwrap_or("").trim();
    let Some((kind, subtype)) = base.split_once('/') else {
        return false;
    };
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-*".contains(&b))
    };
    value.len() <= 128
        && token(kind)
        && token(subtype)
        && (!kind.contains('*') || base == "*/*")
        && (!subtype.contains('*') || subtype == "*")
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
impl Reply {
    pub fn reported_usage(&self) -> Option<super::usage::ReportedUsage> {
        match self {
            Self::Message(v) => v.reported_usage,
            Self::Task(v) => v.reported_usage,
            Self::Status(v) => v.reported_usage,
            Self::Artifact(v) => v.reported_usage,
        }
    }

    pub fn remote_trace(&self) -> Result<Option<super::trace::TraceContext>, Error> {
        Ok(match self {
            Self::Message(v) => v.correlation.clone(),
            Self::Task(v) => v.correlation.clone(),
            Self::Status(v) => v.correlation.clone(),
            Self::Artifact(v) => v.correlation.clone(),
        })
    }
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
                let mut task: Task =
                    serde_json::from_str(result.get()).map_err(|_| Error::Invalid)?;
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
            if let Some(mut message) = body.message {
                message.validate(trace_context)?;
                return Ok(Reply::Message(message));
            }
            if let Some(mut task) = body.task {
                task.validate(trace_context)?;
                return Ok(Reply::Task(task));
            }
            if mode != Mode::Stream {
                return Err(Error::Invalid);
            }
            if let Some(mut status) = body.status_update {
                id(&status.task_id)?;
                id(&status.context_id)?;
                status
                    .status
                    .validate_for(trace_context, &status.context_id, &status.task_id)?;
                status.correlation =
                    super::trace::remote_metadata(&status.metadata, trace_context)?;
                status.reported_usage = super::usage::decode(&status.metadata)?;
                return Ok(Reply::Status(status));
            }
            let mut artifact = body.artifact_update.ok_or(Error::Invalid)?;
            id(&artifact.task_id)?;
            id(&artifact.context_id)?;
            artifact.artifact.validate()?;
            artifact.correlation =
                super::trace::remote_metadata(&artifact.metadata, trace_context)?;
            artifact.reported_usage = super::usage::decode(&artifact.metadata)?;
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
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    pub raw: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub url: Option<String>,
}
impl Part {
    pub fn text(value: &str) -> Self {
        Self {
            text: Some(value.into()),
            ..Default::default()
        }
    }
    pub fn data(value: Value) -> Self {
        Self {
            data: Some(value),
            ..Default::default()
        }
    }
    pub fn file(bytes: &[u8], media_type: &str, filename: Option<&str>) -> Result<Self, Error> {
        use base64::Engine;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::Bound);
        }
        let part = Self {
            raw: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            media_type: Some(media_type.into()),
            filename: filename.map(Into::into),
            ..Default::default()
        };
        parts(std::slice::from_ref(&part))?;
        Ok(part)
    }
    pub fn url_reference(url: &str, media_type: &str) -> Result<Self, Error> {
        let part = Self {
            url: Some(url.into()),
            media_type: Some(media_type.into()),
            ..Default::default()
        };
        parts(std::slice::from_ref(&part))?;
        Ok(part)
    }
    /// Decode file content only. Never writes a file or follows a URL reference.
    pub fn file_bytes(&self) -> Result<Option<Vec<u8>>, Error> {
        use base64::{
            Engine,
            engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
        };
        let Some(raw) = &self.raw else {
            return Ok(None);
        };
        if raw.len() > MAX_FILE_BYTES.div_ceil(3) * 4 {
            return Err(Error::Bound);
        }
        let bytes = [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
            .iter()
            .find_map(|e| e.decode(raw).ok())
            .ok_or(Error::Invalid)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::Bound);
        }
        Ok(Some(bytes))
    }
    pub(crate) fn input_bytes(&self) -> Result<usize, Error> {
        if let Some(text) = &self.text {
            return Ok(text.len());
        }
        if let Some(data) = &self.data {
            return serde_json::to_vec(data)
                .map(|v| v.len())
                .map_err(|_| Error::Invalid);
        }
        if let Some(bytes) = self.file_bytes()? {
            return Ok(bytes.len());
        }
        self.url.as_ref().map(String::len).ok_or(Error::Invalid)
    }
}
fn parts(parts: &[Part]) -> Result<(), Error> {
    if parts.is_empty() || parts.len() > MAX_PARTS {
        return Err(Error::Bound);
    }
    for part in parts {
        if [
            part.text.is_some(),
            part.data.is_some(),
            part.raw.is_some(),
            part.url.is_some(),
        ]
        .into_iter()
        .filter(|v| *v)
        .count()
            != 1
        {
            return Err(Error::Invalid);
        }
        part.file_bytes()?;
        if let Some(url) = &part.url {
            if url.len() > MAX_URL_BYTES {
                return Err(Error::Bound);
            }
            let parsed = reqwest::Url::parse(url).map_err(|_| Error::Invalid)?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(Error::UnsupportedContent);
            }
        }
        if part.filename.as_ref().is_some_and(|v| v.len() > 256) {
            return Err(Error::Bound);
        }
        if part.media_type.as_deref().is_some_and(|v| {
            !media_type(v) || v.split(';').next().is_some_and(|base| base.contains('*'))
        }) {
            return Err(Error::UnsupportedContent);
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[serde(default, skip_serializing)]
    metadata: Value,
    #[serde(skip)]
    correlation: Option<super::trace::TraceContext>,
    #[serde(skip)]
    reported_usage: Option<super::usage::ReportedUsage>,
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
    fn validate(&mut self, trace: bool) -> Result<(), Error> {
        id(&self.message_id)?;
        id(&self.context_id)?;
        if let Some(task) = &self.task_id {
            id(task)?;
        }
        if self.role != "ROLE_AGENT" {
            return Err(Error::Invalid);
        }
        if self.extensions.len() > 16
            || self
                .extensions
                .iter()
                .any(|e| e.is_empty() || e.len() > 2048)
        {
            return Err(Error::Bound);
        }
        self.correlation = super::trace::remote_metadata(&self.metadata, trace)?;
        self.reported_usage = super::usage::decode(&self.metadata)?;
        parts(&self.parts)
    }
    pub fn remote_trace(&self) -> Result<Option<super::trace::TraceContext>, Error> {
        Ok(self.correlation.clone())
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
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(default)]
    pub message: Option<Message>,
}
impl TaskStatus {
    fn validate_for(&mut self, trace: bool, context: &str, task: &str) -> Result<(), Error> {
        self.validate(trace)?;
        if self.message.as_ref().is_some_and(|m| {
            m.context_id != context || m.task_id.as_deref().is_some_and(|id| id != task)
        }) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn validate(&mut self, trace: bool) -> Result<(), Error> {
        self.message.as_mut().map_or(Ok(()), |m| m.validate(trace))
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    #[serde(default, skip_serializing)]
    metadata: Value,
    #[serde(skip)]
    correlation: Option<super::trace::TraceContext>,
    #[serde(skip)]
    reported_usage: Option<super::usage::ReportedUsage>,
    pub id: String,
    pub context_id: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing)]
    history: Vec<Value>,
}
impl Task {
    fn validate(&mut self, trace: bool) -> Result<(), Error> {
        self.correlation = super::trace::remote_metadata(&self.metadata, trace)?;
        self.reported_usage = super::usage::decode(&self.metadata)?;
        id(&self.id)?;
        id(&self.context_id)?;
        self.status
            .validate_for(trace, &self.context_id, &self.id)?;
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
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusUpdate {
    #[serde(default, skip_serializing)]
    metadata: Value,
    #[serde(skip)]
    correlation: Option<super::trace::TraceContext>,
    #[serde(skip)]
    reported_usage: Option<super::usage::ReportedUsage>,
    pub task_id: String,
    pub context_id: String,
    pub status: TaskStatus,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactUpdate {
    #[serde(default, skip_serializing)]
    metadata: Value,
    #[serde(skip)]
    correlation: Option<super::trace::TraceContext>,
    #[serde(skip)]
    reported_usage: Option<super::usage::ReportedUsage>,
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
