//! Optional peer claims. Never convert these into enforceable local Accounting.
use serde::{Deserialize, Serialize};
pub const METADATA_KEY: &str = "urn:pablo:a2a:reported-usage:v1";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportedUsage {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::task::optional_decimal"
    )]
    pub input_tokens: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::task::optional_decimal"
    )]
    pub output_tokens: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::task::optional_decimal"
    )]
    pub total_tokens: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::task::optional_decimal"
    )]
    pub cost_microusd: Option<u64>,
}
pub(super) fn decode(
    metadata: &serde_json::Value,
) -> Result<Option<ReportedUsage>, super::wire::Error> {
    let Some(value) = metadata.get(METADATA_KEY) else {
        return Ok(None);
    };
    if serde_json::to_vec(value)
        .map_err(|_| super::wire::Error::Invalid)?
        .len()
        > 1024
    {
        return Err(super::wire::Error::Bound);
    }
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| super::wire::Error::Invalid)
}
