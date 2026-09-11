//! Single-attempt scoped task HTTP/SSE transport with joined bounded cancellation.
use super::{
    RemoteIdentity, ValidatedCard,
    lifecycle::{self, Disposition, Lifecycle, ResultData, Update},
    sse,
    trace::TraceContext,
    wire::{self, Mode, Part},
};
use crate::{
    CancellationToken, DeliveryCertainty,
    deployment::{CredentialConsumer, ScopedCredential},
};
use futures_util::future::BoxFuture;
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Selection,
    Credential,
    Wire(wire::Error),
    Lifecycle(lifecycle::Error),
    Transport,
    HttpStatus(u16),
    MediaType,
    Cancelled,
    TimedOut,
    Idle,
    Sink,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReceipt {
    NotNeeded,
    Unassigned,
    Received(wire::TaskState),
    Rejected(i64),
    Unconfirmed,
}
pub struct Execution {
    pub result: Result<ResultData, Error>,
    pub remote: RemoteIdentity,
    pub delivery: DeliveryCertainty,
    pub cancellation: CancelReceipt,
}
pub struct Request<'a> {
    pub parts: &'a [Part],
    pub accepted_output_modes: &'a [&'a str],
    pub stream: bool,
    pub trace: Option<&'a TraceContext>,
    pub deadline: Instant,
    pub cleanup_deadline: Instant,
}
pub trait Sink: Send {
    fn update<'a>(
        &'a mut self,
        kind: Update,
        result: &'a ResultData,
    ) -> BoxFuture<'a, Result<(), Error>>;
}
impl Sink for () {
    fn update<'a>(&'a mut self, _: Update, _: &'a ResultData) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}
/// Construct once per admitted peer and reuse its connection pool. Credentials
/// are sensitive host-selected headers, never taken from card/response metadata.
pub struct TaskClient {
    http: Client,
    target: Url,
    headers: HeaderMap,
    card: ValidatedCard,
}
impl TaskClient {
    pub fn new(card: &ValidatedCard, credential: Option<&ScopedCredential>) -> Result<Self, Error> {
        let target = Url::parse(&card.endpoint).map_err(|_| Error::Selection)?;
        if card.endpoint.len() > 4096
            || target.scheme() != "https"
            || target.host_str().is_none()
            || !target.username().is_empty()
            || target.password().is_some()
            || target.query().is_some()
            || target.fragment().is_some()
            || card.protocol_version != super::PROTOCOL_VERSION
            || card.binding != "JSONRPC"
        {
            return Err(Error::Selection);
        }
        let mut headers = HeaderMap::new();
        if let Some(credential) = credential {
            let value = credential
                .expose_for(CredentialConsumer::A2aBearer, &card.endpoint)
                .map_err(|_| Error::Credential)?;
            let mut header =
                HeaderValue::from_str(&format!("Bearer {value}")).map_err(|_| Error::Credential)?;
            header.set_sensitive(true);
            headers.insert(AUTHORIZATION, header);
        }
        Ok(Self {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .no_proxy()
                .build()
                .map_err(|_| Error::Transport)?,
            target,
            headers,
            card: card.clone(),
        })
    }
    /// An explicit host test transport override. It cannot forward a credential.
    #[doc(hidden)]
    pub fn fixture(card: &ValidatedCard, loopback: &str) -> Result<Self, Error> {
        let mut client = Self::new(card, None)?;
        let target = Url::parse(loopback).map_err(|_| Error::Selection)?;
        if loopback.len() > 4096
            || target.scheme() != "http"
            || !matches!(target.host_str(), Some("127.0.0.1" | "[::1]"))
            || !target.username().is_empty()
            || target.password().is_some()
            || target.query().is_some()
            || target.fragment().is_some()
        {
            return Err(Error::Selection);
        }
        client.target = target;
        Ok(client)
    }
    pub async fn execute<S: Sink>(
        &self,
        request: Request<'_>,
        cancellation: &CancellationToken,
        sink: &mut S,
    ) -> Execution {
        let mut lifecycle = Lifecycle::default();
        let mut delivery = DeliveryCertainty::NotSent;
        let deadline = request
            .deadline
            .min(Instant::now() + Duration::from_millis(wire::MAX_TASK_MS));
        let rpc_id = uuid::Uuid::new_v4().to_string();
        let message_id = uuid::Uuid::new_v4().to_string();
        let preflight = if cancellation.is_cancelled() {
            Err(Error::Cancelled)
        } else if Instant::now() >= deadline {
            Err(Error::TimedOut)
        } else if (request.stream && !self.card.streaming)
            || request.parts.iter().any(|part| {
                let media = part
                    .media_type
                    .as_deref()
                    .unwrap_or(if part.text.is_some() {
                        "text/plain"
                    } else if part.data.is_some() {
                        "application/json"
                    } else {
                        "application/octet-stream"
                    });
                !self
                    .card
                    .input_modes
                    .iter()
                    .any(|mode| modes_overlap(mode, media))
            })
            || request.accepted_output_modes.iter().any(|mode| {
                !self
                    .card
                    .output_modes
                    .iter()
                    .any(|offered| modes_overlap(mode, offered))
            })
        {
            Err(Error::Selection)
        } else {
            wire::send_parts_request(
                &rpc_id,
                &message_id,
                request.parts,
                &wire::SendOptions {
                    stream: request.stream,
                    accepted_output_modes: request.accepted_output_modes,
                    trace: request.trace,
                    trace_negotiated: self.card.trace_context,
                },
            )
            .map_err(Error::Wire)
        };
        let outcome = match preflight {
            Err(error) => Err(error),
            Ok(encoded) => {
                let work = self.receive(
                    encoded,
                    &rpc_id,
                    request.stream,
                    &mut lifecycle,
                    &mut delivery,
                    sink,
                );
                tokio::select! {biased;
                    _=cancellation.cancelled()=>Err(Error::Cancelled),
                    _=tokio::time::sleep_until(deadline)=>Err(Error::TimedOut),
                    result=work=>result,
                }
            }
        };
        let remote = lifecycle.observed().clone();
        let settled = matches!(
            lifecycle.result().disposition,
            Some(
                Disposition::Completed
                    | Disposition::Failed
                    | Disposition::Canceled
                    | Disposition::Rejected
            )
        );
        let result = outcome.and_then(|()| lifecycle.finish().map_err(Error::Lifecycle));
        let cancellation = if delivery == DeliveryCertainty::NotSent || settled {
            CancelReceipt::NotNeeded
        } else if let Some(task) = remote.task_id.as_deref() {
            self.cancel(task, &remote, request.trace, request.cleanup_deadline)
                .await
        } else {
            CancelReceipt::Unassigned
        };
        Execution {
            result,
            remote,
            delivery,
            cancellation,
        }
    }
    async fn receive<S: Sink>(
        &self,
        encoded: wire::EncodedRequest,
        rpc_id: &str,
        stream: bool,
        lifecycle: &mut Lifecycle,
        delivery: &mut DeliveryCertainty,
        sink: &mut S,
    ) -> Result<(), Error> {
        let trace_negotiated = encoded.headers.contains_key("a2a-extensions");
        let mut idle = Instant::now() + Duration::from_millis(wire::MAX_IDLE_MS);
        *delivery = DeliveryCertainty::MayHaveBeenSent;
        let mut response = tokio::time::timeout_at(
            idle,
            self.http
                .post(self.target.clone())
                .headers(self.headers.clone())
                .headers(encoded.headers)
                .body(encoded.body)
                .send(),
        )
        .await
        .map_err(|_| Error::Idle)?
        .map_err(|_| Error::Transport)?;
        *delivery = DeliveryCertainty::ResponseReceived;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::HttpStatus(response.status().as_u16()));
        }
        let expected = if stream {
            "text/event-stream"
        } else {
            "application/json"
        };
        if !media_type(&response, expected) {
            return Err(Error::MediaType);
        }
        let maximum = if stream {
            wire::MAX_STREAM_BYTES
        } else {
            wire::MAX_RESPONSE_BYTES
        };
        if response
            .content_length()
            .is_some_and(|n| n > maximum as u64)
        {
            return Err(Error::Wire(wire::Error::Bound));
        }
        let mut received_bytes = 0usize;
        let mut parser = sse::Decoder::default();
        let mut bytes = Vec::new();
        loop {
            if Instant::now() >= idle {
                return Err(Error::Idle);
            }
            let chunk = tokio::time::timeout_at(idle, response.chunk())
                .await
                .map_err(|_| Error::Idle)?
                .map_err(|_| Error::Transport)?;
            let Some(chunk) = chunk else { break };
            if chunk.len() > maximum.saturating_sub(received_bytes) {
                return Err(Error::Wire(wire::Error::Bound));
            }
            received_bytes += chunk.len();
            if stream {
                for byte in chunk {
                    if let Some(data) = parser.push(byte).map_err(Error::Wire)? {
                        let kind = lifecycle
                            .ingest(&data, rpc_id, Mode::Stream, trace_negotiated)
                            .map_err(Error::Lifecycle)?;
                        sink.update(kind, lifecycle.result())
                            .await
                            .map_err(|_| Error::Sink)?;
                        idle = Instant::now() + Duration::from_millis(wire::MAX_IDLE_MS);
                        // A terminal protocol update ends this exchange; do not
                        // keep polling an open remote connection after settlement.
                        if lifecycle.result().disposition.is_some() {
                            return Ok(());
                        }
                    }
                }
            } else {
                if chunk.len() > wire::MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                    return Err(Error::Wire(wire::Error::Bound));
                }
                bytes.extend_from_slice(&chunk);
            }
        }
        if stream {
            parser.finish().map_err(Error::Wire)?;
        } else {
            let kind = lifecycle
                .ingest(&bytes, rpc_id, Mode::Send, trace_negotiated)
                .map_err(Error::Lifecycle)?;
            sink.update(kind, lifecycle.result())
                .await
                .map_err(|_| Error::Sink)?;
        }
        Ok(())
    }
    async fn cancel(
        &self,
        task_id: &str,
        remote: &RemoteIdentity,
        trace: Option<&TraceContext>,
        cleanup_deadline: Instant,
    ) -> CancelReceipt {
        let deadline =
            cleanup_deadline.min(Instant::now() + Duration::from_millis(wire::MAX_CANCEL_MS));
        if Instant::now() >= deadline {
            return CancelReceipt::Unconfirmed;
        }
        let work = async {
            let rpc_id = uuid::Uuid::new_v4().to_string();
            let body = wire::cancel_request(&rpc_id, task_id).ok()?;
            let mut headers = self.headers.clone();
            headers.insert(
                "a2a-version",
                HeaderValue::from_static(super::PROTOCOL_VERSION),
            );
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert("accept", HeaderValue::from_static("application/json"));
            if self.card.trace_context
                && let Some(trace) = trace
            {
                headers.extend(trace.headers());
            }
            let mut response = self
                .http
                .post(self.target.clone())
                .headers(headers)
                .body(body)
                .send()
                .await
                .ok()?;
            if response.status() != reqwest::StatusCode::OK
                || !media_type(&response, "application/json")
            {
                return None;
            }
            if response
                .content_length()
                .is_some_and(|n| n > wire::MAX_RESPONSE_BYTES as u64)
            {
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if chunk.len() > wire::MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            match wire::decode(
                &bytes,
                &rpc_id,
                Mode::Cancel,
                self.card.trace_context && trace.is_some(),
            ) {
                Ok(wire::Reply::Task(task))
                    if task.id == task_id
                        && remote.context_id.as_deref() == Some(&task.context_id) =>
                {
                    Some(CancelReceipt::Received(task.status.state))
                }
                Err(wire::Error::Remote(code)) => Some(CancelReceipt::Rejected(code)),
                _ => None,
            }
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .ok()
            .flatten()
            .unwrap_or(CancelReceipt::Unconfirmed)
    }
}
fn media_type(response: &reqwest::Response, expected: &str) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case(expected)
        })
}

