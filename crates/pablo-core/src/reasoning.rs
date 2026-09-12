//! Provider-independent reasoning intent. Capability data belongs to model profiles.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    ProviderDefault,
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl ReasoningEffort {
    pub fn name(self) -> &'static str {
        match self {
            Self::ProviderDefault => "provider_default",
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}
impl std::str::FromStr for ReasoningEffort {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "provider_default" => Ok(Self::ProviderDefault),
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            _ => Err("unsupported reasoning effort"),
        }
    }
}

/// A string effort or `{ "budget_tokens": N }`; the alternatives cannot conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ReasoningConfig {
    Effort(ReasoningEffort),
    Budget { budget_tokens: u32 },
}
impl Default for ReasoningConfig {
    fn default() -> Self {
        Self::Effort(ReasoningEffort::ProviderDefault)
    }
}
impl ReasoningConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self, max_output_tokens: u32) -> Result<(), &'static str> {
        if let Self::Budget { budget_tokens } = self
            && (*budget_tokens == 0 || *budget_tokens >= max_output_tokens)
        {
            return Err("reasoning budget must be positive and below max output tokens");
        }
        Ok(())
    }

    pub fn validate_gateway(
        &self,
        kind: crate::gateway::GatewayKind,
        capabilities: Option<&ReasoningCapabilities>,
        max_output_tokens: u32,
    ) -> Result<(), &'static str> {
        self.validate(max_output_tokens)?;
        if kind == crate::gateway::GatewayKind::OpenResponses
            && !matches!(
                self,
                Self::Effort(
                    ReasoningEffort::ProviderDefault
                        | ReasoningEffort::None
                        | ReasoningEffort::Low
                        | ReasoningEffort::Medium
                        | ReasoningEffort::High
                        | ReasoningEffort::Xhigh
                )
            )
        {
            return Err("reasoning setting unsupported by the pinned Open Responses profile");
        }
        if let Some(caps) = capabilities {
            caps.validate()?;
            match self {
                Self::Effort(ReasoningEffort::ProviderDefault) => {}
                Self::Effort(effort)
                    if (caps.mandatory && *effort == ReasoningEffort::None)
                        || caps
                            .efforts
                            .as_ref()
                            .is_some_and(|levels| !levels.contains(effort)) =>
                {
                    return Err("reasoning effort unsupported by the model profile");
                }
                Self::Budget { .. } if caps.token_budget == Some(false) => {
                    return Err("reasoning budget unsupported by the model profile");
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Both gateway chat APIs accept this normalized shape. Default adds no fields.
    pub(crate) fn wire(&self) -> Option<serde_json::Value> {
        match self {
            Self::Effort(ReasoningEffort::ProviderDefault) => None,
            Self::Effort(effort) => Some(serde_json::json!({"effort":effort})),
            Self::Budget { budget_tokens } => Some(serde_json::json!({"max_tokens":budget_tokens})),
        }
    }
}

/// Optional operator-declared capabilities; absence is unknown, not unsupported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<ReasoningEffort>>,
    #[serde(default)]
    pub mandatory: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<bool>,
}
impl ReasoningCapabilities {
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(levels) = &self.efforts
            && (levels.len() > 7
                || levels.contains(&ReasoningEffort::ProviderDefault)
                || levels
                    .iter()
                    .enumerate()
                    .any(|(i, level)| levels[..i].contains(level))
                || self.mandatory && levels.contains(&ReasoningEffort::None))
        {
            return Err("invalid reasoning capabilities");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::GatewayKind::*;
    #[test]
    fn explicit_intent_is_never_clamped_and_unknown_models_need_no_catalog() {
        for kind in [Openrouter, Vercel, OpenResponses] {
            for effort in [
                ReasoningEffort::ProviderDefault,
                ReasoningEffort::None,
                ReasoningEffort::Minimal,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Xhigh,
                ReasoningEffort::Max,
            ] {
                let config = ReasoningConfig::Effort(effort);
                let supported = kind != OpenResponses
                    || !matches!(effort, ReasoningEffort::Minimal | ReasoningEffort::Max);
                assert_eq!(config.validate_gateway(kind, None, 4096).is_ok(), supported);
                assert_eq!(
                    config.wire(),
                    if config.is_default() {
                        None
                    } else {
                        Some(serde_json::json!({"effort":effort.name()}))
                    }
                );
            }
            assert_eq!(
                ReasoningConfig::Budget {
                    budget_tokens: 1024
                }
                .validate_gateway(kind, None, 4096)
                .is_ok(),
                kind != OpenResponses
            );
        }
        let caps = ReasoningCapabilities {
            mandatory: true,
            efforts: Some(vec![ReasoningEffort::Low, ReasoningEffort::High]),
            token_budget: Some(false),
        };
        for kind in [Openrouter, Vercel, OpenResponses] {
            assert!(
                ReasoningConfig::default()
                    .validate_gateway(kind, Some(&caps), 4096)
                    .is_ok()
            );
            assert!(
                ReasoningConfig::Effort(ReasoningEffort::Low)
                    .validate_gateway(kind, Some(&caps), 4096)
                    .is_ok()
            );
            for config in [
                ReasoningConfig::Effort(ReasoningEffort::None),
                ReasoningConfig::Effort(ReasoningEffort::Medium),
                ReasoningConfig::Budget {
                    budget_tokens: 1024,
                },
            ] {
                assert!(config.validate_gateway(kind, Some(&caps), 4096).is_err());
            }
        }
    }
    #[test]
    fn budgets_and_capabilities_reject_invalid_or_ambiguous_configuration() {
        for value in [
            serde_json::json!({"effort":"low","budget_tokens":10}),
            serde_json::json!({"budget_tokens":10,"extra":true}),
            serde_json::json!("invented"),
            serde_json::json!({"budget_tokens":-1}),
        ] {
            assert!(serde_json::from_value::<ReasoningConfig>(value).is_err());
        }
        for n in [0, 4096, u32::MAX] {
            assert!(
                ReasoningConfig::Budget { budget_tokens: n }
                    .validate(4096)
                    .is_err()
            );
        }
        assert_eq!(
            ReasoningConfig::Budget {
                budget_tokens: 1024
            }
            .wire(),
            Some(serde_json::json!({"max_tokens":1024}))
        );
        let mut caps = ReasoningCapabilities::default();
        for levels in [
            vec![ReasoningEffort::ProviderDefault],
            vec![ReasoningEffort::Low; 2],
        ] {
            caps.efforts = Some(levels);
            assert!(caps.validate().is_err());
        }
    }
}
