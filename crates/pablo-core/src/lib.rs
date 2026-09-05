//! A bounded, streamed Rust runtime with sequential model and shell calls.
//!
//! Hosts inject telemetry and consume events as they happen. No process-global
//! providers, network exporters, environment files, or worker tasks are installed.

pub mod contracts;
pub mod events;
pub mod gateway;
pub mod provider;
pub mod runtime;
pub mod shell;
pub mod telemetry;
pub mod tool;

pub use contracts::*;
pub use events::{EventSink, JsonlSink, SinkError};
pub use provider::{Provider, ScriptedProvider};
pub use runtime::{RunError, Runtime};
pub use tokio_util::sync::CancellationToken;
pub use tool::{Tool, ToolRegistry};
