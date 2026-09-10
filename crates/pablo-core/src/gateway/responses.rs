//! Pinned Open Responses HTTP/SSE adapter. No server state or global task cache.
use super::{malformed, native_name, not_sent, transport, wire_name};
use crate::{
    DeliveryCertainty, FailureCode, FinishReason, Message, Provider, Usage,
    provider::{
        Continuation, ContinuationScope, ModelRequest, ProviderError, ProviderEvent, ProviderStream,
    },
};
use futures_util::{future::BoxFuture, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    pin::Pin,
};
use tokio::io::{AsyncRead, AsyncReadExt};

#[cfg(test)]
mod tests;
mod unique;
pub const REVISION: &str = "92c12d96d7b61d6d15e2214daa5e9c6000ab6e1c";
pub const PROFILE: &str = "open-responses-text-tools-v1";
const MAX_STATE: usize = 32 * 1024 * 1024;
const MAX_ITEMS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenResponsesProfile {
    endpoint: String,
    model: String,
    capability_profile: String,
    auth_header: String,
    auth_scheme: String,
    revision: String,
}
impl OpenResponsesProfile {
    pub fn new(
        endpoint: &str,
        model: &str,
        profile: &str,
        header: &str,
        scheme: &str,
    ) -> Result<Self, &'static str> {
        let value = Self {
            endpoint: endpoint.into(),
            model: model.into(),
            capability_profile: profile.into(),
            auth_header: header.into(),
            auth_scheme: scheme.into(),
            revision: REVISION.into(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        let url =
            reqwest::Url::parse(&self.endpoint).map_err(|_| "invalid Open Responses endpoint")?;
        if self.endpoint.len() > 4096
            || self.endpoint.contains('\\')
            || self
                .endpoint
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "Open Responses requires an explicit HTTPS endpoint without userinfo, query or fragment",
            );
        }
        if self.model.is_empty()
            || self.model.len() > 256
            || self
                .model
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("Open Responses requires an explicit model");
        }
        if self.capability_profile != PROFILE || self.revision != REVISION {
            return Err("unsupported Open Responses capability profile");
        }
        let header = self.auth_header.to_ascii_lowercase();
        let application_auth = header.starts_with("x-")
            && !header.starts_with("x-forwarded-")
            && [
                "-api-key",
                "-auth-key",
                "-auth-token",
                "-access-token",
                "-authorization",
            ]
            .iter()
            .any(|suffix| header.ends_with(suffix));
        if self.auth_header.len() > 128
            || reqwest::header::HeaderName::from_bytes(self.auth_header.as_bytes()).is_err()
            || !(header == "authorization" || application_auth)
        {
            return Err("invalid Open Responses authentication header");
        }
        if !matches!(self.auth_scheme.as_str(), "bearer" | "raw") {
            return Err("invalid Open Responses authentication scheme");
        }
        Ok(())
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn model(&self) -> &str {
        &self.model
    }
}

