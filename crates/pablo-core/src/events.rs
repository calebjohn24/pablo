use std::{
    fmt,
    io::{self, Write},
};

use crate::{EventKind, RunEvent, RunSpec};
use serde::{Serialize, Serializer, ser::SerializeMap};

#[derive(Debug)]
pub enum SinkError {
    Capacity,
    Io(io::Error),
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Capacity => "native trace capacity exhausted",
            Self::Io(_) => "event sink I/O failed",
        })
    }
}
impl std::error::Error for SinkError {}
impl From<io::Error> for SinkError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Called inline, in sequence, without an internal queue. Sinks must return
/// promptly; blocking I/O cannot be interrupted by the asynchronous deadline.
pub trait EventSink: Send {
    fn emit(&mut self, event: &RunEvent) -> Result<(), SinkError>;
}

impl<F> EventSink for F
where
    F: FnMut(&RunEvent) -> Result<(), SinkError> + Send,
{
    fn emit(&mut self, event: &RunEvent) -> Result<(), SinkError> {
        self(event)
    }
}

/// Bounded JSONL projection. Content capture is independent of OTel, which
/// never receives task content. The writer should be dedicated to one run.
pub struct JsonlSink<W> {
    writer: W,
    capture_content: bool,
    max_bytes: usize,
    terminal_reserve: usize,
    bytes_written: usize,
    closed: bool,
    buffer: Vec<u8>,
}

