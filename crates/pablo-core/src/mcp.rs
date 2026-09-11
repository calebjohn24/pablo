//! Host-owned MCP admission and bounded transport components.
#[cfg(unix)]
pub mod stdio;
#[cfg(unix)]
pub(crate) mod tool;
#[cfg(unix)]
pub use tool::McpResult;
mod transport;
use crate::{
    PolicyRule,
    policy::{DefaultDecision, Policy as ToolPolicy, PolicySet, Rules},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

pub const SDK_VERSION: &str = "3.3.0";
pub const SDK_COMMIT: &str = "3e636cab26c013eca5131103c03d20237f12c4df";
pub const PROTOCOL_VERSION: &str = "2025-11-25";
pub const OTEL_REVISION: &str = "fee465db333bdd6a7d2faa320edab5cf3101a4f4";
pub const MAX_SERVERS: usize = 16;
pub const MAX_TOOLS: usize = 256;
pub const MAX_SCHEMA_BYTES: usize = 65536;
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
fn required() -> bool {
    true
}
fn startup() -> u64 {
    30000
}
fn operation() -> u64 {
    60000
}
fn cwd() -> serde_json::Value {
    serde_json::json!({"base":"workspace","path":"."})
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum Server {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "cwd")]
        cwd: serde_json::Value,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default = "required")]
        required: bool,
        #[serde(default = "startup")]
        startup_timeout_ms: u64,
        #[serde(default = "operation")]
        operation_timeout_ms: u64,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default = "required")]
        required: bool,
        #[serde(default = "startup")]
        startup_timeout_ms: u64,
        #[serde(default = "operation")]
        operation_timeout_ms: u64,
    },
}
impl Server {
    pub fn required(&self) -> bool {
        match self {
            Self::Stdio { required, .. } | Self::Http { required, .. } => *required,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        let (start, op) = match self {
            Self::Stdio {
                startup_timeout_ms,
                operation_timeout_ms,
                ..
            }
            | Self::Http {
                startup_timeout_ms,
                operation_timeout_ms,
                ..
            } => (*startup_timeout_ms, *operation_timeout_ms),
        };
        if !(1000..=120000).contains(&start) || !(1000..=900000).contains(&op) {
            return Err("mcp_timeout");
        }
        match self {
            Self::Stdio {
                command,
                args,
                cwd,
                env,
                ..
            } => {
                if command.split('/').any(|p| matches!(p, "." | ".."))
                    || !Path::new(command).is_absolute()
                    || command.len() > 4096
                    || command.contains('\0')
                    || args.len() > 64
                    || args.iter().any(|s| s.len() > 4096 || s.contains('\0'))
                    || args.iter().map(String::len).sum::<usize>() > 65536
                {
                    return Err("mcp_launcher");
                }
                validate_cwd(cwd)?;
                if env.len() > 32
                    || env
                        .iter()
                        .any(|(k, v)| !environment_name(k) || !name(v, 64))
                {
                    return Err("mcp_environment");
                }
            }
            Self::Http { url, headers, .. } => {
                let parsed = reqwest::Url::parse(url).map_err(|_| "mcp_endpoint")?;
                if url.len() > 4096
                    || url.chars().any(|c| c.is_control() || c.is_whitespace())
                    || parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err("mcp_endpoint");
                }
                if headers.len() > 16
                    || headers.iter().any(|(k, v)| !header_name(k) || !name(v, 64))
                {
                    return Err("mcp_headers");
                }
            }
        }
        Ok(())
    }
    pub fn credentials(&self) -> impl Iterator<Item = (&str, &str, &'static str)> {
        let (values, consumer) = match self {
            Self::Stdio { env, .. } => (env, "mcp.env"),
            Self::Http { headers, .. } => (headers, "mcp.headers"),
        };
        values
            .iter()
            .map(move |(k, v)| (k.as_str(), v.as_str(), consumer))
    }
}
fn validate_cwd(value: &serde_json::Value) -> Result<(), &'static str> {
    let map = value.as_object().ok_or("mcp_cwd")?;
    let base = value["base"].as_str().ok_or("mcp_cwd")?;
    let path = value["path"].as_str().ok_or("mcp_cwd")?;
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\\')
        || path.contains('\0')
        || Path::new(path).is_absolute()
        || path.split('/').any(|p| p == "..")
        || !matches!(base, "config" | "workspace" | "binding")
        || map
            .keys()
            .any(|k| !matches!(k.as_str(), "base" | "path" | "name"))
    {
        return Err("mcp_cwd");
    }
    if (base == "binding" && !value["name"].as_str().is_some_and(|n| name(n, 64)))
        || (base != "binding" && map.contains_key("name"))
    {
        return Err("mcp_cwd");
    }
    Ok(())
}
fn environment_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
}
fn header_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !matches!(
            s,
            "host"
                | "content-length"
                | "content-type"
                | "accept"
                | "connection"
                | "transfer-encoding"
                | "upgrade"
                | "traceparent"
                | "tracestate"
                | "baggage"
        )
        && !s.starts_with("mcp-")
}
pub fn name(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub servers: Option<Rules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Rules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launchers: Option<Rules>,
}
impl Policy {
    fn dimensions(&self) -> [(&'static str, Option<&Rules>); 3] {
        [
            ("servers", self.servers.as_ref()),
            ("tools", self.tools.as_ref()),
            ("launchers", self.launchers.as_ref()),
        ]
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub servers: BTreeMap<String, Server>,
    pub policies: Vec<Policy>,
}
impl Settings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.servers.len() > MAX_SERVERS
            || self.policies.len() > 16
            || !crate::filesystem::fits(self, 1024 * 1024)
        {
            return Err("mcp_configuration_bound");
        }
        for (id, server) in &self.servers {
            if !name(id, 32) {
                return Err("mcp_server_name");
            }
            server.validate()?;
        }
        let mut ids = HashSet::new();
        for p in &self.policies {
            for (dimension, rules) in p.dimensions() {
                if let Some(r) = rules {
                    ToolPolicy {
                        tools: Some(r.clone()),
                        ..Default::default()
                    }
                    .validate()?;
                    for rule in r.allow.iter().chain(&r.deny) {
                        if !ids.insert(&rule.id) || ids.len() > 1024 {
                            return Err("mcp_rule_identity");
                        }
                        let valid = match dimension {
                            "servers" => name(&rule.value, 32),
                            "tools" => split_identity(&rule.value).is_some(),
                            _ => Path::new(&rule.value).is_absolute(),
                        };
                        if !valid {
                            return Err("mcp_rule_value");
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn decide(&self, dimension: &str, value: &str) -> Result<Vec<String>, PolicyRule> {
        let denied = |id: String| Err(PolicyRule::Configured { id: id.into() });
        if !matches!(dimension, "servers" | "tools" | "launchers") {
            return denied("builtin.mcp.unknown_dimension".into());
        }
        let rules: Vec<_> = self
            .policies
            .iter()
            .flat_map(Policy::dimensions)
            .filter_map(|(d, r)| (d == dimension).then_some(r).flatten())
            .collect();
        for r in &rules {
            if let Some(rule) = r.deny.iter().find(|r| r.value == value) {
                return denied(rule.id.clone());
            }
        }
        let mut decisions = Vec::new();
        for r in rules {
            if !r.allow.is_empty() {
                let Some(rule) = r.allow.iter().find(|r| r.value == value) else {
                    return denied(format!("builtin.mcp.{dimension}.allowlist_miss"));
                };
                decisions.push(rule.id.clone());
            } else if r.default == DefaultDecision::Deny {
                return denied(format!("builtin.mcp.{dimension}.default_deny"));
            }
        }
        if decisions.is_empty() {
            decisions.push(format!("builtin.mcp.{dimension}.default_allow"));
        }
        Ok(decisions)
    }
    pub fn admit_server(
        &self,
        id: &str,
        executables: &PolicySet,
    ) -> Result<Vec<String>, PolicyRule> {
        self.validate().map_err(|_| PolicyRule::Configured {
            id: "builtin.mcp.invalid_configuration".into(),
        })?;
        let Some(server) = self.servers.get(id) else {
            return Err(PolicyRule::Configured {
                id: "builtin.mcp.unconfigured_server".into(),
            });
        };
        let mut decisions = self.decide("servers", id)?;
        if let Server::Stdio { command, .. } = server {
            decisions.extend(self.decide("launchers", command)?);
            decisions.extend(executables.decide("executables", command, false)?);
        }
        Ok(decisions)
    }
    pub fn admit_tool(
        &self,
        server: &str,
        tool: &str,
        executables: &PolicySet,
    ) -> Result<Vec<String>, PolicyRule> {
        let mut decisions = self.admit_server(server, executables)?;
        let id = qualified(server, tool).map_err(|_| PolicyRule::Configured {
            id: "builtin.mcp.tool_identity".into(),
        })?;
        decisions.extend(self.decide("tools", &id)?);
        decisions.extend(executables.decide("tools", &id, false)?);
        Ok(decisions)
    }
    /// Normalize ACP inputs first; this only selects immutable host definitions.
    pub fn admit_client(
        &self,
        requests: &[ClientServer],
        executables: &PolicySet,
    ) -> Result<Vec<String>, &'static str> {
        self.validate()?;
        if requests.len() > MAX_SERVERS {
            return Err("mcp_configuration_bound");
        }
        let mut seen = HashSet::new();
        let mut selected = Vec::new();
        for request in requests {
            if !seen.insert(&request.name) {
                return Err("mcp_duplicate_server");
            }
            let server = self
                .servers
                .get(&request.name)
                .ok_or("mcp_unconfigured_server")?;
            let equal = match (server, &request.transport) {
                (
                    Server::Stdio { command, args, .. },
                    ClientTransport::Stdio {
                        command: c,
                        args: a,
                        env,
                    },
                ) => command == c && args == a && env.is_empty(),
                (Server::Http { url, .. }, ClientTransport::Http { url: u, headers }) => {
                    url == u && headers.is_empty()
                }
                _ => false,
            };
            if !equal {
                return Err("mcp_client_authority");
            }
            self.admit_server(&request.name, executables)
                .map_err(|_| "mcp_policy_denied")?;
            selected.push(request.name.clone());
        }
        Ok(selected)
    }
}
#[derive(Clone, Debug)]
pub struct ClientServer {
    pub name: String,
    pub transport: ClientTransport,
}
#[derive(Clone, Debug)]
pub enum ClientTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}
pub fn qualified(server: &str, tool: &str) -> Result<String, &'static str> {
    if !name(server, 32) || !name(tool, 128) {
        return Err("mcp_tool_identity");
    }
    Ok(format!("mcp/{server}/{tool}"))
}
pub(crate) fn split_identity(s: &str) -> Option<(&str, &str)> {
    let mut parts = s.split('/');
    if parts.next() != Some("mcp") {
        return None;
    }
    let server = parts.next()?;
    let tool = parts.next()?;
    (parts.next().is_none() && name(server, 32) && name(tool, 128)).then_some((server, tool))
}
pub fn provider_alias(identity: &str) -> Result<String, &'static str> {
    split_identity(identity).ok_or("mcp_tool_identity")?;
    let hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, identity.as_bytes());
    Ok(format!(
        "mcp_{}",
        hash.as_ref()[..24]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
#[derive(Default)]
pub struct Identities {
    by_alias: BTreeMap<String, String>,
    by_id: BTreeMap<String, String>,
}
impl Identities {
    pub fn insert(
        &mut self,
        server: &str,
        tool: &str,
        reserved: &HashSet<String>,
    ) -> Result<String, &'static str> {
        let id = qualified(server, tool)?;
        if self.by_id.len() >= MAX_TOOLS {
            return Err("mcp_catalog_bound");
        }
        let alias = provider_alias(&id)?;
        if self.by_id.contains_key(&id)
            || self.by_alias.contains_key(&alias)
            || reserved.contains(&alias)
        {
            return Err("mcp_identity_collision");
        }
        self.by_alias.insert(alias.clone(), id.clone());
        self.by_id.insert(id, alias.clone());
        Ok(alias)
    }
    pub fn canonical(&self, alias: &str) -> Option<&str> {
        self.by_alias.get(alias).map(String::as_str)
    }
}
