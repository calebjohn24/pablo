//! Bounded, no-redirect public-card retrieval. Credentials are not inferred from cards.
use super::*;
use crate::CancellationToken;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchError {
    Admission(AdmissionError),
    Transport,
    HttpStatus(u16),
    MediaType,
    Cancelled,
    TimedOut,
}
#[derive(Clone)]
pub struct CardClient {
    client: reqwest::Client,
}
impl CardClient {
    pub fn new() -> Result<Self, FetchError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| FetchError::Transport)?,
        })
    }
    pub async fn fetch(
        &self,
        selection: &CardAdmission,
        card_url: &str,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ValidatedCard, FetchError> {
        let target = CardAdmission::new(card_url, None, false)
            .map_err(FetchError::Admission)?
            .endpoint;
        self.fetch_url(selection, target, deadline, cancellation)
            .await
    }
    /// Explicit independent-fixture hook. Validate the original host URL first;
    /// only literal loopback HTTP can replace transport, never card authority.
    #[doc(hidden)]
    pub async fn fetch_fixture(
        &self,
        selection: &CardAdmission,
        card_url: &str,
        loopback: &str,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ValidatedCard, FetchError> {
        CardAdmission::new(card_url, None, false).map_err(FetchError::Admission)?;
        if loopback.len() > 4096 {
            return Err(FetchError::Admission(AdmissionError::InvalidEndpoint));
        }
        let target = reqwest::Url::parse(loopback)
            .map_err(|_| FetchError::Admission(AdmissionError::InvalidEndpoint))?;
        if target.scheme() != "http"
            || !matches!(target.host_str(), Some("127.0.0.1" | "[::1]"))
            || !target.username().is_empty()
            || target.password().is_some()
            || target.query().is_some()
            || target.fragment().is_some()
        {
            return Err(FetchError::Admission(AdmissionError::InvalidEndpoint));
        }
        self.fetch_url(selection, target, deadline, cancellation)
            .await
    }
    async fn fetch_url(
        &self,
        selection: &CardAdmission,
        target: reqwest::Url,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ValidatedCard, FetchError> {
        let deadline = deadline.min(Instant::now() + std::time::Duration::from_secs(10));
        // An expired timer can initially poll Pending before the timer driver
        // advances. Do not let that start a request already known to be stopped.
        if Instant::now() >= deadline {
            return Err(FetchError::TimedOut);
        }
        if cancellation.is_cancelled() {
            return Err(FetchError::Cancelled);
        }
        let work = async {
            let mut response = self
                .client
                .get(target)
                .header("A2A-Version", PROTOCOL_VERSION)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|_| FetchError::Transport)?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(FetchError::HttpStatus(response.status().as_u16()));
            }
            if !response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("application/json")
                })
            {
                return Err(FetchError::MediaType);
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_CARD_BYTES as u64)
            {
                return Err(FetchError::Admission(AdmissionError::CardBound));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| FetchError::Transport)? {
                if chunk.len() > MAX_CARD_BYTES.saturating_sub(bytes.len()) {
                    return Err(FetchError::Admission(AdmissionError::CardBound));
                }
                bytes.extend_from_slice(&chunk);
            }
            selection.validate(&bytes).map_err(FetchError::Admission)
        };
        tokio::select! { biased; _=tokio::time::sleep_until(deadline)=>Err(FetchError::TimedOut), _=cancellation.cancelled()=>Err(FetchError::Cancelled), result=work=>result }
    }
}
