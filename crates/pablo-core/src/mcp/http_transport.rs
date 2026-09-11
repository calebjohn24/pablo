//! Bounded POST-only Streamable HTTP around official SDK correlation.
use super::{MAX_FRAME_BYTES, MAX_RESULT_BYTES, PROTOCOL_VERSION, transport::Bounds};
use futures_util::TryStreamExt;
use reqwest::{Client, Url, header};
use rmcp::{
    RoleClient,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use serde_json::Value;
use std::{
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, BufReader},
    sync::mpsc,
    time::{Instant, timeout_at},
};
use tokio_util::{io::StreamReader, sync::CancellationToken};

const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub(super) struct HttpError;
impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP HTTP transport failed")
    }
}
impl std::error::Error for HttpError {}

pub(super) struct HttpState {
    client: Client,
    endpoint: Url,
    headers: header::HeaderMap,
    session: Mutex<Option<header::HeaderValue>>,
    initialized: AtomicBool,
    /// A dispatched call with no correlated result may still be executing remotely.
    pub uncertain: AtomicBool,
    pub cancellation: CancellationToken,
    pub bounds: Arc<Bounds>,
}
impl HttpState {
    pub async fn cancel_request(&self, id: &rmcp::model::RequestId, deadline: Instant) {
        // Send directly: the SDK sender may still be occupied by the interrupted POST.
        let _ = timeout_at(deadline, self.request(reqwest::Method::POST)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&serde_json::json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}}))
            .send()).await;
    }
    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, self.endpoint.clone())
            .headers(self.headers.clone());
        if self.initialized.load(Ordering::Acquire) {
            request = request.header("mcp-protocol-version", PROTOCOL_VERSION);
        }
        if let Some(session) = self.session.lock().unwrap().as_ref() {
            request = request.header("mcp-session-id", session.clone());
        }
        request
    }
    pub async fn teardown(&self, deadline: Instant) -> Result<(), HttpError> {
        self.cancellation.cancel();
        if self.session.lock().unwrap().is_none() {
            return Ok(());
        }
        let response = timeout_at(deadline, self.request(reqwest::Method::DELETE).send())
            .await
            .map_err(|_| HttpError)?
            .map_err(|_| HttpError)?;
        // 404 means already expired; 405 is explicitly permitted by the protocol.
        match response.status().as_u16() {
            200 | 202 | 204 | 404 | 405 => Ok(()),
            _ => Err(HttpError),
        }
    }
    pub fn address(&self) -> &str {
        self.endpoint.host_str().unwrap()
    }
    pub fn port(&self) -> u16 {
        self.endpoint.port_or_known_default().unwrap()
    }
}

