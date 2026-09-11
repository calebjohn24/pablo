//! Offline, bounded deployment composition. Resolution never reads ambient
//! environment, credentials or workspace content and never activates a runtime.
//! The checked-in schema/defaults are the option inventory for this revision.

mod a2a;
mod admission;
mod canonical;
mod credentials;
mod input;
mod mcp;
mod render;
mod resolve;
mod routes;
mod shell;
mod skills;
mod validate;

use serde::Serialize;
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fmt, path::PathBuf};

pub use admission::{PreparedRun, RunInput};
pub use canonical::fingerprint;
pub use credentials::{
    CredentialConsumer, CredentialInputs, CredentialReadError, ProcessCredentials, ScopedCredential,
};
pub use resolve::{LoadedDeployment, load, resolve};
pub use routes::{ResolvedRoute, RouteEntry, RoutePolicy};

pub const CONTRACT_REVISION: &str = "c3.26";
pub const DOCUMENT_SCHEMA: &str =
    include_str!("../../../../docs/project/schemas/deployment-v1.schema.json");
pub const DEFAULT_OPTIONS: &str =
    include_str!("../../../../docs/project/schemas/deployment-defaults-v1.json");
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 8 * MAX_FILE_BYTES;
pub const MAX_NODES: usize = 65_536;
pub const MAX_DEPTH: usize = 32;
pub const MAX_FILES: usize = 64;
pub const MAX_IMPORT_DEPTH: usize = 16;
pub const MAX_IMPORT_EDGES: usize = 128;
pub const MAX_ORIGINS: usize = 65_536;
pub const MAX_OUTPUT_BYTES: usize = 8 * MAX_FILE_BYTES;

/// An explicit host-authored document. In-memory documents have no source path;
/// use config/workspace/binding paths instead. Neither form is executable.
pub enum ConfigInput {
    File(PathBuf),
    Document(Value),
}

/// All inputs are explicit. A caller must never fill `environment` by copying
/// the process environment wholesale. Values are only consulted for declarations.
pub struct ResolveRequest {
    pub config_root: PathBuf,
    pub entry: ConfigInput,
    pub user_config: Option<ConfigInput>,
    pub workspace_config: Option<ConfigInput>,
    pub profile: Option<String>,
    pub path_bindings: BTreeMap<String, PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub overrides: Map<String, Value>,
    pub host_authority: Vec<Value>,
    pub locked: bool,
}

impl ResolveRequest {
    pub fn new(config_root: PathBuf, entry: impl Into<PathBuf>) -> Self {
        Self {
            config_root,
            entry: ConfigInput::File(entry.into()),
            user_config: None,
            workspace_config: None,
            profile: None,
            path_bindings: BTreeMap::new(),
            environment: BTreeMap::new(),
            overrides: Map::new(),
            host_authority: Vec::new(),
            locked: false,
        }
    }
}

#[derive(Clone, Serialize, PartialEq, Eq)]
pub struct ConfigError {
    pub code: &'static str,
    pub option: Box<str>,
    pub source: Option<Box<str>>,
    pub line: Option<usize>,
    pub owner: Option<&'static str>,
    pub authority_id: Option<Box<str>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<String>,
}

impl ConfigError {
    pub(crate) fn new(code: &'static str, option: &str) -> Self {
        Self {
            code,
            option: safe(option, 384).into(),
            source: None,
            line: None,
            owner: None,
            authority_id: None,
            chain: Vec::new(),
        }
    }
    fn at(mut self, source: &str) -> Self {
        self.source = Some(safe(source, 256).into());
        self
    }
    fn with_chain<'a>(mut self, names: impl IntoIterator<Item = &'a str>) -> Self {
        self.chain = names
            .into_iter()
            .take(MAX_IMPORT_DEPTH + 1)
            .map(|name| safe(name, 16))
            .collect();
        self
    }
}

fn safe(value: &str, bound: usize) -> String {
    let mut out = String::new();
    for c in value.chars() {
        let c = if c.is_control() { '?' } else { c };
        if out.len() + c.len_utf8() > bound {
            break;
        }
        out.push(c);
    }
    out
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.code, self.option)?;
        if let Some(source) = &self.source {
            write!(f, " in {source}")?;
        }
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        if let Some(owner) = self.owner {
            write!(f, " (requires {owner})")?;
        }
        if let Some(id) = &self.authority_id {
            write!(f, " (authority {id})")?;
        }
        if !self.chain.is_empty() {
            write!(f, " [{}]", self.chain.join(" -> "))?;
        }
        Ok(())
    }
}
impl fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for ConfigError {}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub kind: &'static str,
    pub locator: String,
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Origin {
    pub source: String,
    pub operation: &'static str,
}

/// Read-only, schema-validated snapshot. Explicit serialization is for local
/// inspection; Debug and runtime summaries must not dump operator content.
#[derive(Clone, Serialize)]
pub struct ResolvedDeployment {
    #[serde(skip)]
    config_root: PathBuf,
    #[serde(skip)]
    path_bindings: BTreeMap<String, PathBuf>,
    schema_version: u32,
    contract_revision: &'static str,
    config: Value,
    fingerprint: String,
    sources: Vec<Source>,
    provenance: BTreeMap<String, Vec<Origin>>,
    input_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_route: Option<ResolvedRoute>,
}

/// Safe run metadata. It intentionally excludes sources, paths, task content,
/// credential presence and the private host-bindings fingerprint.
#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DeploymentIdentity {
    schema_version: u32,
    contract_revision: String,
    fingerprint: String,
}
impl ResolvedDeployment {
    pub fn model_route(&self) -> Option<&ResolvedRoute> {
        self.model_route.as_ref()
    }
    pub(super) fn selected_model(&self) -> &Value {
        if let Some(route) = &self.model_route {
            &self.options()["models"][route.entries()[0].name()]
        } else {
            &self.options()["model"]
        }
    }
    pub fn identity(&self) -> DeploymentIdentity {
        DeploymentIdentity {
            schema_version: self.schema_version,
            contract_revision: self.contract_revision.into(),
            fingerprint: self.fingerprint.clone(),
        }
    }
    /// Canonical, portable TOML with all defaults and secret references retained.
    /// This is explicit local inspection and may contain operator-authored text.
    pub fn render(&self) -> Result<String, ConfigError> {
        render::render(&self.config)
    }
    pub fn config(&self) -> &Value {
        &self.config
    }
    pub fn options(&self) -> &Value {
        &self.config["options"]
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    pub fn input_fingerprint(&self) -> &str {
        &self.input_fingerprint
    }
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }
    pub fn provenance(&self) -> &BTreeMap<String, Vec<Origin>> {
        &self.provenance
    }
}
impl fmt::Debug for ResolvedDeployment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedDeployment")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

fn error(code: &'static str, option: &str) -> ConfigError {
    ConfigError::new(code, option)
}
fn limit() -> ConfigError {
    error("config_limit", "/")
}
fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}
