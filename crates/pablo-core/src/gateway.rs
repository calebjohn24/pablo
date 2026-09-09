//! Direct Vercel chat-completions transport. Credentials stay adapter-owned.
use std::{collections::BTreeMap, collections::VecDeque, io, pin::Pin, time::Duration};

use futures_util::{TryStreamExt, future::BoxFuture, stream};
use reqwest::{Client, Url, header};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::io::StreamReader;

use crate::{
    DeliveryCertainty, FailureCode, FinishReason, Message, Provider, Usage,
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
};

pub const VERCEL_ENDPOINT: &str = "https://ai-gateway.vercel.sh/v1/chat/completions";
const MAX_FRAME: usize = 32 * 1024 * 1024;
const MAX_RESPONSE: usize = 128 * 1024 * 1024;
const MAX_REQUEST: usize = 128 * 1024 * 1024;
const MAX_FRAMES: usize = 1_000_000;
const MAX_CALLS: u64 = 128;

/// No Debug implementation: the credential is never a diagnostic value.
pub struct GatewayProvider {
    client: Client,
    endpoint: Url,
    authorization: header::HeaderValue,
}

impl GatewayProvider {
    pub fn vercel(key: &str) -> Result<Self, &'static str> {
        Self::configured(VERCEL_ENDPOINT, key, false)
    }

    /// Host-only fixture override. Only literal loopback HTTP is accepted.
    /// The CLI supplies a synthetic key here, never its Vercel credential.
    pub fn local_fixture(endpoint: &str) -> Result<Self, &'static str> {
        Self::configured(endpoint, "pablo-local-fixture", true)
    }

    fn configured(endpoint: &str, key: &str, local: bool) -> Result<Self, &'static str> {
        let endpoint = Url::parse(endpoint).map_err(|_| "invalid gateway endpoint")?;
        if local
            && !(endpoint.scheme() == "http"
                && endpoint
                    .host_str()
                    .is_some_and(|host| matches!(host, "127.0.0.1" | "[::1]"))
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none())
        {
            return Err(
                "fixture endpoint must use literal loopback HTTP without credentials or query",
            );
        }
        if key.is_empty() || key.len() > 8192 || key.chars().any(char::is_whitespace) {
            return Err("invalid AI_GATEWAY_API_KEY");
        }
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| "invalid AI_GATEWAY_API_KEY")?;
        authorization.set_sensitive(true);
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| "cannot initialize gateway transport")?;
        Ok(Self {
            client,
            endpoint,
            authorization,
        })
    }
}

impl Provider for GatewayProvider {
    fn name(&self) -> &'static str {
        "vercel"
    }

    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let body = request_body(&request)?;
            // The runtime selects against the absolute deadline and cancellation
            // while opening and polling this stream. Dropping it drops the socket.
            let response = self
                .client
                .post(self.endpoint.clone())
                .header(header::AUTHORIZATION, self.authorization.clone())
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "text/event-stream")
                .timeout(
                    request
                        .deadline
                        .saturating_duration_since(tokio::time::Instant::now()),
                )
                .body(body)
                .send()
                .await
                .map_err(|error| ProviderError {
                    code: FailureCode::ProviderTransport,
                    delivery: if error.is_connect() || error.is_builder() {
                        DeliveryCertainty::NotSent
                    } else {
                        DeliveryCertainty::MayHaveBeenSent
                    },
                })?;
            if !response.status().is_success() {
                // Do not read or serialize gateway error bodies.
                return Err(ProviderError {
                    code: FailureCode::ProviderRejected,
                    delivery: DeliveryCertainty::ResponseReceived,
                });
            }
            if !response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("text/event-stream")
                })
            {
                return Err(malformed());
            }
            let reader = StreamReader::new(
                response
                    .bytes_stream()
                    .map_err(|_| io::Error::other("gateway stream failed")),
            );
            let state = ResponseStream {
                reader: Box::pin(reader),
                bytes: [0; 4096],
                cursor: 0,
                available: 0,
                decoder: SseDecoder::default(),
                completion: Completion::default(),
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

fn wire_name(name: &str) -> Result<&'static str, ProviderError> {
    match name {
        "shell.run" => Ok("shell_run"),
        "fs.read" => Ok("fs_read"),
        "fs.list" => Ok("fs_list"),
        "fs.search" => Ok("fs_search"),
        "fs.write" => Ok("fs_write"),
        "fs.edit" => Ok("fs_edit"),
        _ => Err(not_sent()),
    }
}

fn native_name(name: &str) -> Option<&'static str> {
    match name {
        "shell_run" => Some("shell.run"),
        "fs_read" => Some("fs.read"),
        "fs_list" => Some("fs.list"),
        "fs_search" => Some("fs.search"),
        "fs_write" => Some("fs.write"),
        "fs_edit" => Some("fs.edit"),
        _ => None,
    }
}