pub struct OpenResponsesProvider {
    profile: OpenResponsesProfile,
    transport: transport::Transport,
}
impl OpenResponsesProvider {
    pub fn new(profile: OpenResponsesProfile, key: &str) -> Result<Self, &'static str> {
        profile.validate()?;
        let transport = transport::Transport::with_auth(
            &profile.endpoint,
            key,
            false,
            &profile.auth_header,
            profile.auth_scheme == "raw",
        )?;
        Ok(Self { profile, transport })
    }
    /// Fixture transport never accepts or loads a real credential.
    pub fn local_fixture(
        profile: OpenResponsesProfile,
        endpoint: &str,
    ) -> Result<Self, &'static str> {
        profile.validate()?;
        let transport = transport::Transport::with_auth(
            endpoint,
            "pablo-local-fixture",
            true,
            &profile.auth_header,
            profile.auth_scheme == "raw",
        )?;
        Ok(Self { profile, transport })
    }
}
impl Provider for OpenResponsesProvider {
    fn accepts_history(
        &self,
        model: &str,
        messages: &[Message],
        entries: &[crate::provider::ContinuationEntry],
    ) -> bool {
        messages.iter().enumerate().all(|(index, message)| {
            !matches!(message, Message::Assistant { .. })
                || entries.iter().any(|entry| entry.message_index == index)
        }) && entries.iter().all(|entry| {
            let scope = &entry.value.scope;
            scope.provider == "open_responses"
                && scope.endpoint == self.profile.endpoint
                && scope.requested_model == model
                && scope.revision == REVISION
                && scope.profile == PROFILE
        })
    }
    fn validate_model(&self, model: &str, max_output_tokens: u32) -> Result<(), &'static str> {
        if model != self.profile.model || max_output_tokens < 16 {
            return Err(
                "Open Responses model/profile mismatch or output allowance below 16 tokens",
            );
        }
        Ok(())
    }
    fn profile_identity(&self) -> Option<crate::ProviderIdentity> {
        Some(crate::ProviderIdentity {
            protocol: "open-responses-http-sse".into(),
            revision: REVISION.into(),
            capability_profile: PROFILE.into(),
            endpoint: self.profile.endpoint.clone(),
            requested_model: self.profile.model.clone(),
            resolved_model: None,
        })
    }
    fn name(&self) -> &'static str {
        "open_responses"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let body = request_body(&request, &self.profile)?;
            let reader = self.transport.open(body, request.deadline).await?;
            let state = ResponseStream {
                reader,
                bytes: [0; 4096],
                cursor: 0,
                available: 0,
                decoder: transport::SseDecoder::default(),
                stopped: false,
                completion: Completion::new(&request, &self.profile),
            };
            Ok(Box::pin(stream::unfold(state, |mut state| async move {
                if state.stopped {
                    return None;
                }
                let next = state.next().await;
                if next.is_err() || matches!(next, Ok(ProviderEvent::Finished { .. })) {
                    state.stopped = true;
                }
                Some((next, state))
            })) as ProviderStream<'a>)
        })
    }
}

fn unsupported() -> ProviderError {
    ProviderError {
        retry_class: None,
        code: FailureCode::UnsupportedProviderContent,
        delivery: DeliveryCertainty::ResponseReceived,
    }
}
fn require(value: bool) -> Result<(), ProviderError> {
    if value { Ok(()) } else { Err(malformed()) }
}
fn string(value: &Value) -> Result<&str, ProviderError> {
    value.as_str().ok_or_else(malformed)
}
fn array(value: &Value) -> Result<&Vec<Value>, ProviderError> {
    value.as_array().ok_or_else(malformed)
}
fn id(value: &Value, max: usize) -> Result<&str, ProviderError> {
    let value = string(value)?;
    require(!value.is_empty() && value.len() <= max && !value.chars().any(char::is_control))?;
    Ok(value)
}
fn only(value: &Value, fields: &[&str]) -> Result<(), ProviderError> {
    let map = value.as_object().ok_or_else(malformed)?;
    if map.keys().any(|k| !fields.contains(&k.as_str())) {
        return Err(unsupported());
    }
    Ok(())
}
fn request_body(
    request: &ModelRequest<'_>,
    profile: &OpenResponsesProfile,
) -> Result<Vec<u8>, ProviderError> {
    if request.model != profile.model || request.max_output_tokens < 16 {
        return Err(not_sent());
    }
    let mut input = Vec::new();
    for (index, message) in request.messages.iter().enumerate() {
        match message {
            Message::User { text } => {
                if text.len() > 10 * 1024 * 1024 {
                    return Err(not_sent());
                }
                input.push(json!({"type":"message","role":"user","content":[{"type":"input_text","text":text}]}));
            }
            Message::Assistant { .. } => {
                let entry = request
                    .continuations
                    .iter()
                    .find(|entry| entry.message_index == index)
                    .ok_or_else(not_sent)?;
                let scope = &entry.value.scope;
                if scope.provider != "open_responses"
                    || scope.endpoint != profile.endpoint
                    || scope.requested_model != request.model
                    || scope.revision != REVISION
                    || scope.profile != PROFILE
                {
                    return Err(ProviderError {
                        delivery: DeliveryCertainty::NotSent,
                        ..unsupported()
                    });
                }
                input.extend(entry.value.items.iter().cloned());
            }
            Message::Tool {
                call_id, result, ..
            } => {
                let output = serde_json::to_string(result).map_err(|_| not_sent())?;
                if output.len() > 10 * 1024 * 1024 {
                    return Err(not_sent());
                }
                input
                    .push(json!({"type":"function_call_output","call_id":call_id,"output":output}));
            }
        }
    }
    let tools = request
        .tools
        .iter()
        .map(|tool| {
            Ok(json!({"type":"function","name":wire_name(&tool.name)?,
        "description":tool.description,"parameters":tool.input_schema,"strict":false}))
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;
    let body = serde_json::to_vec(&json!({"model":request.model,"instructions":request.instructions,"input":input,
        "tools":tools,"tool_choice":if request.allow_tool_calls {"auto"} else {"none"},"parallel_tool_calls":false,
        "stream":true,"store":false,"background":false,"truncation":"disabled","include":["reasoning.encrypted_content"],
        "max_output_tokens":request.max_output_tokens})).map_err(|_| not_sent())?;
    if body.len() > transport::MAX_REQUEST || body.len() > request.max_context_bytes {
        return Err(not_sent());
    }
    Ok(body)
}

struct ResponseStream {
    reader: Pin<Box<dyn AsyncRead + Send>>,
    bytes: [u8; 4096],
    cursor: usize,
    available: usize,
    decoder: transport::SseDecoder,
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
                }
                tokio::task::consume_budget().await;
            }
            let byte = self.bytes[self.cursor];
            self.cursor += 1;
            if let Some(frame) = self.decoder.push(byte)? {
                self.completion
                    .frame(self.decoder.take_event().as_deref(), &frame)?;
            }
        }
    }
}