impl<W: Write> JsonlSink<W> {
    pub fn new(writer: W, spec: &RunSpec) -> Result<Self, SinkError> {
        // JSON can expand each UTF-8 byte to six bytes (e.g. control characters).
        // Identifiers are bounded by runtime validation; 4 KiB covers metadata.
        let content_reserve = if spec.trace.capture_content {
            spec.limits
                .max_output_bytes
                .checked_mul(6)
                .ok_or(SinkError::Capacity)?
        } else {
            0
        };
        let terminal_reserve = 4096usize
            .checked_add(content_reserve)
            .ok_or(SinkError::Capacity)?;
        if spec.trace.max_bytes < terminal_reserve.saturating_add(4096) {
            return Err(SinkError::Capacity);
        }
        Ok(Self {
            writer,
            capture_content: spec.trace.capture_content,
            max_bytes: spec.trace.max_bytes,
            terminal_reserve,
            bytes_written: 0,
            closed: false,
            buffer: Vec::with_capacity(4096),
        })
    }

    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write + Send> EventSink for JsonlSink<W> {
    fn emit(&mut self, event: &RunEvent) -> Result<(), SinkError> {
        if self.closed {
            return Err(io::Error::other("trace is closed").into());
        }
        let terminal = matches!(event.kind, EventKind::RunFinished { .. });
        let ceiling = if terminal {
            self.max_bytes
        } else {
            self.max_bytes - self.terminal_reserve
        };
        let remaining = ceiling.saturating_sub(self.bytes_written);
        if remaining == 0 {
            return Err(SinkError::Capacity);
        }
        self.buffer.clear();
        let mut buffer = BoundedBuffer {
            bytes: &mut self.buffer,
            limit: remaining - 1, // Reserve the newline as part of the record.
        };
        // Redact through borrowed views before serialization. Never copy task
        // content into a temporary JSON tree, even with content capture enabled.
        let encoded = if self.capture_content {
            serde_json::to_writer(&mut buffer, event)
        } else {
            serde_json::to_writer(&mut buffer, &RedactedEvent(event))
        };
        encoded.map_err(|error| {
            if error.io_error_kind() == Some(io::ErrorKind::FileTooLarge) {
                SinkError::Capacity
            } else {
                SinkError::Io(io::Error::other(error))
            }
        })?;
        self.buffer.push(b'\n');
        // No partial record is written on capacity failure. I/O failure can
        // leave a partial record and cannot promise terminal delivery.
        self.writer.write_all(&self.buffer)?;
        self.bytes_written += self.buffer.len();
        if terminal {
            self.closed = true;
            self.writer.flush()?;
        }
        Ok(())
    }
}

/// Stops oversized serialization in memory, before any of the record is written.
struct BoundedBuffer<'a> {
    bytes: &'a mut Vec<u8>,
    limit: usize,
}
impl Write for BoundedBuffer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct RedactedEvent<'a>(&'a RunEvent);
impl Serialize for RedactedEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let e = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("schema_version", &e.schema_version)?;
        map.serialize_entry("seq", &e.seq)?;
        map.serialize_entry("timestamp_unix_micros", &e.timestamp_unix_micros)?;
        map.serialize_entry("run_id", &e.run_id)?;
        map.serialize_entry("session_id", &e.session_id)?;
        map.serialize_entry("trace_id", &e.trace_id)?;
        map.serialize_entry("span_id", &e.span_id)?;
        map.serialize_entry("parent_span_id", &e.parent_span_id)?;
        map.serialize_entry("trace_flags", &e.trace_flags)?;
        if let Some(profile) = &e.model_profile {
            map.serialize_entry("model_profile", profile)?;
        }
        if let Some(deployment) = &e.deployment {
            map.serialize_entry("deployment", deployment)?;
        }
        if let Some(accounting) = &e.accounting {
            map.serialize_entry("accounting", accounting)?;
        }
        match &e.kind {
            EventKind::RunStarted => map.serialize_entry("type", "run.started")?,
            EventKind::ModelStarted { provider, model } => {
                map.serialize_entry("type", "model.started")?;
                map.serialize_entry("provider", provider)?;
                map.serialize_entry("model", model)?;
            }
            EventKind::TextDelta { text } => {
                map.serialize_entry("type", "assistant.text.delta")?;
                map.serialize_entry("text", &())?;
                map.serialize_entry("text_bytes", &text.len())?;
                map.serialize_entry("content_redacted", &true)?;
            }
            EventKind::ModelFinished {
                status,
                finish_reason,
                usage,
                output_bytes,
            } => {
                map.serialize_entry("type", "model.finished")?;
                map.serialize_entry("status", status)?;
                map.serialize_entry("finish_reason", finish_reason)?;
                map.serialize_entry("usage", usage)?;
                map.serialize_entry("output_bytes", output_bytes)?;
            }
            EventKind::ToolStarted { call } => {
                #[derive(Serialize)]
                struct Call<'a> {
                    id: &'a str,
                    name: &'a str,
                    arguments: (),
                }
                map.serialize_entry("type", "tool.started")?;
                map.serialize_entry(
                    "call",
                    &Call {
                        id: &call.id,
                        name: &call.name,
                        arguments: (),
                    },
                )?;
                map.serialize_entry("content_redacted", &true)?;
            }
            EventKind::ShellStarted {
                call_id,
                process_id,
            } => {
                map.serialize_entry("type", "shell.started")?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("process_id", process_id)?;
            }
            EventKind::ToolFinished {
                call_id,
                name,
                result,
            } => {
                #[derive(Serialize)]
                struct Result<'a> {
                    status: crate::tool::ToolStatus,
                    policy_rule: &'a Option<crate::PolicyRule>,
                    shell: Option<RedactedShell<'a>>,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    filesystem: Option<crate::filesystem::RedactedFilesystem>,
                    #[serde(skip_serializing_if = "<[_]>::is_empty")]
                    policy_decisions: &'a [String],
                }
                map.serialize_entry("type", "tool.finished")?;
                map.serialize_entry("call_id", call_id)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry(
                    "result",
                    &Result {
                        status: result.status,
                        policy_rule: &result.policy_rule,
                        shell: result.shell.as_ref().map(RedactedShell),
                        filesystem: result.filesystem.as_ref().map(|f| f.redacted()),
                        policy_decisions: &result.policy_decisions,
                    },
                )?;
                map.serialize_entry("content_redacted", &true)?;
            }
            EventKind::RunFinished { outcome } => {
                map.serialize_entry("type", "run.finished")?;
                if let crate::RunOutcome::Completed {
                    output,
                    finish_reason,
                    usage,
                } = outcome
                {
                    #[derive(Serialize)]
                    struct Completed<'a> {
                        status: &'static str,
                        output: (),
                        output_bytes: usize,
                        finish_reason: &'a crate::FinishReason,
                        usage: &'a crate::Usage,
                    }
                    map.serialize_entry(
                        "outcome",
                        &Completed {
                            status: "completed",
                            output: (),
                            output_bytes: output.len(),
                            finish_reason,
                            usage,
                        },
                    )?;
                    map.serialize_entry("content_redacted", &true)?;
                } else {
                    map.serialize_entry("outcome", outcome)?;
                }
            }
        }
        map.end()
    }
}

struct RedactedShell<'a>(&'a crate::tool::ShellResult);
impl Serialize for RedactedShell<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let shell = self.0;
        let mut map = serializer.serialize_map(Some(10))?;
        map.serialize_entry("stdout", &())?;
        map.serialize_entry("stderr", &())?;
        map.serialize_entry("stdout_bytes", &shell.stdout.len())?;
        map.serialize_entry("stderr_bytes", &shell.stderr.len())?;
        map.serialize_entry("exit_code", &shell.exit_code)?;
        map.serialize_entry("signal", &shell.signal)?;
        map.serialize_entry("stdout_truncated", &shell.stdout_truncated)?;
        map.serialize_entry("stderr_truncated", &shell.stderr_truncated)?;
        map.serialize_entry("stdout_lossy", &shell.stdout_lossy)?;
        map.serialize_entry("stderr_lossy", &shell.stderr_lossy)?;
        map.end()
    }
}