pub(super) struct HttpTransport {
    pub state: Arc<HttpState>,
    send: mpsc::Sender<WireReader>,
    receive: mpsc::Receiver<WireReader>,
    active: Option<WireReader>,
}
impl HttpTransport {
    pub fn new(
        endpoint: &str,
        headers: header::HeaderMap,
        fixture: bool,
    ) -> Result<Self, HttpError> {
        let endpoint = Url::parse(endpoint).map_err(|_| HttpError)?;
        let scheme_allowed = if fixture {
            endpoint.scheme() == "http"
                && matches!(endpoint.host_str(), Some("127.0.0.1" | "[::1]"))
        } else {
            endpoint.scheme() == "https" && endpoint.host_str().is_some()
        };
        if !scheme_allowed
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(HttpError);
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| HttpError)?;
        let (send, receive) = mpsc::channel(1);
        Ok(Self {
            state: Arc::new(HttpState {
                client,
                endpoint,
                headers,
                session: Mutex::new(None),
                initialized: AtomicBool::new(false),
                uncertain: AtomicBool::new(false),
                cancellation: CancellationToken::new(),
                bounds: Arc::default(),
            }),
            send,
            receive,
            active: None,
        })
    }
}
impl Transport<RoleClient> for HttpTransport {
    type Error = HttpError;
    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), HttpError>> + Send + 'static {
        let state = self.state.clone();
        let send = self.send.clone();
        async move {
            let operation = async {
                if !crate::filesystem::fits(&item, MAX_RESULT_BYTES) {
                    return Err(HttpError);
                }
                let body = serde_json::to_vec(&item).map_err(|_| HttpError)?;
                let value: Value = serde_json::from_slice(&body).map_err(|_| HttpError)?;
                let request = value.get("id").is_some();
                let initialize = value["method"] == "initialize";
                let call = value["method"] == "tools/call";
                if request {
                    state.bounds.reset();
                }
                if call {
                    state.uncertain.store(true, Ordering::Release);
                }
                let response = state
                    .request(reqwest::Method::POST)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .body(body)
                    .send()
                    .await
                    .map_err(|error| {
                        if call && (error.is_connect() || error.is_builder()) {
                            state.uncertain.store(false, Ordering::Release);
                        }
                        HttpError
                    })?;
                if !request {
                    return if response.status().as_u16() == 202 {
                        Ok(())
                    } else {
                        Err(HttpError)
                    };
                }
                if response.status().as_u16() != 200 {
                    return Err(HttpError);
                }
                if let Some(session) = response.headers().get("mcp-session-id") {
                    if session.as_bytes().is_empty()
                        || session.as_bytes().len() > 256
                        || !session.as_bytes().iter().all(|b| (0x21..=0x7e).contains(b))
                    {
                        return Err(HttpError);
                    }
                    let mut previous = state.session.lock().unwrap();
                    if initialize {
                        let mut session = session.clone();
                        session.set_sensitive(true);
                        *previous = Some(session);
                    } else if previous.as_ref() != Some(session) {
                        return Err(HttpError);
                    }
                }
                let sse = match response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(';').next())
                    .map(str::trim)
                {
                    Some(v) if v.eq_ignore_ascii_case("application/json") => false,
                    Some(v) if v.eq_ignore_ascii_case("text/event-stream") => true,
                    _ => return Err(HttpError),
                };
                if response
                    .content_length()
                    .is_some_and(|n| n > MAX_STREAM_BYTES as u64)
                {
                    return Err(HttpError);
                }
                if initialize {
                    state.initialized.store(true, Ordering::Release);
                }
                let reader = StreamReader::new(
                    response
                        .bytes_stream()
                        .map_err(|_| io::Error::other("MCP response stream failed")),
                );
                send.send(WireReader::new(
                    Box::pin(reader),
                    sse,
                    value["id"].clone(),
                    call,
                ))
                .await
                .map_err(|_| HttpError)
            };
            let result = tokio::select! { biased; _=state.cancellation.cancelled()=>Err(HttpError), result=operation=>result };
            if result.is_err() {
                state.bounds.fail();
            }
            result
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        let result = async {
            if self.active.is_none() {
                self.active = self.receive.recv().await;
            }
            let reader = self.active.as_mut().ok_or(HttpError)?;
            let bytes = reader.next().await?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| HttpError)?;
            if !self.state.bounds.admit(&value, bytes.len()) {
                return Err(HttpError);
            }
            let terminal = value.get("id").is_some();
            if terminal && value["id"] != reader.id {
                return Err(HttpError);
            }
            let message = serde_json::from_value(value).map_err(|_| HttpError)?;
            if terminal {
                if reader.call {
                    self.state.uncertain.store(false, Ordering::Release);
                }
                // A correlated response completes this POST. No reconnect or replay.
                self.active = None;
            }
            Ok(message)
        };
        tokio::select! {
            biased;
            _=self.state.cancellation.cancelled()=>None,
            result=result=>match result { Ok(message)=>Some(message), Err(_)=>{self.state.bounds.fail();None} }
        }
    }
    async fn close(&mut self) -> Result<(), HttpError> {
        self.state.cancellation.cancel();
        self.active = None;
        self.receive.close();
        while self.receive.try_recv().is_ok() {}
        Ok(())
    }
}

struct WireReader {
    reader: BufReader<Pin<Box<dyn AsyncRead + Send>>>,
    sse: bool,
    id: Value,
    call: bool,
    total: usize,
    json: Vec<u8>,
    decoder: Sse,
}
impl WireReader {
    fn new(reader: Pin<Box<dyn AsyncRead + Send>>, sse: bool, id: Value, call: bool) -> Self {
        Self {
            reader: BufReader::new(reader),
            sse,
            id,
            call,
            total: 0,
            json: Vec::new(),
            decoder: Sse::default(),
        }
    }
    async fn next(&mut self) -> Result<Vec<u8>, HttpError> {
        loop {
            let byte = match self.reader.read_u8().await {
                Ok(byte) => byte,
                Err(error)
                    if error.kind() == io::ErrorKind::UnexpectedEof
                        && !self.sse
                        && !self.json.is_empty() =>
                {
                    return Ok(std::mem::take(&mut self.json));
                }
                Err(_) => return Err(HttpError),
            };
            self.total += 1;
            if self.total > MAX_STREAM_BYTES {
                return Err(HttpError);
            }
            if self.sse {
                if let Some(frame) = self.decoder.push(byte)? {
                    return Ok(frame);
                }
            } else {
                if self.json.len() >= MAX_FRAME_BYTES {
                    return Err(HttpError);
                }
                self.json.push(byte);
            }
        }
    }
}