fn modes_overlap(left: &str, right: &str) -> bool {
    fn essence(value: &str) -> Option<(&str, &str)> {
        value.split(';').next()?.trim().split_once('/')
    }
    let (Some((lt, ls)), Some((rt, rs))) = (essence(left), essence(right)) else {
        return false;
    };
    (lt == "*" || rt == "*" || lt.eq_ignore_ascii_case(rt))
        && (ls == "*" || rs == "*" || ls.eq_ignore_ascii_case(rs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::{
        self, ConfigInput, CredentialInputs, CredentialReadError, ResolveRequest, RunInput,
    };
    struct Inputs;
    impl CredentialInputs for Inputs {
        fn environment(&self, _: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
            panic!("no ambient credential lookup")
        }
        fn host(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
            assert_eq!(name, "synthetic");
            Ok(Some(b"synthetic-bearer".to_vec()))
        }
    }
    #[test]
    fn bearer_header_is_sensitive_and_cannot_cross_endpoint_scope() {
        let cwd = std::env::current_dir().unwrap();
        let mut request = ResolveRequest::new(cwd.clone(), "unused");
        request.path_bindings.insert("workspace".into(), cwd);
        request.entry = ConfigInput::Document(
            serde_json::json!({"schema_version":1,"options":{"a2a":{"remotes":{"peer":{"card_url":"https://agent.example.test/card","endpoint":"https://agent.example.test/rpc","bearer":{"scheme":"token","credential":"token"}}}}},"credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"unused"}]},"token":{"consumer":"a2a.bearer","sources":[{"kind":"host","name":"synthetic"}]}}}),
        );
        let prepared = deployment::resolve(request)
            .unwrap()
            .prepare_run(RunInput {
                input: "synthetic".into(),
                workspace: None,
                session_id: None,
            })
            .unwrap();
        let credential = prepared.a2a_credential("peer", &Inputs).unwrap().unwrap();
        let mut card =
            super::super::CardAdmission::new("https://agent.example.test/rpc", None, false)
                .unwrap()
                .validate(include_bytes!("../../../../tests/fixtures/a2a/card.json"))
                .unwrap();
        let client = TaskClient::new(&card, Some(&credential)).unwrap();
        assert!(client.headers[AUTHORIZATION].is_sensitive());
        assert_eq!(client.headers[AUTHORIZATION], "Bearer synthetic-bearer");
        card.endpoint = "https://other.example.test/rpc".into();
        assert!(matches!(
            TaskClient::new(&card, Some(&credential)),
            Err(Error::Credential)
        ));
        assert!(TaskClient::fixture(&card, "http://external.example.test/rpc").is_err());
    }
    #[test]
    fn selected_modes_use_media_ranges_without_treating_parameters_as_authority() {
        assert!(modes_overlap(
            "application/json",
            "application/json; charset=utf-8"
        ));
        assert!(modes_overlap("text/*", "text/plain"));
        assert!(modes_overlap("*/*", "application/pdf"));
        assert!(!modes_overlap("application/json", "text/plain"));
        assert!(!modes_overlap("invalid", "text/plain"));
    }
}
