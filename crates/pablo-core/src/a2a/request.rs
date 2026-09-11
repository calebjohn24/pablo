//! Explicit selected remote input; no local RunSpec, transcript or credential fields.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnRequest {
    pub remote: String,
    pub parts: Vec<super::wire::Part>,
    pub accepted_output_modes: Vec<String>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
}
impl SpawnRequest {
    pub fn validate_shape(&self) -> Result<(), crate::children::ContractError> {
        if self.remote.is_empty()
            || self.remote.len() > 128
            || !self
                .remote
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(crate::children::ContractError::InvalidSelection);
        }
        let modes = self
            .accepted_output_modes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        super::wire::send_parts_request(
            "shape",
            "shape",
            &self.parts,
            &super::wire::SendOptions {
                accepted_output_modes: &modes,
                stream: self.stream.unwrap_or(false),
                ..Default::default()
            },
        )
        .map_err(|_| crate::children::ContractError::InputBound)?;
        Ok(())
    }
}
