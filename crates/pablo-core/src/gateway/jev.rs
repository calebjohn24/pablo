//! Task-start classification only; never a text generation or fallback adapter.
use super::*;
use crate::deployment::JevRouter;

pub const JEV_MODEL: &str = "typesafe-ai/jev";
pub const JEV_ENDPOINT: &str = "https://ai-gateway.vercel.sh/v4/ai/evaluation-model";

pub struct JevProvider {
    pub(crate) config: JevRouter,
    transport: transport::Transport,
}
impl JevProvider {
    pub fn new(config: JevRouter, key: &str) -> Result<Self, &'static str> {
        Self::build(config, transport::Transport::new(JEV_ENDPOINT, key, false)?)
    }
    /// Host-only loopback fixture: never accepts a real credential.
    pub fn local_fixture(config: JevRouter, endpoint: &str) -> Result<Self, &'static str> {
        Self::build(
            config,
            transport::Transport::new(endpoint, "pablo-local-fixture", true)?,
        )
    }
    fn build(config: JevRouter, transport: transport::Transport) -> Result<Self, &'static str> {
        if config.provider != "vercel" || config.model != JEV_MODEL {
            return Err("Jev routing requires Vercel AI Gateway");
        }
        Ok(Self { config, transport })
    }
}
impl Provider for JevProvider {
    fn name(&self) -> &'static str {
        "vercel"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let criteria: BTreeMap<_, _> = self
                .config
                .categories
                .iter()
                .map(|(name, category)| (name, &category.description))
                .collect();
            // Only immutable original task context: never accumulated tool history.
            let body = serde_json::to_vec(&json!({
                "state": {"task": request.input, "instructions": request.instructions},
                "questions": {"complexity": {
                    "type": "choice", "instructions": self.config.instructions, "criteria": criteria
                }}
            }))
            .map_err(|_| malformed())?;
            if body.len() > 128 * 1024 {
                return Err(ProviderError {
                    code: FailureCode::ContextOverflow,
                    delivery: DeliveryCertainty::NotSent,
                    retry_class: None,
                });
            }
            let value = self.transport.evaluate(body, request.deadline).await?;
            let answer = &value["answers"]["complexity"];
            let choice = answer["choice"]
                .as_str()
                .filter(|choice| {
                    answer["type"] == "choice" && self.config.categories.contains_key(*choice)
                })
                .ok_or_else(malformed)?;
            let tokens = |key| -> Result<Option<u64>, ProviderError> {
                match value.get("usage").and_then(|usage| usage.get(key)) {
                    None => Ok(None),
                    Some(value) => value.as_u64().map(Some).ok_or_else(malformed),
                }
            };
            let usage = Usage {
                input_tokens: tokens("inputTokens")?,
                output_tokens: tokens("outputTokens")?,
                ..Usage::default()
            };
            Ok(Box::pin(stream::iter([
                Ok(ProviderEvent::TextDelta(choice.into())),
                Ok(ProviderEvent::Finished {
                    reason: FinishReason::Stop,
                    usage,
                }),
            ])) as ProviderStream<'a>)
        })
    }
}
