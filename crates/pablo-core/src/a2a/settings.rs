//! Host-authored remote definitions; never populated from Agent Card metadata.
use super::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub remotes: BTreeMap<String, Remote>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub card_url: String,
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer: Option<Bearer>,
    #[serde(default)]
    pub trace_context: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bearer {
    pub scheme: String,
    pub credential: String,
}
impl Remote {
    pub fn admission(&self) -> Result<CardAdmission, AdmissionError> {
        CardAdmission::new(&self.card_url, None, false)?;
        if self
            .bearer
            .as_ref()
            .is_some_and(|b| !identifier(&b.credential))
        {
            return Err(AdmissionError::UnsupportedSecurity);
        }
        CardAdmission::new(
            &self.endpoint,
            self.bearer.as_ref().map(|b| b.scheme.clone()),
            self.trace_context,
        )
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), AdmissionError> {
        if self.remotes.len() > 16 {
            return Err(AdmissionError::CardBound);
        }
        for (id, remote) in &self.remotes {
            if !identifier(id) {
                return Err(AdmissionError::InvalidCard);
            }
            remote.admission()?;
        }
        Ok(())
    }
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