fn request_body(request: &ModelRequest<'_>) -> Result<Vec<u8>, ProviderError> {
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
        "stream":true, "stream_options":{"include_usage":true},
        "max_tokens":request.max_output_tokens});
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
        code: FailureCode::ProviderRejected,
        delivery: DeliveryCertainty::NotSent,
    }
}
fn malformed() -> ProviderError {
    ProviderError {
        code: FailureCode::MalformedStream,
        delivery: DeliveryCertainty::ResponseReceived,
    }
}

#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    frame_bytes: usize,
    total: usize,
    frames: usize,
    skip_lf: bool,
}
impl SseDecoder {
    fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>, ProviderError> {
        self.total += 1;
        self.frame_bytes += 1;
        if self.total > MAX_RESPONSE || self.frame_bytes > MAX_FRAME {
            return Err(malformed());
        }
        if self.skip_lf && byte == b'\n' {
            self.skip_lf = false;
            return Ok(None);
        }
        self.skip_lf = byte == b'\r';
        if byte != b'\n' && byte != b'\r' {
            self.line.push(byte);
            return Ok(None);
        }
        if self.line.is_empty() {
            self.frame_bytes = 0;
            self.frames += 1;
            if self.frames > MAX_FRAMES {
                return Err(malformed());
            }
            if self.data.is_empty() {
                return Ok(None);
            }
            self.data.pop(); // Last data-field newline.
            return Ok(Some(std::mem::take(&mut self.data)));
        }
        if self.line == b"data" || self.line.starts_with(b"data:") {
            let value = self.line.get(5..).unwrap_or_default();
            self.data
                .extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
            self.data.push(b'\n');
        }
        self.line.clear();
        Ok(None)
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
            self.pending.push_back(ProviderEvent::Finished {
                reason,
                usage: self.usage.clone(),
            });
            return Ok(());
        }
        let value: Value = serde_json::from_slice(data).map_err(|_| malformed())?;
        if value.get("error").is_some() {
            return Err(ProviderError {
                code: FailureCode::ProviderRejected,
                delivery: DeliveryCertainty::ResponseReceived,
            });
        }
        if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
            if self.usage_seen || !usage.is_object() {
                return Err(malformed());
            }
            self.usage_seen = true;
            self.usage = Usage {
                input_tokens: counter(usage.get("prompt_tokens"))?,
                output_tokens: counter(usage.get("completion_tokens"))?,
                cache_read_input_tokens: counter(
                    usage.pointer("/prompt_tokens_details/cached_tokens"),
                )?,
                cache_write_input_tokens: None,
            };
        }
        let choices = value
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?;
        if choices.is_empty() {
            return Ok(());
        }
        if choices.len() != 1 || choices[0]["index"] != 0 || self.finish.is_some() {
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
        if value.get("type").is_some_and(|kind| kind != "function") {
            return Err(malformed());
        }
        let call = self.calls.entry(index).or_default();
        if let Some(id) = value.get("id") {
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
        if let Some(name) = function.get("name") {
            let name = name.as_str().ok_or_else(malformed)?;
            if call.started || call.name.len() + name.len() > 64 {
                return Err(malformed());
            }
            call.name.push_str(name);
        }
        if let Some(arguments) = function.get("arguments") {
            let delta = arguments.as_str().ok_or_else(malformed)?;
            if !delta.is_empty() {
                if !call.started {
                    if call.id.is_empty() {
                        return Err(malformed());
                    }
                    call.started = true;
                    self.pending.push_back(ProviderEvent::ToolCallStart {
                        id: call.id.clone(),
                        name: native_name(&call.name).ok_or_else(malformed)?.into(),
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
    fn sse_supports_cr_lf_comments_multiline_data_and_bounds() {
        for newline in ["\n", "\r\n", "\r"] {
            let input = format!(
                ": keepalive{newline}{newline}event: message{newline}data: {{\"a\":{newline}data: 1}}{newline}{newline}"
            );
            let mut decoder = SseDecoder::default();
            let frames: Vec<_> = input
                .bytes()
                .filter_map(|b| decoder.push(b).unwrap())
                .collect();
            assert_eq!(frames, [b"{\"a\":\n1}".to_vec()]);
        }
        let mut decoder = SseDecoder::default();
        for _ in 0..MAX_FRAME {
            decoder.push(b'x').unwrap();
        }
        assert!(decoder.push(b'x').is_err());
        let mut decoder = SseDecoder::default();
        for _ in 0..MAX_FRAMES {
            decoder.push(b'\n').unwrap();
        }
        assert!(decoder.push(b'\n').is_err());
        let mut decoder = SseDecoder {
            total: MAX_RESPONSE,
            ..Default::default()
        };
        assert!(decoder.push(b'x').is_err());
    }

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
}
