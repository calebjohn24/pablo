//! Direct Vercel/OpenRouter chat-completions adapters. Credentials stay adapter-owned.
use std::{collections::BTreeMap, collections::VecDeque, pin::Pin};

use futures_util::{future::BoxFuture, stream};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{
    DeliveryCertainty, FailureCode, FinishReason, Message, Provider, Usage,
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
};

mod openrouter;
mod profile;
mod responses;
pub use responses::{
    OpenResponsesProfile, OpenResponsesProvider, PROFILE as OPEN_RESPONSES_PROFILE,
    REVISION as OPEN_RESPONSES_REVISION,
};
mod transport;
pub use profile::{
    GatewayCapabilities, GatewayKind, ModelProfile, OPENROUTER_DEFAULT_MODEL, OPENROUTER_ENDPOINT,
    VERCEL_DEFAULT_MODEL,
};
use transport::{MAX_REQUEST, SseDecoder};

pub const VERCEL_ENDPOINT: &str = "https://ai-gateway.vercel.sh/v1/chat/completions";
const MAX_CALLS: u64 = 128;

/// No Debug implementation: the credential is never a diagnostic value.
pub struct GatewayProvider {
    backend: Backend,
}
enum Backend {
    Chat(ChatProvider),
    Responses(OpenResponsesProvider),
}
struct ChatProvider {
    kind: GatewayKind,
    transport: transport::Transport,
}
/// Closed, body-free diagnostics for an explicitly requested reachability probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeFailure {
    Authentication,
    ModelRequest,
    Transport,
    Response,
}
impl GatewayProvider {
    /// One small request through the ordinary transport. No runtime, tools, fallback,
    /// trace exporter, or response content is executed/exposed. Success means HTTP/SSE
    /// request acceptance only; it does not attest generated output or remote cleanup.
    pub async fn probe(
        &self,
        model: &str,
        deadline: tokio::time::Instant,
    ) -> Result<(), ProbeFailure> {
        let messages = [Message::User {
            text: "Reply OK.".into(),
        }];
        let request = ModelRequest {
            model,
            input: "Reply OK.",
            instructions: "Reply briefly.",
            messages: &messages,
            continuations: &[],
            max_continuation_bytes: 1024,
            max_context_bytes: 4096,
            max_tool_input_bytes: 1024,
            max_output_bytes: 1024,
            tools: &[],
            allow_tool_calls: false,
            max_output_tokens: 16,
            deadline,
            context: opentelemetry::Context::new(),
            cancellation: crate::CancellationToken::new(),
        };
        match &self.backend {
            Backend::Chat(chat) => {
                chat.transport
                    .probe(
                        request_body(&request, chat.kind)
                            .map_err(|_| ProbeFailure::ModelRequest)?,
                        deadline,
                    )
                    .await
            }
            Backend::Responses(responses) => responses.probe(&request).await,
        }
    }
    pub fn vercel(key: &str) -> Result<Self, &'static str> {
        Self::selected(GatewayKind::Vercel, key)
    }
    pub fn selected(kind: GatewayKind, key: &str) -> Result<Self, &'static str> {
        kind.ensure_available()?;
        if kind == GatewayKind::OpenResponses {
            return Err(
                "Open Responses requires configured endpoint, model and capability profile",
            );
        }
        Ok(Self {
            backend: Backend::Chat(ChatProvider {
                kind,
                transport: transport::Transport::new(kind.endpoint(), key, false).map_err(|e| {
                    if kind == GatewayKind::Openrouter && e == "invalid AI_GATEWAY_API_KEY" {
                        "invalid OPENROUTER_API_KEY"
                    } else {
                        e
                    }
                })?,
            }),
        })
    }
    pub fn local_fixture(endpoint: &str) -> Result<Self, &'static str> {
        Self::local_fixture_for(GatewayKind::Vercel, endpoint)
    }
    /// Host-only override. No real credential argument exists on this path.
    pub fn local_fixture_for(kind: GatewayKind, endpoint: &str) -> Result<Self, &'static str> {
        kind.ensure_available()?;
        if kind == GatewayKind::OpenResponses {
            return Err("Open Responses requires a configured fixture profile");
        }
        Ok(Self {
            backend: Backend::Chat(ChatProvider {
                kind,
                transport: transport::Transport::new(endpoint, "pablo-local-fixture", true)?,
            }),
        })
    }
    pub fn configured(profile: &ModelProfile, key: &str) -> Result<Self, &'static str> {
        if let Some(responses) = &profile.open_responses {
            Ok(Self {
                backend: Backend::Responses(OpenResponsesProvider::new(responses.clone(), key)?),
            })
        } else {
            Self::selected(profile.provider, key)
        }
    }
    pub fn configured_fixture(
        profile: &ModelProfile,
        endpoint: &str,
    ) -> Result<Self, &'static str> {
        if let Some(responses) = &profile.open_responses {
            Ok(Self {
                backend: Backend::Responses(OpenResponsesProvider::local_fixture(
                    responses.clone(),
                    endpoint,
                )?),
            })
        } else {
            Self::local_fixture_for(profile.provider, endpoint)
        }
    }
}

