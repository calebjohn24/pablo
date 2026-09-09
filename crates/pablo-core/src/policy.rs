//! Exact host-owned capability rules. This is policy, not environment containment.
use crate::PolicyRule;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DefaultDecision {
    #[default]
    Allow,
    Deny,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub value: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub default: DefaultDecision,
    #[serde(default)]
    pub allow: Vec<Rule>,
    #[serde(default)]
    pub deny: Vec<Rule>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub tools: Option<Rules>,
    pub executables: Option<Rules>,
    pub read_roots: Option<Rules>,
    pub write_roots: Option<Rules>,
}

/// An ordinary policy intersected with immutable host/deployment policies.
/// Missing dimensions in a ceiling add no constraint. Lists from separate
/// layers are never merged: every applicable layer must admit the operation.
#[derive(Clone, Debug, Default)]
pub struct PolicySet {
    ordinary: Policy,
    ceilings: Vec<Policy>,
}

impl From<Policy> for PolicySet {
    fn from(ordinary: Policy) -> Self {
        Self {
            ordinary,
            ceilings: Vec::new(),
        }
    }
}

impl PolicySet {
    pub(crate) fn rule_ids(&self) -> std::collections::HashSet<String> {
        std::iter::once(&self.ordinary)
            .chain(&self.ceilings)
            .flat_map(|p| p.dimensions().into_iter().filter_map(|(_, r)| r))
            .flat_map(|r| r.allow.iter().chain(&r.deny))
            .map(|r| r.id.clone())
            .collect()
    }
    pub fn new(ordinary: Policy, ceilings: Vec<Policy>) -> Result<Self, &'static str> {
        let set = Self { ordinary, ceilings };
        set.validate()?;
        Ok(set)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.ceilings.len() > 64 {
            return Err("too many authority policies");
        }
        let mut ids = HashSet::new();
        for policy in std::iter::once(&self.ordinary).chain(&self.ceilings) {
            policy.validate()?;
            for (_, rules) in policy.dimensions() {
                for rule in rules
                    .into_iter()
                    .flat_map(|r| r.allow.iter().chain(&r.deny))
                {
                    if !ids.insert(&rule.id) {
                        return Err("duplicate policy rule ID across layers");
                    }
                    if ids.len() > 1024 {
                        return Err("too many policy rules across layers");
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn dimensions(&self) -> impl Iterator<Item = (&'static str, Option<&Rules>)> {
        std::iter::once(&self.ordinary)
            .chain(&self.ceilings)
            .flat_map(Policy::dimensions)
    }

    pub(crate) fn decide(
        &self,
        dimension: &str,
        value: &str,
        write_opt_in: bool,
    ) -> Result<Vec<String>, PolicyRule> {
        let mut decisions = vec![self.ordinary.decide(dimension, value, write_opt_in)?];
        for ceiling in &self.ceilings {
            if ceiling
                .dimensions()
                .iter()
                .any(|(name, rules)| *name == dimension && rules.is_some())
            {
                decisions.push(ceiling.decide(dimension, value, write_opt_in)?);
            }
        }
        Ok(decisions)
    }
}
impl Policy {
    pub fn validate(&self) -> Result<(), &'static str> {
        let mut ids = HashSet::new();
        for (dimension, rules) in self.dimensions() {
            if let Some(rules) = rules {
                if rules.allow.len().saturating_add(rules.deny.len()) > 128 {
                    return Err("too many policy rules");
                }
                for rule in rules.allow.iter().chain(&rules.deny) {
                    if rule.id.is_empty()
                        || rule.id.len() > 64
                        || !rule
                            .id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                        || rule.id.starts_with("builtin.")
                        || !ids.insert(&rule.id)
                    {
                        return Err("invalid or duplicate policy rule ID");
                    }
                    if rule.value.is_empty() || rule.value.len() > 4096 || rule.value.contains('\0')
                    {
                        return Err("invalid policy value");
                    }
                    if dimension.ends_with("roots") {
                        relative_path(&rule.value).map_err(|_| "policy roots must be relative directories without parent traversal")?;
                    }
                    if dimension == "executables" && !Path::new(&rule.value).is_absolute() {
                        return Err("executable rules must be absolute");
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn dimensions(&self) -> [(&'static str, Option<&Rules>); 4] {
        [
            ("tools", self.tools.as_ref()),
            ("executables", self.executables.as_ref()),
            ("read_roots", self.read_roots.as_ref()),
            ("write_roots", self.write_roots.as_ref()),
        ]
    }
    pub(crate) fn decide(
        &self,
        dimension: &str,
        value: &str,
        write_opt_in: bool,
    ) -> Result<String, PolicyRule> {
        let rules = self
            .dimensions()
            .into_iter()
            .find(|(name, _)| *name == dimension)
            .and_then(|(_, r)| r);
        let matches = |rule: &&Rule| {
            if dimension.ends_with("roots") {
                relative_path(value)
                    .ok()
                    .zip(relative_path(&rule.value).ok())
                    .is_some_and(|(path, root)| path.starts_with(root))
            } else {
                rule.value == value
            }
        };
        let denied = |id: String| Err(PolicyRule::Configured { id: id.into() });
        if let Some(rules) = rules {
            if let Some(rule) = rules.deny.iter().find(matches) {
                return denied(rule.id.clone());
            }
            if !rules.allow.is_empty() {
                return rules
                    .allow
                    .iter()
                    .find(matches)
                    .map(|rule| Ok(rule.id.clone()))
                    .unwrap_or_else(|| denied(format!("builtin.{dimension}.allowlist_miss")));
            }
        }
        let default =
            rules
                .map(|r| r.default)
                .unwrap_or(if dimension == "write_roots" && !write_opt_in {
                    DefaultDecision::Deny
                } else {
                    DefaultDecision::Allow
                });
        if default == DefaultDecision::Deny {
            denied(format!("builtin.{dimension}.default_deny"))
        } else {
            Ok(format!("builtin.{dimension}.default_allow"))
        }
    }
}
pub(crate) fn relative_path(value: &str) -> Result<PathBuf, PolicyRule> {
    let mut path = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(name) => path.push(name),
            Component::CurDir => {}
            _ => return Err(PolicyRule::Workspace),
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_sets_validate_global_identity_and_layer_bounds() {
        let policy: Policy = serde_json::from_value(serde_json::json!({"tools":{"default":"allow","allow":[{"id":"shared","value":"fs.read"}]}})).unwrap();
        assert!(PolicySet::new(policy.clone(), vec![policy]).is_err());
        assert!(PolicySet::new(Policy::default(), vec![Policy::default(); 64]).is_ok());
        assert!(PolicySet::new(Policy::default(), vec![Policy::default(); 65]).is_err());
    }
    #[test]
    fn closed_configuration_and_component_matching_preserve_precedence() {
        for json in [
            r#"{"unknown":{}}"#,
            r#"{"tools":{"default":"allow","extra":true}}"#,
        ] {
            assert!(serde_json::from_str::<Policy>(json).is_err());
        }
        for value in ["../outside", "/outside"] {
            let p:Policy=serde_json::from_value(serde_json::json!({"read_roots":{"default":"allow","allow":[{"id":"root","value":value}]}})).unwrap();
            assert!(p.validate().is_err());
        }
        let p:Policy=serde_json::from_str(r#"{"read_roots":{"default":"allow","allow":[{"id":"allow.nested","value":"nested"}],"deny":[{"id":"deny.first","value":"nested/private"},{"id":"deny.second","value":"nested/private"}]}}"#).unwrap();
        p.validate().unwrap();
        assert_eq!(
            p.decide("read_roots", "nested/b.txt", false).unwrap(),
            "allow.nested"
        );
        for (path, id) in [
            ("nested/private/a", "deny.first"),
            ("nested-other/a", "builtin.read_roots.allowlist_miss"),
        ] {
            assert_eq!(
                p.decide("read_roots", path, false),
                Err(PolicyRule::Configured { id: id.into() })
            );
        }
        let p:Policy=serde_json::from_str(r#"{"tools":{"default":"allow","allow":[{"id":"same","value":"fs.read"}]},"executables":{"default":"allow","allow":[{"id":"same","value":"/bin/sh"}]}}"#).unwrap();
        assert!(p.validate().is_err());
    }
}
