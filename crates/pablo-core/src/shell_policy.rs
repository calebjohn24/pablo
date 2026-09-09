//! Literal executable/argv restrictions, independent of the fixed shell launcher.
use crate::{
    PolicyRule,
    policy::{DefaultDecision, Policy, Rules},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentMatch {
    Exact,
    Prefix,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRule {
    pub id: String,
    pub executable: String,
    pub args: Vec<String>,
    pub r#match: ArgumentMatch,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRules {
    pub default: DefaultDecision,
    #[serde(default)]
    pub allow: Vec<CommandRule>,
    #[serde(default)]
    pub deny: Vec<CommandRule>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellEnvironment {
    pub values: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_names: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnvironmentRestriction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_names: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<CommandRules>,
    pub environment: ShellEnvironment,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd_roots: Option<Rules>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellRestriction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<CommandRules>,
    pub environment: EnvironmentRestriction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd_roots: Option<Rules>,
}

fn valid_name(name: &str) -> bool {
    name.starts_with("PABLO_TASK_")
        && name.len() > 11
        && name.len() <= 128
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn rule_id(id: &str, ids: &mut HashSet<String>) -> Result<(), &'static str> {
    if id.is_empty()
        || id.len() > 64
        || id.starts_with("builtin.")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err("invalid shell rule ID");
    }
    if !ids.insert(id.into()) {
        return Err("duplicate shell rule ID");
    }
    if ids.len() > 1024 {
        return Err("too many policy rules");
    }
    Ok(())
}
pub(crate) fn validate(
    settings: &ShellSettings,
    ceilings: &[ShellRestriction],
    ids: &mut HashSet<String>,
) -> Result<(), &'static str> {
    if ceilings.len() > 64 {
        return Err("too many shell authority layers");
    }
    let env = &settings.environment;
    if env.values.len() > 32
        || env
            .values
            .iter()
            .any(|(name, value)| !valid_name(name) || value.len() > 8192 || value.contains('\0'))
    {
        return Err("invalid shell environment defaults");
    }
    for names in std::iter::once(env.allowed_names.as_ref())
        .chain(
            ceilings
                .iter()
                .map(|c| c.environment.allowed_names.as_ref()),
        )
        .flatten()
    {
        if names.len() > 32
            || names.iter().any(|n| !valid_name(n))
            || names.iter().collect::<HashSet<_>>().len() != names.len()
            || env.values.keys().any(|name| !names.contains(name))
        {
            return Err("invalid shell environment restriction");
        }
    }
    for roots in std::iter::once(settings.cwd_roots.as_ref())
        .chain(ceilings.iter().map(|c| c.cwd_roots.as_ref()))
        .flatten()
    {
        Policy {
            read_roots: Some(roots.clone()),
            ..Policy::default()
        }
        .validate()?;
        for r in roots.allow.iter().chain(&roots.deny) {
            rule_id(&r.id, ids)?;
        }
    }
    for rules in std::iter::once(settings.commands.as_ref())
        .chain(ceilings.iter().map(|c| c.commands.as_ref()))
        .flatten()
    {
        if rules.allow.len().saturating_add(rules.deny.len()) > 128 {
            return Err("too many shell command rules");
        }
        for rule in rules.allow.iter().chain(&rules.deny) {
            rule_id(&rule.id, ids)?;
            if !Path::new(&rule.executable).is_absolute()
                || rule.executable.len() > 4096
                || rule.executable.chars().any(char::is_control)
                || rule.args.len() > 256
                || rule
                    .args
                    .iter()
                    .any(|a| a.len() > 8192 || a.chars().any(char::is_control))
                || rule.args.iter().map(String::len).sum::<usize>() > 65536
            {
                return Err("invalid shell command rule");
            }
        }
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct ShellPolicy {
    settings: ShellSettings,
    ceilings: Vec<ShellRestriction>,
    configured: bool,
}
pub(crate) struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
}
pub(crate) struct Admission {
    pub invocation: Option<Invocation>,
    pub decisions: Vec<String>,
}
pub(crate) struct Denial {
    pub rule: PolicyRule,
    pub decisions: Vec<String>,
}
fn denied(id: &str) -> PolicyRule {
    PolicyRule::Configured { id: id.into() }
}
impl ShellPolicy {
    pub(crate) fn new(
        settings: ShellSettings,
        ceilings: Vec<ShellRestriction>,
    ) -> Result<Self, &'static str> {
        validate(&settings, &ceilings, &mut HashSet::new())?;
        Ok(Self {
            settings,
            ceilings,
            configured: true,
        })
    }
    pub(crate) fn restricted(&self) -> bool {
        self.settings.commands.is_some() || self.ceilings.iter().any(|c| c.commands.is_some())
    }
    pub(crate) fn admit(
        &self,
        text: &str,
        cwd: &Path,
        env: &mut BTreeMap<String, String>,
        prior: &[String],
    ) -> Result<Admission, Denial> {
        let mut decisions = prior.to_vec();
        let result = (|| {
            for roots in std::iter::once(self.settings.cwd_roots.as_ref())
                .chain(self.ceilings.iter().map(|c| c.cwd_roots.as_ref()))
                .flatten()
            {
                let policy = Policy {
                    read_roots: Some(roots.clone()),
                    ..Policy::default()
                };
                let id = policy
                    .decide(
                        "read_roots",
                        cwd.to_str()
                            .ok_or_else(|| denied("builtin.shell.cwd_roots.path"))?,
                        false,
                    )
                    .map_err(|r| match r {
                        PolicyRule::Configured { id } if id.starts_with("builtin.read_roots.") => {
                            denied(&id.replace("builtin.read_roots.", "builtin.shell.cwd_roots."))
                        }
                        r => r,
                    })?;
                decisions.push(id.replace("builtin.read_roots.", "builtin.shell.cwd_roots."));
            }
            let mut effective = self.settings.environment.values.clone();
            effective.append(env);
            if effective.len() > 32 {
                return Err(denied("builtin.shell.environment.capacity"));
            }
            if self.configured
                && effective.iter().any(|(name, value)| {
                    !valid_name(name) || value.len() > 8192 || value.contains('\0')
                })
            {
                return Err(denied("builtin.shell.environment.value"));
            }
            for names in std::iter::once(self.settings.environment.allowed_names.as_ref())
                .chain(
                    self.ceilings
                        .iter()
                        .map(|c| c.environment.allowed_names.as_ref()),
                )
                .flatten()
            {
                if effective.keys().any(|name| !names.contains(name)) {
                    return Err(denied("builtin.shell.environment.names"));
                }
                decisions.push("builtin.shell.environment.allowed".into());
            }
            *env = effective;
            if !self.restricted() {
                return Ok(None);
            }
            let mut words = parse(text).map_err(|_| denied("builtin.shell.commands.syntax"))?;
            let program = words.remove(0);
            let executable = resolve_program(&program)
                .ok_or_else(|| denied("builtin.shell.commands.executable"))?;
            for rules in std::iter::once(self.settings.commands.as_ref())
                .chain(self.ceilings.iter().map(|c| c.commands.as_ref()))
                .flatten()
            {
                let matches = |rule: &&CommandRule| {
                    let argv = match rule.r#match {
                        ArgumentMatch::Exact => words == rule.args,
                        ArgumentMatch::Prefix => words.starts_with(&rule.args),
                    };
                    argv && identity(Path::new(&rule.executable))
                        .is_some_and(|rule| rule.same(&executable))
                };
                if let Some(rule) = rules.deny.iter().find(matches) {
                    return Err(denied(&rule.id));
                }
                if !rules.allow.is_empty() {
                    let rule = rules
                        .allow
                        .iter()
                        .find(matches)
                        .ok_or_else(|| denied("builtin.shell.commands.allowlist_miss"))?;
                    decisions.push(rule.id.clone());
                } else if rules.default == DefaultDecision::Deny {
                    return Err(denied("builtin.shell.commands.default_deny"));
                } else {
                    decisions.push("builtin.shell.commands.default_allow".into());
                }
            }
            Ok(Some(Invocation {
                program: executable.path,
                args: words,
            }))
        })();
        match result {
            Ok(invocation) => Ok(Admission {
                invocation,
                decisions,
            }),
            Err(rule) => Err(Denial { rule, decisions }),
        }
    }
}
struct Identity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl Identity {
    fn same(&self, other: &Self) -> bool {
        if self.path == other.path {
            return true;
        }
        #[cfg(unix)]
        {
            self.device == other.device && self.inode == other.inode
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}
fn identity(path: &Path) -> Option<Identity> {
    let path = path.canonicalize().ok()?;
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o111 == 0 {
            return None;
        }
        Some(Identity {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        Some(Identity { path })
    }
}
fn resolve_program(program: &str) -> Option<Identity> {
    if program.is_empty() {
        return None;
    }
    if Path::new(program).is_absolute() {
        return identity(Path::new(program));
    }
    if program.contains('/') || program.contains('=') {
        return None;
    }
    ["/usr/bin", "/bin"]
        .into_iter()
        .find_map(|dir| identity(&Path::new(dir).join(program)))
}

/// Parse a deliberately small literal subset; execution never reparses these words.
fn parse(text: &str) -> Result<Vec<String>, ()> {
    if text.len() > 65536 || text.chars().any(|c| c.is_control() && c != '\t') {
        return Err(());
    }
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else if c.is_control() {
                    return Err(());
                } else {
                    word.push(c);
                }
            }
            Some('"') => match c {
                '"' => quote = None,
                '\\' => {
                    let next = chars.next().ok_or(())?;
                    if !matches!(next, '"' | '\\' | '$' | '`') {
                        return Err(());
                    }
                    word.push(next);
                }
                '$' | '`' => return Err(()),
                c if c.is_control() => return Err(()),
                c => word.push(c),
            },
            None => match c {
                ' ' | '\t' => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                '\'' | '"' => {
                    quote = Some(c);
                    started = true;
                }
                '\\' => {
                    let next = chars.next().ok_or(())?;
                    if next.is_control() {
                        return Err(());
                    }
                    word.push(next);
                    started = true;
                }
                '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '*' | '?' | '[' | ']'
                | '{' | '}' | '~' | '#' => return Err(()),
                c => {
                    word.push(c);
                    started = true;
                }
            },
            _ => unreachable!(),
        }
        if word.len() > 8192 || words.len() > 257 {
            return Err(());
        }
    }
    if quote.is_some() {
        return Err(());
    }
    if started {
        words.push(word);
    }
    if words.is_empty() || words.len() > 257 {
        return Err(());
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_and_typed_policy_accept_exact_capacity_and_reject_one_more() {
        assert!(parse(&format!("printf {}", "é".repeat(4096))).is_ok());
        assert!(parse(&format!("printf {}x", "é".repeat(4096))).is_err());
        assert_eq!(
            parse(&format!("printf{}", " ''".repeat(256)))
                .unwrap()
                .len(),
            257
        );
        assert!(parse(&format!("printf{}", " ''".repeat(257))).is_err());
        let text = format!("printf{}", " ".repeat(65530));
        assert!(parse(&text).is_ok());
        assert!(parse(&(text + " ")).is_err());
        let rules = |layer| CommandRules {
            default: DefaultDecision::Allow,
            deny: vec![],
            allow: (0..128)
                .map(|i| CommandRule {
                    id: format!("rule.{layer}.{i}"),
                    executable: "/usr/bin/printf".into(),
                    args: vec!["a".into(); 256],
                    r#match: ArgumentMatch::Prefix,
                })
                .collect(),
        };
        let mut settings = ShellSettings {
            commands: Some(rules(0)),
            ..Default::default()
        };
        let mut ceilings: Vec<_> = (1..8)
            .map(|i| ShellRestriction {
                commands: Some(rules(i)),
                ..Default::default()
            })
            .collect();
        assert!(validate(&settings, &ceilings, &mut HashSet::new()).is_ok());
        ceilings.push(ShellRestriction {
            commands: Some(CommandRules {
                default: DefaultDecision::Allow,
                deny: vec![],
                allow: vec![rules(9).allow.remove(0)],
            }),
            ..Default::default()
        });
        assert_eq!(
            validate(&settings, &ceilings, &mut HashSet::new()),
            Err("too many policy rules")
        );
        settings
            .commands
            .as_mut()
            .unwrap()
            .allow
            .push(rules(9).allow.remove(0));
        assert_eq!(
            validate(&settings, &[], &mut HashSet::new()),
            Err("too many shell command rules")
        );
        settings.commands.as_mut().unwrap().allow = vec![rules(0).allow.remove(0)];
        settings.commands.as_mut().unwrap().allow[0]
            .args
            .push("extra".into());
        assert!(validate(&settings, &[], &mut HashSet::new()).is_err());
    }
    #[test]
    fn literal_quoting_keeps_argument_boundaries_and_rejects_shell_evaluation() {
        assert_eq!(
            parse("printf '%s' a\\ b \"c d\" '' ab'cd' '$HOME;*' \"\\$HOME\"").unwrap(),
            ["printf", "%s", "a b", "c d", "", "abcd", "$HOME;*", "$HOME"]
        );
        for text in [
            "",
            "  ",
            "git status; touch bad",
            "git status | cat",
            "git $(touch bad)",
            "git `id`",
            "git \"$HOME\"",
            "git >out",
            "git *",
            "git #x",
            "git\nstatus",
            "git 'oops",
            "git \\",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert!(parse(&format!("git {}", "x".repeat(8193))).is_err());
        assert!(parse(&format!("git{}", " x".repeat(257))).is_err());
    }
}
