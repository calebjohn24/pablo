use super::{
    stdio::{Error, ResultContent, StdioSession},
    *,
};
use crate::tool::{Tool, ToolContext, ToolDescriptor, ToolResult, ToolStatus};
use futures_util::future::BoxFuture;
use opentelemetry::{KeyValue, trace::TraceContextExt};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ResultContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}
#[derive(Serialize)]
pub(crate) struct RedactedMcpResult {
    error: Option<Error>,
    text_blocks: usize,
    text_bytes: usize,
    structured_bytes: Option<usize>,
    is_error: Option<bool>,
}
impl McpResult {
    pub(crate) fn redacted(&self) -> RedactedMcpResult {
        RedactedMcpResult {
            error: self.error,
            text_blocks: self.content.as_ref().map_or(0, |c| c.text.len()),
            text_bytes: self.content.as_ref().map_or(0, |c| {
                c.text.iter().fold(0usize, |n, s| n.saturating_add(s.len()))
            }),
            structured_bytes: self
                .content
                .as_ref()
                .and_then(|c| c.structured.as_ref())
                .map(|v| {
                    struct Count(usize);
                    impl std::io::Write for Count {
                        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                            self.0 = self.0.saturating_add(bytes.len());
                            Ok(bytes.len())
                        }
                        fn flush(&mut self) -> std::io::Result<()> {
                            Ok(())
                        }
                    }
                    let mut count = Count(0);
                    serde_json::to_writer(&mut count, v).expect("counting JSON cannot fail");
                    count.0
                }),
            is_error: self.content.as_ref().map(|c| c.is_error),
        }
    }
}
pub(crate) struct McpTool {
    pub descriptor: ToolDescriptor,
    pub server: String,
    pub name: String,
    pub session: Arc<Mutex<StdioSession>>,
    pub decisions: Box<[String]>,
}
impl Tool for McpTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }
    fn execute<'a>(
        &'a self,
        arguments: serde_json::Value,
        context: ToolContext<'a>,
    ) -> BoxFuture<'a, ToolResult> {
        Box::pin(async move {
            context.context.span().set_attributes([
                KeyValue::new("mcp.method.name", "tools/call"),
                KeyValue::new("mcp.protocol.version", PROTOCOL_VERSION),
                KeyValue::new("network.transport", "pipe"),
                KeyValue::new("pablo.mcp.server", self.server.clone()),
            ]);
            let result = tokio::select! {
                biased;
                _ = context.cancellation.cancelled() => Err(Error::Cancelled),
                _ = tokio::time::sleep_until(context.deadline) => Err(Error::TimedOut),
                session = self.session.lock() => {
                    let mut session = session;
                    context.context.span().set_attribute(KeyValue::new("pablo.mcp.process.id",i64::from(session.process_id())));
                    session.call_with_context(&self.name,arguments,context.deadline,context.cancellation,&context.context).await
                }
            };
            let (status, content, error) = match result {
                Ok(content) => (
                    if content.is_error {
                        ToolStatus::RecoverableError
                    } else {
                        ToolStatus::Completed
                    },
                    Some(content),
                    None,
                ),
                Err(error) => (
                    match error {
                        Error::PolicyDenied => ToolStatus::PolicyDenied,
                        Error::InvalidArguments => ToolStatus::InvalidArguments,
                        Error::Cancelled => ToolStatus::Cancelled,
                        Error::TimedOut => ToolStatus::TimedOut,
                        Error::Cleanup => ToolStatus::CleanupFailed,
                        Error::Spawn => ToolStatus::SpawnFailed,
                        _ => ToolStatus::IoFailed,
                    },
                    None,
                    Some(error),
                ),
            };
            let mut result = ToolResult::status(status);
            result.policy_decisions = self.decisions.clone();
            result.mcp = Some(Box::new(McpResult { content, error }));
            if !crate::filesystem::fits(&result, context.limits.max_tool_output_bytes) {
                result.status = ToolStatus::OutputLimit;
                result.mcp = None;
            }
            result
        })
    }
}