#[derive(Default)]
struct Part {
    text: String,
    text_done: bool,
    closed: bool,
    final_value: Option<Value>,
}
struct Item {
    value: Value,
    parts: Vec<Part>,
    arguments: String,
    arguments_done: bool,
    final_value: Option<Value>,
}
struct Completion {
    scope: ContinuationScope,
    max_state: usize,
    retained: usize,
    max_arguments: usize,
    max_output: usize,
    output_bytes: usize,
    units: usize,
    calls: usize,
    sequence: Option<u64>,
    response_id: Option<String>,
    explicit_progress: bool,
    saw_items: bool,
    items: Vec<Item>,
    ids: HashSet<String>,
    call_ids: HashSet<String>,
    expected_resolved: Option<String>,
    terminal: Option<(FinishReason, Usage)>,
    pending: VecDeque<ProviderEvent>,
}
impl Completion {
    fn new(request: &ModelRequest<'_>, profile: &OpenResponsesProfile) -> Self {
        Self {
            scope: ContinuationScope {
                provider: "open_responses",
                endpoint: profile.endpoint.clone(),
                requested_model: request.model.into(),
                resolved_model: String::new(),
                revision: REVISION,
                profile: PROFILE,
            },
            max_state: request.max_continuation_bytes.min(MAX_STATE),
            retained: 0,
            max_arguments: request.max_tool_input_bytes,
            max_output: request.max_output_bytes,
            output_bytes: 0,
            units: 0,
            calls: 0,
            sequence: None,
            response_id: None,
            explicit_progress: false,
            saw_items: false,
            items: Vec::new(),
            ids: HashSet::new(),
            call_ids: HashSet::new(),
            expected_resolved: request
                .continuations
                .last()
                .map(|e| e.value.scope.resolved_model.clone()),
            terminal: None,
            pending: VecDeque::new(),
        }
    }
    fn reserve(&mut self, count: usize) -> Result<(), ProviderError> {
        require(count <= self.max_state.saturating_sub(self.retained))?;
        self.retained += count;
        Ok(())
    }
    fn unit(&mut self) -> Result<(), ProviderError> {
        self.units += 1;
        require(self.units <= MAX_ITEMS)
    }
    fn frame(&mut self, event: Option<&[u8]>, data: &[u8]) -> Result<(), ProviderError> {
        if data == b"[DONE]" {
            require(event.is_none())?;
            let (reason, usage) = self.terminal.take().ok_or_else(malformed)?;
            if reason != FinishReason::Length {
                let mut items = Vec::with_capacity(self.items.len());
                for item in &mut self.items {
                    let mut value = item.final_value.take().ok_or_else(malformed)?;
                    if value["type"] == "reasoning" {
                        value.as_object_mut().unwrap().remove("content");
                    }
                    items.push(value);
                }
                let continuation = Continuation::new(self.scope.clone(), items, self.max_state)
                    .ok_or_else(malformed)?;
                self.pending
                    .push_back(ProviderEvent::Continuation(continuation));
            }
            self.pending
                .push_back(ProviderEvent::Finished { reason, usage });
            return Ok(());
        }
        require(self.terminal.is_none())?;
        let value = unique::parse(data)?;
        let kind = string(&value["type"])?;
        if let Some(obfuscation) = value.get("obfuscation") {
            string(obfuscation)?;
        }
        require(event == Some(kind.as_bytes()))?;
        let sequence = value["sequence_number"].as_u64().ok_or_else(malformed)?;
        require(self.sequence.is_none_or(|n| sequence > n))?;
        self.sequence = Some(sequence);
        if self.response_id.is_none() && kind != "response.created" {
            return Err(malformed());
        }
        match kind {
            "response.created" | "response.in_progress" => {
                only(&value, &["type", "sequence_number", "response"])?;
                let response = &value["response"];
                self.response(response)?;
                require(
                    response["status"] == "in_progress" && array(&response["output"])?.is_empty(),
                )?;
                if kind == "response.created" {
                    require(self.response_id.is_none())?;
                    self.response_id = Some(id(&response["id"], 256)?.into());
                    let resolved = id(&response["model"], 256)?;
                    require(
                        self.expected_resolved
                            .as_deref()
                            .is_none_or(|m| m == resolved),
                    )?;
                    self.scope.resolved_model = resolved.into();
                    self.pending
                        .push_back(ProviderEvent::ResolvedModel(resolved.into()));
                } else {
                    require(!self.explicit_progress && !self.saw_items)?;
                    self.explicit_progress = true;
                }
            }
            "response.output_item.added" => self.add_item(&value)?,
            "response.output_item.done" => self.end_item(&value)?,
            "response.content_part.added" | "response.reasoning_summary_part.added" => {
                self.add_part(&value)?
            }
            "response.output_text.delta"
            | "response.output_text.done"
            | "response.content_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => self.part(&value)?,
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                self.arguments(&value)?
            }
            "response.completed" | "response.incomplete" => {
                only(&value, &["type", "sequence_number", "response"])?;
                let response = &value["response"];
                self.response(response)?;
                let incomplete = kind == "response.incomplete";
                require(
                    response["status"]
                        == if incomplete {
                            "incomplete"
                        } else {
                            "completed"
                        },
                )?;
                require(response["error"].is_null())?;
                if incomplete {
                    if response["incomplete_details"]["reason"] != "max_output_tokens" {
                        return Err(unsupported());
                    }
                } else {
                    require(response["incomplete_details"].is_null())?;
                }
                let output = array(&response["output"])?;
                require(output.len() == self.items.len())?;
                for (index, (expected, item)) in output.iter().zip(&self.items).enumerate() {
                    require(item.final_value.as_ref() == Some(expected))?;
                    if expected.get("status").is_some_and(|s| s == "incomplete") {
                        require(incomplete && index + 1 == output.len())?;
                    }
                }
                let usage = usage(&response["usage"])?;
                let reason = if incomplete {
                    FinishReason::Length
                } else if self.calls > 0 {
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                };
                self.terminal = Some((reason, usage));
            }
            "error" | "response.failed" => {
                return Err(ProviderError {
                    retry_class: None,
                    code: if super::transport::context_overflow(&value) {
                        FailureCode::ContextOverflow
                    } else {
                        FailureCode::ProviderRejected
                    },
                    delivery: DeliveryCertainty::ResponseReceived,
                });
            }
            _ => return Err(unsupported()),
        }
        Ok(())
    }
    fn response(&self, response: &Value) -> Result<(), ProviderError> {
        const FIELDS: &[&str] = &[
            "id",
            "object",
            "created_at",
            "completed_at",
            "status",
            "incomplete_details",
            "model",
            "previous_response_id",
            "instructions",
            "output",
            "error",
            "tools",
            "tool_choice",
            "truncation",
            "parallel_tool_calls",
            "text",
            "top_p",
            "presence_penalty",
            "frequency_penalty",
            "top_logprobs",
            "temperature",
            "reasoning",
            "usage",
            "max_output_tokens",
            "max_tool_calls",
            "store",
            "background",
            "service_tier",
            "metadata",
            "safety_identifier",
            "prompt_cache_key",
        ];
        only(response, FIELDS)?;
        require(FIELDS.iter().all(|key| response.get(key).is_some()))?;
        require(response["created_at"].is_u64())?;
        for key in ["completed_at", "max_output_tokens", "max_tool_calls"] {
            require(response[key].is_null() || response[key].is_u64())?;
        }
        for key in [
            "instructions",
            "previous_response_id",
            "safety_identifier",
            "prompt_cache_key",
        ] {
            require(response[key].is_null() || response[key].is_string())?;
        }
        for key in [
            "top_p",
            "presence_penalty",
            "frequency_penalty",
            "temperature",
        ] {
            require(response[key].is_number())?;
        }
        require(response["top_logprobs"].is_u64() && response["service_tier"].is_string())?;
        if !response["incomplete_details"].is_null() {
            only(&response["incomplete_details"], &["reason"])?;
            string(&response["incomplete_details"]["reason"])?;
        }
        if !response["error"].is_null() {
            only(&response["error"], &["code", "message"])?;
            string(&response["error"]["code"])?;
            string(&response["error"]["message"])?;
        }
        if !response["reasoning"].is_null() {
            let reasoning = &response["reasoning"];
            only(reasoning, &["effort", "summary"])?;
            for key in ["effort", "summary"] {
                require(
                    reasoning.get(key).is_some()
                        && (reasoning[key].is_null() || reasoning[key].is_string()),
                )?;
            }
        }
        only(&response["text"], &["format", "verbosity"])?;
        only(&response["text"]["format"], &["type"])?;
        if let Some(verbosity) = response["text"].get("verbosity") {
            require(matches!(
                verbosity.as_str(),
                Some("low" | "medium" | "high")
            ))?;
        }
        for tool in array(&response["tools"])? {
            only(
                tool,
                &["type", "name", "description", "parameters", "strict"],
            )?;
            if tool["type"] != "function" {
                return Err(unsupported());
            }
            id(&tool["name"], 64)?;
            for key in ["description", "parameters", "strict"] {
                require(tool.get(key).is_some())?;
            }
            require(tool["description"].is_null() || tool["description"].is_string())?;
            require(tool["parameters"].is_null() || tool["parameters"].is_object())?;
            require(tool["strict"].is_null() || tool["strict"].is_boolean())?;
        }
        if !matches!(response["tool_choice"].as_str(), Some("auto" | "none")) {
            return Err(unsupported());
        }
        usage(&response["usage"])?;
        require(response["object"] == "response")?;
        let response_id = id(&response["id"], 256)?;
        require(self.response_id.as_deref().is_none_or(|s| s == response_id))?;
        id(&response["model"], 256)?;
        if self.response_id.is_some() {
            require(response["model"] == self.scope.resolved_model)?;
        }
        require(
            response["background"] == false
                && response["store"] == false
                && response["parallel_tool_calls"] == false,
        )?;
        require(
            response["previous_response_id"].is_null() && response["truncation"] == "disabled",
        )?;
        if response["text"]["format"]["type"] != "text" {
            return Err(unsupported());
        }
        Ok(())
    }
    fn index(&self, event: &Value) -> Result<usize, ProviderError> {
        let index = event["output_index"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(malformed)?;
        let item = self.items.get(index).ok_or_else(malformed)?;
        require(item.final_value.is_none())?;
        if event["type"] != "response.output_item.done" {
            require(event.get("item_id") == item.value.get("id"))?;
        }
        Ok(index)
    }
    fn add_item(&mut self, event: &Value) -> Result<(), ProviderError> {
        only(event, &["type", "sequence_number", "output_index", "item"])?;
        require(event["output_index"].as_u64() == Some(self.items.len() as u64))?;
        self.unit()?;
        let value = &event["item"];
        let item_id = id(&value["id"], 256)?;
        require(self.ids.insert(item_id.into()))?;
        match string(&value["type"])? {
            "message" => {
                only(value, &["type", "id", "status", "role", "content", "phase"])?;
                require(
                    value["role"] == "assistant"
                        && value["status"] == "in_progress"
                        && array(&value["content"])?.is_empty(),
                )?;
                if let Some(phase) = value.get("phase")
                    && phase != "commentary"
                    && phase != "final_answer"
                {
                    return Err(unsupported());
                }
            }
            "function_call" => {
                only(
                    value,
                    &["type", "id", "status", "name", "call_id", "arguments"],
                )?;
                require(value["status"] == "in_progress" && value["arguments"] == "")?;
                let call_id = id(&value["call_id"], 64)?;
                require(self.call_ids.insert(call_id.into()))?;
                let name = native_name(id(&value["name"], 64)?).ok_or_else(unsupported)?;
                self.calls += 1;
                require(self.calls <= 128)?;
                self.pending.push_back(ProviderEvent::ToolCallStart {
                    id: call_id.into(),
                    name: name.into(),
                });
            }
            "reasoning" => {
                only(
                    value,
                    &[
                        "type",
                        "id",
                        "summary",
                        "content",
                        "encrypted_content",
                        "status",
                    ],
                )?;
                require(array(&value["summary"])?.is_empty())?;
                private_content(value)?;
            }
            _ => return Err(unsupported()),
        }
        self.reserve(serde_json::to_vec(value).map_err(|_| malformed())?.len())?;
        self.items.push(Item {
            value: value.clone(),
            parts: Vec::new(),
            arguments: String::new(),
            arguments_done: false,
            final_value: None,
        });
        self.saw_items = true;
        Ok(())
    }
    fn add_part(&mut self, event: &Value) -> Result<(), ProviderError> {
        let summary = event["type"] == "response.reasoning_summary_part.added";
        let field = if summary {
            "summary_index"
        } else {
            "content_index"
        };
        only(
            event,
            &[
                "type",
                "sequence_number",
                "output_index",
                "item_id",
                field,
                "part",
            ],
        )?;
        let index = self.index(event)?;
        self.unit()?;
        let part = &event["part"];
        text_part(part, summary)?;
        require(part["text"] == "")?;
        self.reserve(serde_json::to_vec(part).map_err(|_| malformed())?.len())?;
        let item = &mut self.items[index];
        require(item.value["type"] == if summary { "reasoning" } else { "message" })?;
        require(event[field].as_u64() == Some(item.parts.len() as u64))?;
        item.parts.push(Part::default());
        Ok(())
    }
    fn part(&mut self, event: &Value) -> Result<(), ProviderError> {
        let kind = string(&event["type"])?;
        let summary = kind.starts_with("response.reasoning_summary");
        let field = if summary {
            "summary_index"
        } else {
            "content_index"
        };
        let index = self.index(event)?;
        let pi = event[field]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(malformed)?;
        let delta = kind.ends_with(".delta");
        let close = kind.ends_with("part.done");
        let payload = if delta {
            "delta"
        } else if close {
            "part"
        } else {
            "text"
        };
        only(
            event,
            &[
                "type",
                "sequence_number",
                "output_index",
                "item_id",
                field,
                payload,
                "obfuscation",
                "logprobs",
            ],
        )?;
        if let Some(logprobs) = event.get("logprobs")
            && !array(logprobs)?.is_empty()
        {
            return Err(unsupported());
        }
        if delta {
            let text = string(&event["delta"])?;
            self.reserve(text.len())?;
            if !summary {
                require(text.len() <= self.max_output.saturating_sub(self.output_bytes))?;
                self.output_bytes += text.len();
            }
        }
        let item = &mut self.items[index];
        require(item.value["type"] == if summary { "reasoning" } else { "message" })?;
        let part = item.parts.get_mut(pi).ok_or_else(malformed)?;
        require(!part.closed)?;
        if delta {
            require(!part.text_done)?;
            let text = string(&event["delta"])?;
            part.text.push_str(text);
            if !summary {
                self.pending
                    .push_back(ProviderEvent::TextDelta(text.into()));
            }
        } else if close {
            require(part.text_done)?;
            text_part(&event["part"], summary)?;
            require(event["part"]["text"] == part.text)?;
            part.closed = true;
            part.final_value = Some(event["part"].clone());
        } else {
            require(!part.text_done && event["text"] == part.text)?;
            part.text_done = true;
        }
        Ok(())
    }
    fn arguments(&mut self, event: &Value) -> Result<(), ProviderError> {
        let delta = event["type"] == "response.function_call_arguments.delta";
        let field = if delta { "delta" } else { "arguments" };
        only(
            event,
            &[
                "type",
                "sequence_number",
                "output_index",
                "item_id",
                field,
                "obfuscation",
            ],
        )?;
        let index = self.index(event)?;
        let text = string(&event[field])?;
        if delta {
            self.reserve(text.len())?;
        }
        let item = &mut self.items[index];
        require(item.value["type"] == "function_call" && !item.arguments_done)?;
        if delta {
            require(text.len() <= self.max_arguments.saturating_sub(item.arguments.len()))?;
            item.arguments.push_str(text);
            self.pending
                .push_back(ProviderEvent::ToolCallArgumentsDelta {
                    id: string(&item.value["call_id"])?.into(),
                    delta: text.into(),
                });
        } else {
            require(text == item.arguments)?;
            item.arguments_done = true;
        }
        Ok(())
    }
    fn end_item(&mut self, event: &Value) -> Result<(), ProviderError> {
        only(event, &["type", "sequence_number", "output_index", "item"])?;
        let index = self.index(event)?;
        let value = &event["item"];
        let item = &self.items[index];
        require(value["id"] == item.value["id"] && value["type"] == item.value["type"])?;
        let mut expected = item.value.clone();
        match string(&value["type"])? {
            "message" => {
                require(item.parts.iter().all(|p| p.closed))?;
                expected["content"] = Value::Array(
                    item.parts
                        .iter()
                        .map(|p| p.final_value.clone().unwrap())
                        .collect(),
                );
                require(value["status"] == "completed" || value["status"] == "incomplete")?;
                expected["status"] = value["status"].clone();
            }
            "function_call" => {
                require(item.arguments_done)?;
                expected["arguments"] = item.arguments.clone().into();
                require(value["status"] == "completed" || value["status"] == "incomplete")?;
                expected["status"] = value["status"].clone();
                if value["status"] == "completed" {
                    require(unique::parse(item.arguments.as_bytes())?.is_object())?;
                }
            }
            "reasoning" => {
                private_content(value)?;
                require(item.parts.iter().all(|p| p.closed))?;
                expected["summary"] = Value::Array(
                    item.parts
                        .iter()
                        .map(|p| p.final_value.clone().unwrap())
                        .collect(),
                );
                if let Some(encrypted) = value.get("encrypted_content") {
                    let text = string(encrypted)?;
                    self.reserve(text.len())?;
                    expected["encrypted_content"] = encrypted.clone();
                }
                if let Some(content) = value.get("content") {
                    expected["content"] = content.clone();
                }
                if let Some(status) = value.get("status") {
                    require(status == "completed")?;
                    expected["status"] = status.clone();
                }
            }
            _ => return Err(unsupported()),
        }
        require(value == &expected)?;
        self.items[index].final_value = Some(value.clone());
        Ok(())
    }
}
fn private_content(value: &Value) -> Result<(), ProviderError> {
    if let Some(content) = value.get("content")
        && !content.is_null()
        && !array(content)?.is_empty()
    {
        return Err(unsupported());
    }
    Ok(())
}
fn text_part(value: &Value, summary: bool) -> Result<(), ProviderError> {
    only(
        value,
        if summary {
            &["type", "text"]
        } else {
            &["type", "text", "annotations", "logprobs"]
        },
    )?;
    if value["type"]
        != if summary {
            "summary_text"
        } else {
            "output_text"
        }
    {
        return Err(unsupported());
    }
    string(&value["text"])?;
    if !summary && !array(&value["annotations"])?.is_empty() {
        return Err(unsupported());
    }
    if let Some(logprobs) = value.get("logprobs")
        && !array(logprobs)?.is_empty()
    {
        return Err(unsupported());
    }
    Ok(())
}
fn usage(value: &Value) -> Result<Usage, ProviderError> {
    if value.is_null() {
        return Ok(Usage::default());
    }
    only(
        value,
        &[
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "input_tokens_details",
            "output_tokens_details",
        ],
    )?;
    only(&value["input_tokens_details"], &["cached_tokens"])?;
    only(&value["output_tokens_details"], &["reasoning_tokens"])?;
    let count = |v: &Value| v.as_u64().ok_or_else(malformed);
    let input = count(&value["input_tokens"])?;
    let output = count(&value["output_tokens"])?;
    let cached = count(&value["input_tokens_details"]["cached_tokens"])?;
    let reasoning = count(&value["output_tokens_details"]["reasoning_tokens"])?;
    require(
        input.checked_add(output) == Some(count(&value["total_tokens"])?)
            && cached <= input
            && reasoning <= output,
    )?;
    Ok(Usage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_input_tokens: Some(cached),
        cache_write_input_tokens: None,
    })
}
