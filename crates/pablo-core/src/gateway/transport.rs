//! Shared private HTTP/SSE mechanics; provider-specific mapping stays in the adapter.
use super::malformed;
use crate::provider::diagnostics::{CallDiagnostics, Phase};
use crate::{DeliveryCertainty, FailureCode, provider::ProviderError};
use futures_util::TryStreamExt;
use reqwest::{Client, Url, header};
use std::{io, pin::Pin, time::Duration};
use tokio::io::AsyncRead;
use tokio_util::io::StreamReader;
const MAX_FRAME: usize = 32 * 1024 * 1024;
const MAX_RESPONSE: usize = 128 * 1024 * 1024;
const MAX_FRAMES: usize = 1_000_000;
pub(super) const MAX_REQUEST: usize = 128 * 1024 * 1024;
pub(super) struct Transport {
    client: Client,
    endpoint: Url,
    authorization: header::HeaderValue,
    auth_header: header::HeaderName,
}
impl Transport {
    pub(super) fn new(endpoint: &str, key: &str, local: bool) -> Result<Self, &'static str> {
        Self::with_auth(endpoint, key, local, "Authorization", false)
    }
    pub(super) fn with_auth(
        endpoint: &str,
        key: &str,
        local: bool,
        auth_header: &str,
        raw: bool,
    ) -> Result<Self, &'static str> {
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
        let auth_header = header::HeaderName::from_bytes(auth_header.as_bytes())
            .map_err(|_| "invalid authentication header")?;
        let mut authorization = header::HeaderValue::from_str(&if raw {
            key.to_owned()
        } else {
            format!("Bearer {key}")
        })
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
            auth_header,
        })
    }
    fn request(&self, body: Vec<u8>, deadline: tokio::time::Instant) -> reqwest::RequestBuilder {
        self.client
            .post(self.endpoint.clone())
            .header(self.auth_header.clone(), self.authorization.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "text/event-stream")
            .timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .body(body)
    }
    pub(super) async fn probe(
        &self,
        body: Vec<u8>,
        deadline: tokio::time::Instant,
    ) -> Result<(), super::ProbeFailure> {
        let response = self
            .request(body, deadline)
            .send()
            .await
            .map_err(|_| super::ProbeFailure::Transport)?;
        match response.status().as_u16() {
            401 | 403 => Err(super::ProbeFailure::Authentication),
            400 | 404 | 422 => Err(super::ProbeFailure::ModelRequest),
            200..=299
                if response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| {
                        v.split(';')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .eq_ignore_ascii_case("text/event-stream")
                    }) =>
            {
                Ok(())
            }
            _ => Err(super::ProbeFailure::Response),
        }
    }
    pub(super) async fn open(
        &self,
        body: Vec<u8>,
        deadline: tokio::time::Instant,
        diagnostics: Option<CallDiagnostics>,
    ) -> Result<Pin<Box<dyn AsyncRead + Send>>, ProviderError> {
        if let Some(d) = &diagnostics {
            d.mark(Phase::Dispatch);
        }
        let mut response =
            self.request(body, deadline)
                .send()
                .await
                .map_err(|error| ProviderError {
                    retry_class: None,
                    code: FailureCode::ProviderTransport,
                    delivery: if error.is_connect() || error.is_builder() {
                        DeliveryCertainty::NotSent
                    } else {
                        DeliveryCertainty::MayHaveBeenSent
                    },
                })?;
        if let Some(d) = &diagnostics {
            d.mark(Phase::Headers);
            d.http(response.version());
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let mut code = FailureCode::ProviderRejected;
            // Only a small private machine-code envelope is inspected. Neither
            // its text nor transport diagnostics become public error content.
            if matches!(status, 400 | 413) {
                let mut bytes = Vec::new();
                let complete = loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) if chunk.len() <= 8192usize.saturating_sub(bytes.len()) => {
                            bytes.extend_from_slice(&chunk)
                        }
                        Ok(None) => break true,
                        _ => break false,
                    }
                };
                if complete
                    && serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .is_some_and(|v| context_overflow(&v))
                {
                    code = FailureCode::ContextOverflow;
                }
            }
            return Err(ProviderError {
                retry_class: match status {
                    429 => Some(crate::provider::RetryClass::RateLimited),
                    502..=504 => Some(crate::provider::RetryClass::ServiceUnavailable),
                    _ => None,
                },
                code,
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
                .map_ok(move |chunk| {
                    if !chunk.is_empty()
                        && let Some(d) = &diagnostics
                    {
                        d.mark(Phase::FirstData);
                    }
                    chunk
                })
                .map_err(|_| io::Error::other("gateway stream failed")),
        );
        Ok(Box::pin(reader))
    }
}
#[derive(Default)]
pub(super) struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    frame_bytes: usize,
    total: usize,
    frames: usize,
    skip_lf: bool,
    event: Option<Vec<u8>>,
    completed_event: Option<Vec<u8>>,
}
impl SseDecoder {
    pub(super) fn take_event(&mut self) -> Option<Vec<u8>> {
        self.completed_event.take()
    }
    pub(super) fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>, ProviderError> {
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
                self.event = None;
                return Ok(None);
            }
            self.completed_event = self.event.take();
            self.data.pop(); // Last data-field newline.
            return Ok(Some(std::mem::take(&mut self.data)));
        }
        if self.line == b"data" || self.line.starts_with(b"data:") {
            let value = self.line.get(5..).unwrap_or_default();
            self.data
                .extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
            self.data.push(b'\n');
        } else if self.line == b"event" || self.line.starts_with(b"event:") {
            let value = self.line.get(6..).unwrap_or_default();
            self.event = Some(value.strip_prefix(b" ").unwrap_or(value).to_vec());
        }
        self.line.clear();
        Ok(None)
    }
}

/// Closed, bounded provider error classification; never search free-form text.
pub(super) fn context_overflow(value: &serde_json::Value) -> bool {
    fn check(value: &serde_json::Value, depth: u8) -> bool {
        if ["code", "type", "error_type"].iter().any(|key| {
            matches!(
                value[*key].as_str(),
                Some("context_length_exceeded" | "context_window_exceeded")
            )
        }) {
            return true;
        }
        if depth == 0 {
            return false;
        }
        for child in [
            value.get("error"),
            value.pointer("/response/error"),
            value.get("cause"),
        ] {
            if child.is_some_and(|v| check(v, depth - 1)) {
                return true;
            }
        }
        if let Some(raw) = value
            .pointer("/metadata/raw")
            .and_then(serde_json::Value::as_str)
            .filter(|v| v.len() <= 8192)
        {
            return serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .is_some_and(|v| check(&v, depth - 1));
        }
        false
    }
    check(value, 3)
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
}
