//! Explicit result selections are resolved only by the owning supervisor.
use super::ContractError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffKind {
    Inline,
    Artifact,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffSelection {
    pub source_agent_id: String,
    pub result_id: String,
    pub kind: HandoffKind,
}
impl HandoffSelection {
    pub fn validate(&self) -> Result<(), ContractError> {
        for id in [&self.source_agent_id, &self.result_id] {
            if id.len() > 128 || uuid::Uuid::parse_str(id).is_err() {
                return Err(ContractError::InvalidSelection);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReference {
    pub path: String,
    pub revision: String,
}
impl ArtifactReference {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.path.is_empty()
            || self.path.len() > 4096
            || self.path.contains('\0')
            || crate::policy::relative_path(&self.path).is_err()
            || crate::policy::relative_path(&self.path).is_ok_and(|p| p.as_os_str().is_empty())
            || self.revision.len() != 64
            || !self
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ContractError::InvalidSelection);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactError {
    InvalidReference,
    Unauthorized,
    Unavailable,
    Stale,
    WorkLimit,
    Cancelled,
    TimedOut,
}
