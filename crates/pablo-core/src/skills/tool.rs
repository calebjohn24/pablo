use super::{
    Error,
    activation::{ActivatedSkills, MAX_RESOURCE_BYTES, Resource},
};
use crate::tool::{
    Tool, ToolContext, ToolDescriptor, ToolResult, ToolSetupError, ToolStatus, compile_schema,
};
use futures_util::future::BoxFuture;
use opentelemetry::{KeyValue, trace::TraceContextExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<Resource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}
#[derive(Serialize)]
pub(crate) struct RedactedSkillResult<'a> {
    pub qualified_name: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub bytes: usize,
    pub error: Option<Error>,
}
impl SkillResult {
    pub(crate) fn redacted(&self) -> RedactedSkillResult<'_> {
        RedactedSkillResult {
            qualified_name: self.resource.as_ref().map(|r| r.qualified_name.as_str()),
            sha256: self.resource.as_ref().map(|r| r.sha256.as_str()),
            bytes: self.resource.as_ref().map_or(0, |r| r.bytes),
            error: self.error,
        }
    }
}
pub(crate) struct ResourceTool {
    pub active: ActivatedSkills,
    descriptor: ToolDescriptor,
    validator: jsonschema::Validator,
}
impl ResourceTool {
    pub fn new(active: ActivatedSkills) -> Result<Self, ToolSetupError> {
        let descriptor=ToolDescriptor {name:"skill.read".into(),description:"Read one UTF-8 resource from an explicitly activated Skill. Use its exact ROOT/NAME and a relative path. This grants no execution or write authority.".into(),input_schema:json!({"type":"object","additionalProperties":false,"properties":{"skill":{"type":"string","minLength":1,"maxLength":128},"path":{"type":"string","minLength":1,"maxLength":4096},"max_bytes":{"type":"integer","minimum":1,"maximum":MAX_RESOURCE_BYTES}},"required":["skill","path"]})};
        let validator = compile_schema(&descriptor.input_schema)?;
        Ok(Self {
            active,
            descriptor,
            validator,
        })
    }
}
impl Tool for ResourceTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
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
            let max = arguments["max_bytes"]
                .as_u64()
                .map_or(MAX_RESOURCE_BYTES, |n| n as usize)
                .min(context.limits.max_tool_output_bytes.saturating_sub(1024));
            if max == 0 {
                return ToolResult::status(ToolStatus::OutputLimit);
            }
            let read = self
                .active
                .read_resource(
                    arguments["skill"].as_str().unwrap().into(),
                    arguments["path"].as_str().unwrap().into(),
                    max,
                    context.cancellation.clone(),
                    context.deadline,
                )
                .await;
            let (status, resource, error) = match read {
                Ok(resource) => {
                    context.context.span().set_attribute(KeyValue::new(
                        "pablo.skill.resource.sha256",
                        resource.sha256.clone(),
                    ));
                    context.context.span().set_attribute(KeyValue::new(
                        "pablo.skill.resource.bytes",
                        resource.bytes as i64,
                    ));
                    (ToolStatus::Completed, Some(resource), None)
                }
                Err(error) => {
                    let status = match error {
                        Error::Cancelled => ToolStatus::Cancelled,
                        Error::ScanBound => ToolStatus::TimedOut,
                        Error::ResourceBound => ToolStatus::OutputLimit,
                        _ => ToolStatus::RecoverableError,
                    };
                    (status, None, Some(error))
                }
            };
            let mut result = ToolResult::status(status);
            result.skill = Some(Box::new(SkillResult { resource, error }));
            if serde_json::to_vec(&result)
                .expect("serializable Skill result")
                .len()
                > context.limits.max_tool_output_bytes
            {
                return ToolResult::status(ToolStatus::OutputLimit);
            }
            result
        })
    }
}
