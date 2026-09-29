//! Explicit provider-managed prompt caching intent; never local response reuse.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCaching {
    #[default]
    ProviderDefault,
    Auto,
}
impl PromptCaching {
    pub fn is_default(&self) -> bool {
        *self == Self::ProviderDefault
    }
    pub fn validate_gateway(self, kind: crate::gateway::GatewayKind) -> Result<(), &'static str> {
        if self.is_default() || kind == crate::gateway::GatewayKind::Vercel {
            Ok(())
        } else {
            Err("automatic prompt caching requires Vercel Chat Completions")
        }
    }
}
impl std::str::FromStr for PromptCaching {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "provider_default" => Ok(Self::ProviderDefault),
            "auto" => Ok(Self::Auto),
            _ => Err("prompt caching must be provider_default or auto"),
        }
    }
}
