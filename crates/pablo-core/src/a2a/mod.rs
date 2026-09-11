//! Host-selected A2A admission. Remote descriptions never grant local authority.
mod fetch;
mod proxy;
mod settings;
pub mod wire;
pub use fetch::{CardClient, FetchError};
pub use proxy::{IdentityError, MAX_REMOTE_ID_BYTES, RemoteIdentity, RemoteProxy, ResolveError};
use serde::{Deserialize, Serialize};
pub use settings::{Bearer, Remote, Settings};
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: &str = "1.0";
pub const BINDING: &str = "JSONRPC";
pub const TRACE_EXTENSION: &str = "urn:pablo:a2a:tracecontext:v1";
pub const MAX_CARD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    InvalidEndpoint,
    CardBound,
    InvalidCard,
    UnsupportedInterface,
    UnsupportedExtension,
    UnsupportedSecurity,
}

/// Immutable host selection. No card field can replace the selected endpoint,
/// choose a credential source or activate an extension the host did not select.
#[derive(Clone, Debug)]
pub struct CardAdmission {
    endpoint: reqwest::Url,
    bearer_scheme: Option<String>,
    trace_context: bool,
}
impl CardAdmission {
    pub fn new(
        endpoint: &str,
        bearer_scheme: Option<String>,
        trace_context: bool,
    ) -> Result<Self, AdmissionError> {
        if endpoint.len() > 4096 {
            return Err(AdmissionError::InvalidEndpoint);
        }
        let url = reqwest::Url::parse(endpoint).map_err(|_| AdmissionError::InvalidEndpoint)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || bearer_scheme
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 128)
        {
            return Err(AdmissionError::InvalidEndpoint);
        }
        Ok(Self {
            endpoint: url,
            bearer_scheme,
            trace_context,
        })
    }

    pub fn validate(&self, bytes: &[u8]) -> Result<ValidatedCard, AdmissionError> {
        if bytes.len() > MAX_CARD_BYTES {
            return Err(AdmissionError::CardBound);
        }
        // Typed deserialization rejects duplicate known fields. The JSON parser's
        // recursion bound applies; unknown additive metadata stays untrusted.
        let card: Card = serde_json::from_slice(bytes).map_err(|_| AdmissionError::InvalidCard)?;
        if !bounded(&card.name, 256)
            || !bounded(&card.description, 8192)
            || !bounded(&card.version, 128)
            || card.supported_interfaces.len() > 16
            || card.skills.len() > 64
            || card.capabilities.extensions.len() > 16
            || card.default_input_modes.len() > 16
            || card.default_output_modes.len() > 16
            || card.security_schemes.len() > 16
            || card.security_requirements.len() > 16
        {
            return Err(AdmissionError::InvalidCard);
        }
        if !card.default_input_modes.iter().any(|s| s == "text/plain")
            || !card
                .default_output_modes
                .iter()
                .any(|s| s == "text/plain" || s == "application/json")
        {
            return Err(AdmissionError::UnsupportedInterface);
        }
        let mut matched = card.supported_interfaces.iter().filter(|interface| {
            interface.protocol_version == PROTOCOL_VERSION
                && interface.protocol_binding == BINDING
                && interface.tenant.is_empty()
                && reqwest::Url::parse(&interface.url).is_ok_and(|url| url == self.endpoint)
        });
        if matched.next().is_none() || matched.next().is_some() {
            return Err(AdmissionError::UnsupportedInterface);
        }
        let mut ids = std::collections::BTreeSet::new();
        for skill in &card.skills {
            if !bounded(&skill.id, 128)
                || !bounded(&skill.name, 256)
                || !bounded(&skill.description, 8192)
                || skill.tags.len() > 32
                || !ids.insert(&skill.id)
            {
                return Err(AdmissionError::InvalidCard);
            }
        }
        let mut trace_context = false;
        let mut extensions = std::collections::BTreeSet::new();
        for extension in &card.capabilities.extensions {
            if !bounded(&extension.uri, 2048) || !extensions.insert(&extension.uri) {
                return Err(AdmissionError::InvalidCard);
            }
            let understood = self.trace_context
                && extension.uri == TRACE_EXTENSION
                && extension
                    .params
                    .as_ref()
                    .is_none_or(|v| v.as_object().is_some_and(|v| v.is_empty()));
            if extension.required && !understood {
                return Err(AdmissionError::UnsupportedExtension);
            }
            trace_context |= understood;
        }
        let supports_bearer = |name: &str| {
            card.security_schemes.get(name).is_some_and(|scheme| {
                scheme.as_object().is_some_and(|s| s.len() == 1)
                    && scheme["httpAuthSecurityScheme"]["scheme"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case("bearer"))
            })
        };
        if self
            .bearer_scheme
            .as_ref()
            .is_some_and(|name| !supports_bearer(name))
            || (!card.security_requirements.is_empty()
                && !card.security_requirements.iter().any(|requirement| {
                    requirement.schemes.is_empty()
                        || self.bearer_scheme.as_ref().is_some_and(|name| {
                            requirement.schemes.len() == 1
                                && requirement
                                    .schemes
                                    .get(name)
                                    .is_some_and(|scopes| scopes.list.is_empty())
                                && supports_bearer(name)
                        })
                }))
        {
            return Err(AdmissionError::UnsupportedSecurity);
        }
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
        Ok(ValidatedCard {
            endpoint: self.endpoint.as_str().into(),
            protocol_version: PROTOCOL_VERSION,
            binding: BINDING,
            card_sha256: digest.as_ref().iter().map(|b| format!("{b:02x}")).collect(),
            name: card.name,
            description: card.description,
            version: card.version,
            streaming: card.capabilities.streaming,
            trace_context,
        })
    }
}
fn bounded(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.contains('\0')
}

/// Untrusted display metadata plus validated transport identity, without secrets
/// or a claim that a task has been submitted or a remote agent authenticated.
#[derive(Clone, Debug, Serialize)]
pub struct ValidatedCard {
    pub endpoint: String,
    pub protocol_version: &'static str,
    pub binding: &'static str,
    pub card_sha256: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub streaming: bool,
    pub trace_context: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Card {
    name: String,
    description: String,
    version: String,
    supported_interfaces: Vec<Interface>,
    capabilities: Capabilities,
    default_input_modes: Vec<String>,
    default_output_modes: Vec<String>,
    #[serde(default)]
    skills: Vec<Skill>,
    #[serde(default)]
    security_schemes: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    security_requirements: Vec<SecurityRequirement>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Interface {
    url: String,
    protocol_binding: String,
    protocol_version: String,
    #[serde(default)]
    tenant: String,
}
#[derive(Deserialize)]
struct Capabilities {
    #[serde(default)]
    streaming: bool,
    #[serde(default)]
    extensions: Vec<Extension>,
}
#[derive(Deserialize)]
struct Extension {
    uri: String,
    #[serde(default)]
    required: bool,
    params: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct Skill {
    id: String,
    name: String,
    description: String,
    #[serde(default)]
    tags: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecurityRequirement {
    #[serde(default)]
    schemes: BTreeMap<String, Scopes>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scopes {
    #[serde(default)]
    list: Vec<String>,
}
