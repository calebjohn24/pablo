use std::{
    fmt,
    io::{self, Write},
};

use crate::{EventKind, RunEvent, RunSpec};

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
        // Remove content before it reaches the serializer. Keep the original
        // event intact for live consumers and use its lengths for the projection.
        let redacted_kind = if self.capture_content {
            None
        } else {
            match &event.kind {
                EventKind::TextDelta { .. } => Some(EventKind::TextDelta {
                    text: String::new(),
                }),
                EventKind::ToolStarted { call } => Some(EventKind::ToolStarted {
                    call: crate::ToolCall {
                        arguments: serde_json::Value::Null,
                        ..call.clone()
                    },
                }),
                EventKind::ToolFinished {
                    call_id,
                    name,
                    result,
                } => {
                    let mut result = result.clone();
                    if let Some(shell) = result.shell.as_mut() {
                        shell.stdout.clear();
                        shell.stderr.clear();
                    }
                    Some(EventKind::ToolFinished {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        result,
                    })
                }
                EventKind::RunFinished {
                    outcome:
                        crate::RunOutcome::Completed {
                            finish_reason,
                            usage,
                            ..
                        },
                } => Some(EventKind::RunFinished {
                    outcome: crate::RunOutcome::Completed {
                        output: String::new(),
                        finish_reason: *finish_reason,
                        usage: usage.clone(),
                    },
                }),
                _ => None,
            }
        };
        let redacted = redacted_kind.map(|kind| RunEvent {
            kind,
            ..event.clone()
        });
        let mut value =
            serde_json::to_value(redacted.as_ref().unwrap_or(event)).map_err(io::Error::other)?;
        if !self.capture_content {
            match &event.kind {
                EventKind::TextDelta { text } => {
                    value["text"] = serde_json::Value::Null;
                    value["text_bytes"] = text.len().into();
                    value["content_redacted"] = true.into();
                }
                EventKind::ToolStarted { .. } => {
                    value["content_redacted"] = true.into();
                }
                EventKind::ToolFinished { result, .. } => {
                    if let Some(shell) = &result.shell {
                        value["result"]["shell"]["stdout"] = serde_json::Value::Null;
                        value["result"]["shell"]["stderr"] = serde_json::Value::Null;
                        value["result"]["shell"]["stdout_bytes"] = shell.stdout.len().into();
                        value["result"]["shell"]["stderr_bytes"] = shell.stderr.len().into();
                    }
                    value["content_redacted"] = true.into();
                }
                EventKind::RunFinished {
                    outcome: crate::RunOutcome::Completed { output, .. },
                } => {
                    value["outcome"]["output"] = serde_json::Value::Null;
                    value["outcome"]["output_bytes"] = output.len().into();
                    value["content_redacted"] = true.into();
                }
                _ => {}
            }
        }
        let mut bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let ceiling = if terminal {
            self.max_bytes
        } else {
            self.max_bytes - self.terminal_reserve
        };
        if bytes.len() > ceiling.saturating_sub(self.bytes_written) {
            return Err(SinkError::Capacity);
        }
        // No partial record is written on capacity failure. I/O failure can
        // leave a partial record and cannot promise terminal delivery.
        self.writer.write_all(&bytes)?;
        self.bytes_written += bytes.len();
        if terminal {
            self.closed = true;
            self.writer.flush()?;
        }
        Ok(())
    }
}
