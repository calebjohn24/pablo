//! Child limits are projections of admitted parent limits, never fresh defaults.
use super::{ChildCeilings, MAX_CONTEXT_BYTES, MAX_INPUT_BYTES, MAX_RESULT_BYTES};
use crate::RunLimits;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidChildCeiling;

pub(super) fn narrower<T: PartialOrd>(parent: Option<T>, child: Option<T>) -> bool {
    match (parent, child) {
        (None, _) => true,
        (Some(parent), Some(child)) => child <= parent,
        (Some(_), None) => false,
    }
}

fn optional<T: Copy + PartialOrd>(
    parent: Option<T>,
    requested: Option<T>,
) -> Result<Option<T>, InvalidChildCeiling> {
    let selected = requested.or(parent);
    narrower(parent, selected)
        .then_some(selected)
        .ok_or(InvalidChildCeiling)
}

fn bounded<T: Copy + Ord>(parent: T, requested: Option<T>) -> Result<T, InvalidChildCeiling> {
    let selected = requested.unwrap_or(parent);
    (selected <= parent)
        .then_some(selected)
        .ok_or(InvalidChildCeiling)
}

fn decimal(value: &Option<String>) -> Result<Option<u64>, InvalidChildCeiling> {
    value
        .as_deref()
        .map(|s| {
            if s.is_empty()
                || !s.bytes().all(|b| b.is_ascii_digit())
                || (s.len() > 1 && s.starts_with('0'))
            {
                return Err(InvalidChildCeiling);
            }
            s.parse().map_err(|_| InvalidChildCeiling)
        })
        .transpose()
}

impl ChildCeilings {
    /// Inherit every unselected parent limit, with the child context/input/result
    /// bounds applied. An explicit widening fails instead of silently clamping.
    /// This grants no authority, reserves no capacity and does not reset the root
    /// deadline: supervision must still attach the resulting run to its ledger.
    pub fn apply_to(&self, parent: &RunLimits) -> Result<RunLimits, InvalidChildCeiling> {
        let mut child = parent.clone();
        child.max_model_calls = optional(parent.max_model_calls, self.max_model_calls)?;
        child.max_tool_calls = optional(parent.max_tool_calls, self.max_tool_calls)?;
        child.max_total_tokens =
            optional(parent.max_total_tokens, decimal(&self.max_total_tokens)?)?;
        child.max_cost_microusd =
            optional(parent.max_cost_microusd, decimal(&self.max_cost_microusd)?)?;
        child.max_run_duration_ms = bounded(parent.max_run_duration_ms, self.max_duration_ms)?;
        child.max_context_bytes = bounded(
            parent.max_context_bytes.min(MAX_CONTEXT_BYTES),
            self.max_context_bytes,
        )?;
        child.max_output_bytes = bounded(
            parent.max_output_bytes.min(MAX_RESULT_BYTES),
            self.max_output_bytes,
        )?;
        child.max_input_bytes = parent.max_input_bytes.min(MAX_INPUT_BYTES);
        Ok(child)
    }
}

/// Defense at the shared ledger boundary, including host-constructed RunLimits.
/// All fields are destructured so adding a limit requires an explicit decision.
pub(super) fn inherits(parent: &RunLimits, child: &RunLimits) -> bool {
    let RunLimits {
        max_model_calls,
        max_tool_calls,
        max_total_tokens,
        max_cost_microusd,
        filesystem,
        max_tool_duration_ms,
        max_tool_input_bytes,
        max_tool_output_bytes,
        max_context_bytes,
        max_run_duration_ms,
        max_input_bytes,
        max_output_bytes,
        max_output_tokens,
        max_events,
    } = child;
    let crate::filesystem::FilesystemLimits {
        max_file_bytes,
        max_entries,
        max_depth,
        max_scan_bytes,
    } = filesystem;
    narrower(parent.max_model_calls, *max_model_calls)
        && narrower(parent.max_tool_calls, *max_tool_calls)
        && narrower(parent.max_total_tokens, *max_total_tokens)
        && narrower(parent.max_cost_microusd, *max_cost_microusd)
        && narrower(parent.filesystem.max_file_bytes, *max_file_bytes)
        && narrower(parent.filesystem.max_entries, *max_entries)
        && narrower(parent.filesystem.max_depth, *max_depth)
        && narrower(parent.filesystem.max_scan_bytes, *max_scan_bytes)
        && *max_tool_duration_ms <= parent.max_tool_duration_ms
        && *max_tool_input_bytes <= parent.max_tool_input_bytes
        && *max_tool_output_bytes <= parent.max_tool_output_bytes
        && *max_context_bytes <= parent.max_context_bytes
        && *max_run_duration_ms <= parent.max_run_duration_ms
        && *max_input_bytes <= parent.max_input_bytes
        && *max_output_bytes <= parent.max_output_bytes
        && *max_output_tokens <= parent.max_output_tokens
        && *max_events <= parent.max_events
}

#[cfg(test)]
mod tests;
