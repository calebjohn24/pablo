//! Tokenizer-free, task-local context estimation and compaction settings.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextSettings {
    pub enabled: bool,
    /// Operator-declared capacity for a direct/legacy model. None is unknown.
    pub window_tokens: Option<u64>,
    pub safety_margin_percent: u8,
    pub max_summary_tokens: u32,
    pub max_summary_bytes: usize,
    pub keep_recent_turns: usize,
}
impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            window_tokens: None,
            safety_margin_percent: 10,
            max_summary_tokens: 1024,
            max_summary_bytes: 16384,
            keep_recent_turns: 1,
        }
    }
}
impl ContextSettings {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if !(1..=50).contains(&self.safety_margin_percent)
            || !(16..=65536).contains(&self.max_summary_tokens)
            || !(256..=1048576).contains(&self.max_summary_bytes)
            || !(1..=32).contains(&self.keep_recent_turns)
            || self
                .window_tokens
                .is_some_and(|n| !(1..=1_000_000_000).contains(&n))
        {
            return Err("invalid context settings");
        }
        Ok(())
    }
    pub(crate) fn usable(&self, capacity: u64) -> u64 {
        (u128::from(capacity) * u128::from(100 - self.safety_margin_percent) / 100) as u64
    }
}
#[derive(Clone, Copy, Default)]
pub(crate) struct Calibration {
    observed: Option<(usize, u64)>,
}
impl Calibration {
    pub(crate) fn observe(&mut self, bytes: usize, tokens: Option<u64>) {
        if let Some(tokens) = tokens.filter(|n| *n != 0) {
            self.observed = Some((bytes.max(1), tokens));
        }
    }
    pub(crate) fn estimate(&self, bytes: usize) -> u64 {
        let base = (bytes as u128).div_ceil(4);
        let calibrated = self.observed.map_or(0, |(b, t)| {
            (bytes as u128 * u128::from(t)).div_ceil(b as u128)
        });
        base.max(calibrated).min(u128::from(u64::MAX)) as u64
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionRecord {
    pub schema_version: String,
    pub id: String,
    pub trigger: String,
    pub status: String,
    pub before_bytes: String,
    pub after_bytes: Option<String>,
    pub before_estimated_tokens: String,
    pub after_estimated_tokens: Option<String>,
    pub window_tokens: Option<String>,
    pub removed_turns: String,
    pub replaced_history_sha256: String,
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_calibration_is_conservative_profile_local_and_overflow_safe() {
        let mut first = Calibration::default();
        let second = Calibration::default();
        assert_eq!(first.estimate(10001), 2501);
        first.observe(8000, Some(4000));
        assert_eq!(first.estimate(10000), 5000);
        first.observe(9000, None);
        assert_eq!(first.estimate(10000), 5000);
        assert_eq!(second.estimate(10000), 2500);
        first.observe(1, Some(u64::MAX));
        assert_eq!(first.estimate(usize::MAX), u64::MAX);
        assert_eq!(ContextSettings::default().usable(1000), 900);
    }
}
