//! Shell settings projection used both by offline validation and run admission.
use super::*;
use crate::shell_policy::{ShellRestriction, ShellSettings};
use std::collections::HashSet;

fn diagnostic(message: &str) -> ConfigError {
    let code = if message.contains("too many") {
        "config_limit"
    } else if message.contains("duplicate") {
        "config_conflict"
    } else {
        "config_invalid_value"
    };
    error(code, "/options/shell")
}
fn raw(mut value: Value) -> Value {
    if let Some(map) = value.as_object_mut() {
        map.remove("enabled");
    }
    // Source cwd lists may use the existing list operations. Validate their
    // items intrinsically here; full composition is validated again afterward.
    for list in ["allow", "deny"] {
        if let Some(items) = value.pointer(&format!("/cwd_roots/{list}/items")).cloned() {
            value["cwd_roots"][list] = items;
        }
    }
    value
}
pub(crate) fn settings(value: &Value) -> Result<ShellSettings, ConfigError> {
    serde_json::from_value(raw(value.clone())).map_err(|_| diagnostic("invalid shell settings"))
}
pub(crate) fn restriction(value: &Value) -> Result<ShellRestriction, ConfigError> {
    serde_json::from_value(raw(value.clone())).map_err(|_| diagnostic("invalid shell restriction"))
}
pub(crate) fn ceilings(config: &Value) -> Result<Vec<ShellRestriction>, ConfigError> {
    config["authority"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|layer| layer.get("shell"))
        .map(restriction)
        .collect()
}
pub(crate) fn validate(config: &Value, ids: &mut HashSet<String>) -> Result<(), ConfigError> {
    let settings = settings(&config["options"]["shell"])?;
    let ceilings = ceilings(config)?;
    crate::shell_policy::validate(&settings, &ceilings, ids).map_err(diagnostic)
}
pub(crate) fn declaration(value: &Value) -> Result<(), ConfigError> {
    crate::shell_policy::validate(&settings(value)?, &[], &mut HashSet::new()).map_err(diagnostic)
}
pub(crate) fn authority(value: &Value) -> Result<(), ConfigError> {
    crate::shell_policy::validate(
        &ShellSettings::default(),
        &[restriction(value)?],
        &mut HashSet::new(),
    )
    .map_err(diagnostic)
}