impl Provider for GatewayProvider {
    fn accepts_history(
        &self,
        model: &str,
        messages: &[Message],
        entries: &[crate::provider::ContinuationEntry],
    ) -> bool {
        match &self.backend {
            Backend::Responses(p) => p.accepts_history(model, messages, entries),
            Backend::Chat(_) => entries.is_empty(),
        }
    }
    fn validate_model(&self, model: &str, max_output_tokens: u32) -> Result<(), &'static str> {
        match &self.backend {
            Backend::Responses(p) => p.validate_model(model, max_output_tokens),
            Backend::Chat(_) => Ok(()),
        }
    }
    fn profile_identity(&self) -> Option<crate::ProviderIdentity> {
        match &self.backend {
            Backend::Responses(p) => p.profile_identity(),
            Backend::Chat(_) => None,
        }
    }
    fn name(&self) -> &'static str {
        match &self.backend {
            Backend::Chat(p) => p.kind.name(),
            Backend::Responses(p) => p.name(),
        }
    }

    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        let chat = match &self.backend {
            Backend::Chat(p) => p,
            Backend::Responses(p) => return p.stream(request),
        };
        Box::pin(async move {
            let aliases = tool_aliases(request.tools)?;
            let body = request_body(&request, chat.kind)?;
            // The runtime selects against the absolute deadline and cancellation
            // while opening and polling this stream. Dropping it drops the socket.
            let reader = chat.transport.open(body, request.deadline).await?;
            let state = ResponseStream {
                reader,
                bytes: [0; 4096],
                cursor: 0,
                available: 0,
                decoder: SseDecoder::default(),
                completion: Completion {
                    aliases,
                    kind: chat.kind,
                    ..Completion::default()
                },
                stopped: false,
            };
            Ok(Box::pin(stream::unfold(state, |mut state| async move {
                if state.stopped {
                    return None;
                }
                let event = state.next().await;
                if event.is_err() || matches!(event, Ok(ProviderEvent::Finished { .. })) {
                    state.stopped = true;
                }
                Some((event, state))
            })) as ProviderStream<'a>)
        })
    }
}

fn wire_name(name: &str) -> Result<std::borrow::Cow<'_, str>, ProviderError> {
    match name {
        "subagent" => Ok("subagent".into()),
        "shell.run" => Ok("shell_run".into()),
        "fs.read" => Ok("fs_read".into()),
        "skill.read" => Ok("skill_read".into()),
        "fs.list" => Ok("fs_list".into()),
        "fs.search" => Ok("fs_search".into()),
        "fs.write" => Ok("fs_write".into()),
        "fs.edit" => Ok("fs_edit".into()),
        _ => crate::mcp::provider_alias(name)
            .map(Into::into)
            .map_err(|_| not_sent()),
    }
}

fn tool_aliases(
    tools: &[crate::tool::ToolDescriptor],
) -> Result<BTreeMap<String, String>, ProviderError> {
    if tools.len() > crate::mcp::MAX_TOOLS + 8 {
        return Err(not_sent());
    }
    let mut aliases = BTreeMap::new();
    for tool in tools {
        let alias = wire_name(&tool.name)?.into_owned();
        if aliases.insert(alias, tool.name.clone()).is_some() {
            return Err(not_sent());
        }
    }
    Ok(aliases)
}

fn native_name(name: &str, aliases: &BTreeMap<String, String>) -> Option<String> {
    match name {
        "shell_run" => Some("shell.run".into()),
        "fs_read" => Some("fs.read".into()),
        "skill_read" => Some("skill.read".into()),
        "fs_list" => Some("fs.list".into()),
        "fs_search" => Some("fs.search".into()),
        "fs_write" => Some("fs.write".into()),
        "fs_edit" => Some("fs.edit".into()),
        _ => aliases.get(name).cloned(),
    }
}