#[derive(Default)]
struct Sse {
    line: Vec<u8>,
    data: Vec<u8>,
    frame: usize,
    skip_lf: bool,
    event: Option<Vec<u8>>,
}
impl Sse {
    fn push(&mut self, byte: u8) -> Result<Option<Vec<u8>>, HttpError> {
        self.frame += 1;
        if self.frame > MAX_FRAME_BYTES {
            return Err(HttpError);
        }
        if self.skip_lf && byte == b'\n' {
            self.skip_lf = false;
            return Ok(None);
        }
        self.skip_lf = byte == b'\r';
        if !matches!(byte, b'\n' | b'\r') {
            self.line.push(byte);
            return Ok(None);
        }
        if self.line.is_empty() {
            self.frame = 0;
            let event = self.event.take();
            if event.as_ref().is_some_and(|event| event != b"message") {
                return Err(HttpError);
            }
            if self.data.last() == Some(&b'\n') {
                self.data.pop();
            }
            return Ok((!self.data.is_empty()).then(|| std::mem::take(&mut self.data)));
        }
        let line = std::mem::take(&mut self.line);
        if line.starts_with(b":") {
            return Ok(None);
        }
        let split = line.iter().position(|b| *b == b':').unwrap_or(line.len());
        let value = line.get(split + 1..).unwrap_or_default();
        let value = value.strip_prefix(b" ").unwrap_or(value);
        match &line[..split] {
            b"data" => {
                self.data.extend_from_slice(value);
                self.data.push(b'\n');
            }
            b"event" => {
                if value.len() > 64 {
                    return Err(HttpError);
                }
                self.event = Some(value.to_vec());
            }
            b"id" => {
                if value.len() > 1024 || value.contains(&0) {
                    return Err(HttpError);
                }
            }
            b"retry" if value.len() > 20 || !value.iter().all(u8::is_ascii_digit) => {
                return Err(HttpError);
            }
            _ => {}
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn json_and_sse_framing_are_finite_and_do_not_accept_partial_results() {
        let data = br#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        for sse in [false, true] {
            let bytes = if sse {
                format!(
                    "id: priming\r\ndata:\r\n\r\n: heartbeat\n\nevent: message\ndata: {}\n\n",
                    std::str::from_utf8(data).unwrap()
                )
                .into_bytes()
            } else {
                data.to_vec()
            };
            let mut reader =
                WireReader::new(Box::pin(std::io::Cursor::new(bytes)), sse, 1.into(), false);
            assert_eq!(reader.next().await.unwrap(), data);
            assert!(reader.next().await.is_err());
        }
        for bytes in [
            b"data: {}".to_vec(),
            vec![b'x'; MAX_FRAME_BYTES + 1],
            b"event: endpoint\ndata: {}\n\n".to_vec(),
        ] {
            let mut reader =
                WireReader::new(Box::pin(std::io::Cursor::new(bytes)), true, 1.into(), false);
            assert!(reader.next().await.is_err());
        }
        let mut reader = WireReader::new(
            Box::pin(std::io::Cursor::new(vec![b' '; MAX_FRAME_BYTES + 1])),
            false,
            1.into(),
            false,
        );
        assert!(reader.next().await.is_err());
    }

    #[test]
    fn fixture_endpoint_cannot_select_remote_http_or_embed_credentials() {
        for url in [
            "http://example.com/mcp",
            "http://localhost/mcp",
            "http://127.0.0.1/mcp?key=x",
            "http://user@127.0.0.1/mcp",
            "https://127.0.0.1/mcp",
        ] {
            assert!(HttpTransport::new(url, header::HeaderMap::new(), true).is_err());
        }
        assert!(
            HttpTransport::new("http://127.0.0.1/mcp", header::HeaderMap::new(), false).is_err()
        );
    }
}
