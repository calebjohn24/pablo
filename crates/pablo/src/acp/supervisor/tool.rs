//! Model and host operations share the same owned registry and admission path.
use super::*;
use pablo_core::{
    children::{ChildAction, owner::RootOwner},
    tool::{Tool, ToolContext, ToolDescriptor, ToolResult, ToolStatus},
};

impl RootOwner for Supervisor {
    fn root(&self) -> &AgentRef {
        &self.inner.root
    }
    fn ledger(&self) -> &RootLedger {
        &self.inner.ledger
    }
    fn cancellation(&self) -> &CancellationToken {
        &self.inner.root_cancel
    }
    fn model_tool_allowed(&self) -> bool {
        self.inner.parent.admits_subagent_tool()
    }
    fn claim_root(&self) -> bool {
        self.inner
            .root_claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    fn cancel(&self) {
        self.inner.registry.lock().unwrap().closed = true;
        self.inner.cancel.cancel();
        self.inner.ledger.close_admission();
        // The manager closes updates after active work and its native closings
        // settle. Closing here would discard healthy cancellation delivery.
    }
    fn close(&self) -> futures::future::BoxFuture<'_, Result<(), &'static str>> {
        Box::pin(async move { Supervisor::close(self).await })
    }
}
impl Tool for Supervisor {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "subagent".into(),
            description: "Run one temporary child with an explicitly selected task and inherited capabilities. Spawn returns a handle immediately; inspect and wait return bounded state/results; stop joins owned child work. Children share the workspace and root budgets.".into(),
            input_schema: serde_json::json!({
                "type":"object","additionalProperties":false,
                "properties":{
                    "action":{"enum":["spawn","inspect","wait","stop"]},
                    "request":{"type":"object","additionalProperties":false,"required":["input"],
                        "properties":{
                            "input":{"type":"string","minLength":1,"maxLength":1048576},
                            "overlay":{"type":["string","null"]},"context":{"type":"array","items":{"type":"string"}},
                            "capabilities":{"type":"object","additionalProperties":false,"properties":{
                                "tools":{"type":["array","null"],"items":{"type":"string"}},
                                "skills":{"type":["array","null"],"items":{"type":"string"}},
                                "mcp_servers":{"type":["array","null"],"items":{"type":"string"}},
                                "model_route":{"type":["array","null"],"items":{"type":"string"}}
                            }},
                            "ceilings":{"type":"object","additionalProperties":false,"properties":{
                                "max_model_calls":{"type":["integer","null"],"minimum":0},
                                "max_tool_calls":{"type":["integer","null"],"minimum":0},
                                "max_duration_ms":{"type":["integer","null"],"minimum":0},
                                "max_context_bytes":{"type":["integer","null"],"minimum":0},
                                "max_output_bytes":{"type":["integer","null"],"minimum":0},
                                "max_total_tokens":{"type":["string","null"],"pattern":"^(0|[1-9][0-9]*)$"},
                                "max_cost_microusd":{"type":["string","null"],"pattern":"^(0|[1-9][0-9]*)$"}
                            }},"output_schema":{}
                        }},
                    "agent_id":{"type":"string"},
                    "agent_ids":{"type":"array","minItems":1,"maxItems":16,"uniqueItems":true,"items":{"type":"string"}},
                    "mode":{"enum":["any","all"]},"timeout_ms":{"type":"integer","minimum":0,"maximum":900000}
                },
                "required":["action"],
                "oneOf":[
                    {"properties":{"action":{"const":"spawn"}},"required":["request"]},
                    {"properties":{"action":{"const":"inspect"}},"required":["agent_id"]},
                    {"properties":{"action":{"const":"stop"}},"required":["agent_id"]},
                    {"properties":{"action":{"const":"wait"}},"required":["agent_ids","mode","timeout_ms"]}
                ]
            }),
        }
    }
    fn execute<'a>(
        &'a self,
        arguments: Value,
        context: ToolContext<'a>,
    ) -> futures::future::BoxFuture<'a, ToolResult> {
        Box::pin(async move {
            if context.cancellation.is_cancelled() {
                return ToolResult::status(ToolStatus::Cancelled);
            }
            if Instant::now() >= context.deadline {
                return ToolResult::status(ToolStatus::TimedOut);
            }
            let action: ChildAction = match serde_json::from_value(arguments) {
                Ok(action) => action,
                Err(_) => return ToolResult::status(ToolStatus::InvalidArguments),
            };
            if action.validate_shape().is_err() {
                return ToolResult::status(ToolStatus::InvalidArguments);
            }
            let value = match action {
                ChildAction::Spawn { request } => self
                    .spawn(&request, context.context)
                    .map(|agent| json!({"action":"spawn","agent":agent})),
                ChildAction::Inspect { agent_id } => self
                    .inspect(&agent_id)
                    .map(|snapshot| json!({"action":"inspect","snapshot":snapshot})),
                ChildAction::Stop { agent_id } => self
                    .stop(&agent_id)
                    .await
                    .map(|snapshot| json!({"action":"stop","snapshot":snapshot})),
                ChildAction::Wait {
                    agent_ids,
                    mode,
                    timeout_ms,
                } => {
                    let remaining = context
                        .deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64;
                    tokio::select! {
                        biased;
                        _ = context.cancellation.cancelled() => return ToolResult::status(ToolStatus::Cancelled),
                        result = self.wait(&agent_ids, mode, timeout_ms.min(remaining)) => result.map(|result| json!({"action":"wait","settled":result.settled,"remaining":result.remaining})),
                    }
                }
            };
            let mut result = ToolResult::status(if value.is_ok() {
                ToolStatus::Completed
            } else {
                ToolStatus::RecoverableError
            });
            result.subagent = Some(Box::new(
                value.unwrap_or_else(|code| json!({"action":"error","code":code})),
            ));
            if serde_json::to_vec(&result).map_or(true, |bytes| {
                bytes.len() > context.limits.max_tool_output_bytes
            }) {
                return ToolResult::status(ToolStatus::OutputLimit);
            }
            result
        })
    }
}
