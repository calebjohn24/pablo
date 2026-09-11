//! Optional correlation data. Remote receipts never create a local parent span.
use super::{TRACE_EXTENSION, wire::Error};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_TRACE_METADATA_BYTES: usize = 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TraceContext {
    traceparent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracestate: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    traceparent: String,
    #[serde(default)]
    tracestate: Option<String>,
}
impl TraceContext {
    pub fn new(parent: &str, state: Option<&str>) -> Result<Self, Error> {
        let bytes = parent.as_bytes();
        if bytes.len() != 55
            || &bytes[..3] != b"00-"
            || bytes[35] != b'-'
            || bytes[52] != b'-'
            || !bytes.iter().enumerate().all(|(i, b)| {
                matches!(i, 2 | 35 | 52) || b.is_ascii_digit() || (b'a'..=b'f').contains(b)
            })
            || bytes[3..35].iter().all(|b| *b == b'0')
            || bytes[36..52].iter().all(|b| *b == b'0')
        {
            return Err(Error::Invalid);
        }
        let state = state.filter(|s| !s.is_empty());
        if let Some(state) = state {
            if state.len() > 512 {
                return Err(Error::Invalid);
            }
            let mut keys = std::collections::BTreeSet::new();
            for member in state.split(',') {
                let (key, value) = member.trim().split_once('=').ok_or(Error::Invalid)?;
                if key.is_empty() || value.is_empty() || !keys.insert(key) || keys.len() > 32 {
                    return Err(Error::Invalid);
                }
            }
            if !state.is_ascii()
                || state.chars().any(char::is_control)
                || state.parse::<opentelemetry::trace::TraceState>().is_err()
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Self {
            traceparent: parent.into(),
            tracestate: state.map(Into::into),
        })
    }
    pub fn traceparent(&self) -> &str {
        &self.traceparent
    }
    pub fn tracestate(&self) -> Option<&str> {
        self.tracestate.as_deref()
    }
    pub fn metadata(&self) -> Value {
        json!({TRACE_EXTENSION:self})
    }
    pub fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "a2a-extensions",
            reqwest::header::HeaderValue::from_static(TRACE_EXTENSION),
        );
        headers.insert(
            "traceparent",
            self.traceparent.parse().expect("validated traceparent"),
        );
        if let Some(state) = &self.tracestate {
            headers.insert("tracestate", state.parse().expect("validated tracestate"));
        }
        headers
    }
}
pub fn remote_metadata(value: &Value, negotiated: bool) -> Result<Option<TraceContext>, Error> {
    if value.is_null() {
        return Ok(None);
    }
    let map = value.as_object().ok_or(Error::Invalid)?;
    let Some(value) = map.get(TRACE_EXTENSION) else {
        return Ok(None);
    };
    if !negotiated {
        return Ok(None);
    }
    if serde_json::to_vec(value).map_err(|_| Error::Invalid)?.len() > MAX_TRACE_METADATA_BYTES {
        return Err(Error::Bound);
    }
    let fields: Fields = serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
    TraceContext::new(&fields.traceparent, fields.tracestate.as_deref()).map(Some)
}