fn request_body(request: &ModelRequest<'_>, kind: GatewayKind) -> Result<Vec<u8>, ProviderError> {
    if !request.continuations.is_empty() {
        return Err(ProviderError {
            retry_class: None,
            code: FailureCode::UnsupportedProviderContent,
            delivery: DeliveryCertainty::NotSent,
        });
    }
    let mut messages = Vec::new();
    if !request.instructions.is_empty() {
        messages.push(json!({"role":"system", "content":request.instructions}));
    }
    for message in request.messages {
        messages.push(match message {
            Message::User { text } => json!({"role":"user", "content":text}),
            Message::Assistant { text, tool_calls } => {
                let mut value = json!({"role":"assistant", "content":text});
                if !tool_calls.is_empty() {
                    let calls: Result<Vec<_>, _> = tool_calls.iter().map(|call| Ok(json!({
                        "id":call.id, "type":"function", "function":{
                            "name":wire_name(&call.name)?, "arguments":call.arguments.to_string()
                        }
                    }))).collect();
                    value["tool_calls"] = json!(calls?);
                }
                value
            }
            Message::Tool {
                call_id, result, ..
            } => json!({
                "role":"tool", "tool_call_id":call_id,
                "content":serde_json::to_string(result).map_err(|_| not_sent())?
            }),
        });
    }
    let mut body = json!({"model":request.model, "messages":messages,
        "stream":true, "max_tokens":request.max_output_tokens});
    if kind == GatewayKind::Vercel {
        body["stream_options"] = json!({"include_usage":true});
    }
    if !request.tools.is_empty() {
        let tools: Result<Vec<_>, _> = request
            .tools
            .iter()
            .map(|tool| {
                Ok(json!({
                    "type":"function", "function":{"name":wire_name(&tool.name)?,
                        "description":tool.description, "parameters":tool.input_schema}
                }))
            })
            .collect();
        body["tools"] = json!(tools?);
        body["tool_choice"] = json!(if request.allow_tool_calls {
            "auto"
        } else {
            "none"
        });
        body["parallel_tool_calls"] = json!(false);
    }
    let body = serde_json::to_vec(&body).map_err(|_| not_sent())?;
    if body.len() > MAX_REQUEST {
        return Err(not_sent());
    }
    Ok(body)
}

fn not_sent() -> ProviderError {
    ProviderError {
        retry_class: None,
        code: FailureCode::ProviderRejected,
        delivery: DeliveryCertainty::NotSent,
    }
}
fn malformed() -> ProviderError {
    ProviderError {
        retry_class: None,
        code: FailureCode::MalformedStream,
        delivery: DeliveryCertainty::ResponseReceived,
    }
}

