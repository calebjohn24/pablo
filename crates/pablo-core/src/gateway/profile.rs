//! Provider selection and safe model metadata. This is not a live model catalog.
use serde::{Deserialize, Serialize};

pub const VERCEL_DEFAULT_MODEL: &str = "zai/glm-5.3-flash";
pub const OPENROUTER_DEFAULT_MODEL: &str = "z-ai/glm-5.3-flash";
pub const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayKind {
    #[default]
    Vercel,
    Openrouter,
}
impl std::str::FromStr for GatewayKind {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "vercel" => Ok(Self::Vercel),
            "openrouter" => Ok(Self::Openrouter),
            "open_responses" => Err("provider_unavailable: open_responses (C3.9)"),
            _ => Err("unknown provider"),
        }
    }
}
impl GatewayKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Vercel => "vercel",
            Self::Openrouter => "openrouter",
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Vercel => VERCEL_DEFAULT_MODEL,
            Self::Openrouter => OPENROUTER_DEFAULT_MODEL,
        }
    }
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Vercel => super::VERCEL_ENDPOINT,
            Self::Openrouter => OPENROUTER_ENDPOINT,
        }
    }
    pub fn credential_consumer(self) -> crate::deployment::CredentialConsumer {
        match self {
            Self::Vercel => crate::deployment::CredentialConsumer::Vercel,
            Self::Openrouter => crate::deployment::CredentialConsumer::OpenRouter,
        }
    }
    pub fn available(self) -> bool {
        self.ensure_available().is_ok()
    }
    pub fn ensure_available(self) -> Result<(), &'static str> {
        match self {
            Self::Vercel | Self::Openrouter => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelProfile {
    pub provider: GatewayKind,
    pub model: String,
    pub endpoint: &'static str,
    pub protocol: &'static str,
    pub adapter_available: bool,
    pub capabilities: GatewayCapabilities,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GatewayCapabilities {
    pub text_streaming: bool,
    pub tool_calls: bool,
    pub reported_usage: bool,
    pub reported_cost: bool,
    pub hard_accounting_bounds: bool,
}
impl ModelProfile {
    pub fn resolve(provider: GatewayKind, model: Option<&str>) -> Result<Self, &'static str> {
        let model = model.unwrap_or(provider.default_model());
        if model.is_empty()
            || model.len() > 256
            || model.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("model must be a nonempty provider/model identifier");
        }
        Ok(Self {
            provider,
            model: model.into(),
            endpoint: provider.endpoint(),
            protocol: "chat_completions",
            adapter_available: provider.available(),
            capabilities: GatewayCapabilities {
                text_streaming: provider.available(),
                tool_calls: provider.available(),
                reported_usage: provider.available(),
                reported_cost: provider == GatewayKind::Openrouter,
                hard_accounting_bounds: false,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Provider, gateway::GatewayProvider};
    #[tokio::test]
    async fn provider_factories_keep_credentials_and_fixture_destinations_scoped() {
        let provider =
            GatewayProvider::selected(GatewayKind::Vercel, "synthetic-unread-key").unwrap();
        assert_eq!(provider.name(), "vercel");
        assert_eq!(
            provider
                .accounting_bounds(VERCEL_DEFAULT_MODEL, 1024)
                .tokens,
            None
        );
        assert!(GatewayProvider::selected(GatewayKind::Openrouter, "invalid private key").is_err());
        let router =
            GatewayProvider::selected(GatewayKind::Openrouter, "synthetic-unread-key").unwrap();
        assert_eq!(router.name(), "openrouter");
        assert_eq!(
            router
                .accounting_bounds(OPENROUTER_DEFAULT_MODEL, 1024)
                .cost_microusd,
            None
        );
        assert!("open_responses".parse::<GatewayKind>().is_err());
        assert!(
            GatewayProvider::local_fixture_for(
                GatewayKind::Openrouter,
                "http://127.0.0.1:1/fixture"
            )
            .is_ok()
        );
        for endpoint in [
            "https://openrouter.ai/api/v1/chat/completions",
            "http://localhost:1234/fixture",
            "http://127.0.0.1:1234/fixture?key=private",
            "http://user:private@127.0.0.1:1234/fixture",
            "http://127.0.0.1:1234/#fragment",
        ] {
            for kind in [GatewayKind::Vercel, GatewayKind::Openrouter] {
                assert!(GatewayProvider::local_fixture_for(kind, endpoint).is_err());
            }
        }
        assert!(
            GatewayProvider::local_fixture_for(GatewayKind::Vercel, "http://127.0.0.1:1/fixture")
                .is_ok()
        );
        assert_eq!(
            ModelProfile::resolve(GatewayKind::Openrouter, Some("explicit/model"))
                .unwrap()
                .model,
            "explicit/model"
        );
        for model in ["", "private\nmodel", "space model"] {
            assert!(ModelProfile::resolve(GatewayKind::Vercel, Some(model)).is_err());
        }
    }
}