struct ResponseStream {
    reader: Pin<Box<dyn AsyncRead + Send>>,
    bytes: [u8; 4096],
    cursor: usize,
    available: usize,
    decoder: SseDecoder,
    completion: Completion,
    stopped: bool,
}
impl ResponseStream {
    async fn next(&mut self) -> Result<ProviderEvent, ProviderError> {
        loop {
            if let Some(event) = self.completion.pending.pop_front() {
                return Ok(event);
            }
            if self.cursor == self.available {
                self.available =
                    self.reader
                        .read(&mut self.bytes)
                        .await
                        .map_err(|_| ProviderError {
                            retry_class: None,
                            code: FailureCode::ProviderTransport,
                            delivery: DeliveryCertainty::ResponseReceived,
                        })?;
                self.cursor = 0;
                if self.available == 0 {
                    return Err(malformed());
                } // Missing [DONE].
                tokio::task::consume_budget().await;
            }
            let byte = self.bytes[self.cursor];
            self.cursor += 1;
            if let Some(frame) = self.decoder.push(byte)? {
                self.completion.frame(&frame)?;
            }
        }
    }
}

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    started: bool,
}
#[derive(Default)]
struct Completion {
    aliases: BTreeMap<String, String>,
    kind: GatewayKind,
    cost: Option<u64>,
    pending: VecDeque<ProviderEvent>,
    calls: BTreeMap<u64, Call>,
    finish: Option<FinishReason>,
    usage: Usage,
    usage_seen: bool,
}
impl Completion {
    fn frame(&mut self, data: &[u8]) -> Result<(), ProviderError> {
        if data == b"[DONE]" {
            let reason = self.finish.ok_or_else(malformed)?;
            if self.calls.values().any(|call| !call.started) {
                return Err(malformed());
            }
            if let Some(microusd) = self.cost.take() {
                self.pending.push_back(ProviderEvent::Cost { microusd });
            }
            self.pending.push_back(ProviderEvent::Finished {
                reason,
                usage: self.usage.clone(),
            });
            return Ok(());
        }
        let value: Value = serde_json::from_slice(data).map_err(|_| malformed())?;
        if value.get("error").is_some() {
            return Err(ProviderError {
                retry_class: None,
                code: if transport::context_overflow(&value) {
                    FailureCode::ContextOverflow
                } else {
                    FailureCode::ProviderRejected
                },
                delivery: DeliveryCertainty::ResponseReceived,
            });
        }
        let accounting = value.get("usage").filter(|v| !v.is_null());
        if let Some(usage) = accounting {
            if self.usage_seen || !usage.is_object() {
                return Err(malformed());
            }
            self.usage_seen = true;
            if self.kind == GatewayKind::Openrouter {
                (self.usage, self.cost) = openrouter::accounting(data)?;
            } else {
                self.usage = Usage {
                    input_tokens: counter(usage.get("prompt_tokens"))?,
                    output_tokens: counter(usage.get("completion_tokens"))?,
                    cache_read_input_tokens: counter(
                        usage.pointer("/prompt_tokens_details/cached_tokens"),
                    )?,
                    cache_write_input_tokens: None,
                };
            }
        }
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?;
        if choices.is_empty() {
            return Ok(());
        }
        if let Some(finish) = self.finish {
            if self.kind == GatewayKind::Openrouter
                && accounting.is_some()
                && choices.len() == 1
                && openrouter::accounting_choice(&choices[0], finish)
            {
                return Ok(());
            }
            return Err(malformed());
        }
        if choices.len() != 1 || choices[0]["index"] != 0 {
            return Err(malformed());
        }
        let choice = &choices[0];
        let delta = choice
            .get("delta")
            .and_then(Value::as_object)
            .ok_or_else(malformed)?;
        if delta.get("function_call").is_some()
            || delta.get("refusal").is_some_and(|v| !v.is_null())
        {
            return Err(malformed());
        }
        if let Some(text) = delta.get("content").filter(|v| !v.is_null()) {
            let text = text.as_str().ok_or_else(malformed)?;
            if !text.is_empty() {
                self.pending
                    .push_back(ProviderEvent::TextDelta(text.into()));
            }
        }
        if let Some(calls) = delta.get("tool_calls").filter(|v| !v.is_null()) {
            let calls = calls.as_array().ok_or_else(malformed)?;
            if calls.len() > MAX_CALLS as usize {
                return Err(malformed());
            }
            for call in calls {
                self.call(call)?;
            }
        }
        if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            self.finish = Some(match reason.as_str() {
                Some("stop") if self.calls.is_empty() => FinishReason::Stop,
                Some("length") => FinishReason::Length,
                Some("tool_calls") if !self.calls.is_empty() => FinishReason::ToolCalls,
                _ => return Err(malformed()),
            });
        }
        Ok(())
    }
    fn call(&mut self, value: &Value) -> Result<(), ProviderError> {
        let index = value
            .get("index")
            .and_then(Value::as_u64)
            .filter(|&i| i < MAX_CALLS)
            .ok_or_else(malformed)?;
        let nullable = self.kind == GatewayKind::Openrouter;
        if value
            .get("type")
            .filter(|v| !nullable || !v.is_null())
            .is_some_and(|kind| kind != "function")
        {
            return Err(malformed());
        }
        let call = self.calls.entry(index).or_default();
        if let Some(id) = value.get("id").filter(|v| !nullable || !v.is_null()) {
            let id = id.as_str().ok_or_else(malformed)?;
            if id.is_empty()
                || id.len() > 128
                || !call.id.is_empty()
                || id.chars().any(char::is_control)
            {
                return Err(malformed());
            }
            call.id = id.into();
        }
        let function = value
            .get("function")
            .and_then(Value::as_object)
            .ok_or_else(malformed)?;
        if let Some(name) = function.get("name").filter(|v| !nullable || !v.is_null()) {
            let name = name.as_str().ok_or_else(malformed)?;
            if call.started || call.name.len() + name.len() > 64 {
                return Err(malformed());
            }
            call.name.push_str(name);
        }
        if let Some(arguments) = function
            .get("arguments")
            .filter(|v| !nullable || !v.is_null())
        {
            let delta = arguments.as_str().ok_or_else(malformed)?;
            if !delta.is_empty() {
                if !call.started {
                    if call.id.is_empty() {
                        return Err(malformed());
                    }
                    call.started = true;
                    self.pending.push_back(ProviderEvent::ToolCallStart {
                        id: call.id.clone(),
                        name: native_name(&call.name, &self.aliases).ok_or_else(malformed)?,
                    });
                }
                self.pending
                    .push_back(ProviderEvent::ToolCallArgumentsDelta {
                        id: call.id.clone(),
                        delta: delta.into(),
                    });
            }
        }
        Ok(())
    }
}
fn counter(value: Option<&Value>) -> Result<Option<u64>, ProviderError> {
    value
        .filter(|v| !v.is_null())
        .map(|v| v.as_u64().ok_or_else(malformed))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_is_reported_once_after_finish_and_missing_fields_stay_unknown() {
        let mut completion = Completion::default();
        completion
            .frame(br#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#)
            .unwrap();
        assert!(completion.pending.is_empty());
        completion.frame(br#"{"choices":[],"usage":{"prompt_tokens":17,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":8}}}"#).unwrap();
        completion.frame(b"[DONE]").unwrap();
        match completion.pending.pop_front().unwrap() {
            ProviderEvent::Finished {
                usage,
                reason: FinishReason::Stop,
            } => {
                assert_eq!(usage.input_tokens, Some(17));
                assert_eq!(usage.output_tokens, Some(3));
                assert_eq!(usage.cache_read_input_tokens, Some(8));
                assert_eq!(usage.cache_write_input_tokens, None);
            }
            _ => panic!("expected finished"),
        }
        let mut completion = Completion::default();
        completion
            .frame(br#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#)
            .unwrap();
        completion.frame(b"[DONE]").unwrap();
        assert!(
            matches!(completion.pending.pop_front(), Some(ProviderEvent::Finished { usage, .. }) if usage == Usage::default())
        );
    }

    #[test]
    fn incomplete_duplicate_or_unsupported_provider_data_is_rejected() {
        for data in [
            "[DONE]",
            r#"{"choices":[{"index":1,"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"content_filter"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":-1}}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":99,"function":{"arguments":"{}"}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":null}]}"#,
        ] {
            assert!(
                Completion::default().frame(data.as_bytes()).is_err(),
                "{data}"
            );
        }
        let mut completion = Completion::default();
        let finish = br#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
        completion.frame(finish).unwrap();
        assert!(completion.frame(finish).is_err());
        let mut completion = Completion::default();
        let usage = br#"{"choices":[],"usage":{"prompt_tokens":1}}"#;
        completion.frame(usage).unwrap();
        assert!(completion.frame(usage).is_err());
    }

    #[test]
    fn fragmented_function_identity_maps_back_to_the_internal_name() {
        let mut completion = Completion::default();
        for function in [
            json!({"name":"shell_"}),
            json!({"name":"run","arguments":"{"}),
            json!({"arguments":"}"}),
        ] {
            let mut call = json!({"index":0,"function":function});
            if completion.calls.is_empty() {
                call["id"] = json!("call_1");
            }
            completion.frame(json!({"choices":[{"index":0,"delta":{"tool_calls":[call]},"finish_reason":null}]}).to_string().as_bytes()).unwrap();
        }
        assert!(
            matches!(completion.pending.pop_front(), Some(ProviderEvent::ToolCallStart { name, .. }) if name == "shell.run")
        );
        assert!(
            matches!(completion.pending.pop_front(), Some(ProviderEvent::ToolCallArgumentsDelta { delta, .. }) if delta == "{")
        );
    }
    #[test]
    fn mcp_aliases_round_trip_only_through_the_admitted_catalog() {
        let name = crate::mcp::qualified(&"s".repeat(32), &"t".repeat(128)).unwrap();
        let descriptor = crate::tool::ToolDescriptor {
            name: name.clone(),
            description: "synthetic".into(),
            input_schema: json!({"type":"object"}),
        };
        let aliases = tool_aliases(std::slice::from_ref(&descriptor)).unwrap();
        let alias = wire_name(&name).unwrap();
        assert_eq!(alias.len(), 52);
        assert_eq!(native_name(&alias, &aliases), Some(name.clone()));
        assert!(native_name(&alias, &BTreeMap::new()).is_none());
        assert!(tool_aliases(&[descriptor.clone(), descriptor]).is_err());
        for kind in [GatewayKind::Vercel, GatewayKind::Openrouter] {
            let mut completion = Completion {
                kind,
                aliases: aliases.clone(),
                ..Default::default()
            };
            for (i, part) in [&alias[..20], &alias[20..]].iter().enumerate() {
                let mut function = json!({"name":part});
                if i == 1 {
                    function["arguments"] = "{}".into();
                }
                let mut call = json!({"index":0,"function":function});
                if i == 0 {
                    call["id"] = "mcp-1".into();
                }
                completion.frame(json!({"choices":[{"index":0,"delta":{"tool_calls":[call]},"finish_reason":null}]}).to_string().as_bytes()).unwrap();
            }
            assert!(
                matches!(completion.pending.pop_front(),Some(ProviderEvent::ToolCallStart{name:actual,..}) if actual==name)
            );
        }
    }
}
