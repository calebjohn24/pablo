//! Secrets are resolved only at use, never by config loading or inspection.
use super::*;

#[cfg(test)]
mod mcp_tests {
    use super::*;
    struct Inputs;
    impl CredentialInputs for Inputs {
        fn environment(&self, _: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
            panic!("ambient credential lookup")
        }
        fn host(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
            assert_eq!(name, "synthetic");
            Ok(Some(b"synthetic-http-token".to_vec()))
        }
    }
    #[test]
    fn mcp_header_credentials_are_sensitive_and_bound_to_exact_definition() {
        let cwd = std::env::current_dir().unwrap();
        let mut request = ResolveRequest::new(cwd.clone(), "unused");
        request.path_bindings.insert("workspace".into(), cwd);
        request.entry = ConfigInput::Document(
            serde_json::json!({"schema_version":1,"options":{"mcp":{"servers":{"remote":{"transport":"http","url":"https://synthetic.example/mcp","headers":{"x-fixture-token":"token"}}}}},"credentials":{"gateway":{"consumer":"provider.vercel","sources":[{"kind":"host","name":"unused"}]},"token":{"consumer":"mcp.headers","sources":[{"kind":"host","name":"synthetic"}]}}}),
        );
        let prepared = resolve(request)
            .unwrap()
            .prepare_run(RunInput {
                input: "synthetic".into(),
                workspace: None,
                session_id: None,
            })
            .unwrap();
        let server = &prepared.deployment().mcp().unwrap().servers["remote"];
        let headers = prepared
            .mcp_headers("remote", server, "https://synthetic.example/mcp", &Inputs)
            .unwrap();
        assert!(headers["x-fixture-token"].is_sensitive());
        let destination = serde_json::json!(["remote", server, "x-fixture-token"]).to_string();
        let credential = prepared
            .resolve_credential(
                CredentialConsumer::McpHeaders,
                &Value::String("token".into()),
                &destination,
                &Inputs,
            )
            .unwrap()
            .unwrap();
        assert!(
            credential
                .expose_for(
                    CredentialConsumer::McpHeaders,
                    "https://different.example/mcp"
                )
                .is_err()
        );
        assert!(
            credential
                .expose_for(CredentialConsumer::McpEnvironment, &destination)
                .is_err()
        );
        assert!(
            prepared
                .credential(CredentialConsumer::McpHeaders, &Inputs)
                .is_err()
        );
    }
}
use std::{
    collections::HashSet,
    path::{Component, Path},
};

pub const MAX_CREDENTIAL_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialConsumer {
    Vercel,
    OpenRouter,
    OpenResponses,
    OtelHeaders,
    McpEnvironment,
    McpHeaders,
    A2aBearer,
}
impl CredentialConsumer {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Vercel => "provider.vercel",
            Self::OpenRouter => "provider.openrouter",
            Self::OpenResponses => "provider.open_responses",
            Self::OtelHeaders => "otel.headers",
            Self::McpEnvironment => "mcp.env",
            Self::McpHeaders => "mcp.headers",
            Self::A2aBearer => "a2a.bearer",
        }
    }
}

/// Deliberately carries no underlying parser, OS, host or secret value.
#[derive(Debug)]
pub struct CredentialReadError;

/// The host supplies private lookups; only declared names are ever requested.
/// None means absent. Err means present but unusable and forbids fallback.
pub trait CredentialInputs: Send + Sync {
    fn environment(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError>;
    fn host(&self, _name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        Ok(None)
    }
}
pub struct ProcessCredentials;
impl CredentialInputs for ProcessCredentials {
    fn environment(&self, name: &str) -> Result<Option<Vec<u8>>, CredentialReadError> {
        match std::env::var(name) {
            Ok(value) if value.len() <= MAX_CREDENTIAL_BYTES => Ok(Some(value.into_bytes())),
            Err(std::env::VarError::NotPresent) => Ok(None),
            _ => Err(CredentialReadError),
        }
    }
}

/// No Serialize/Hash implementation. Scope must match before a consumer can
/// access bytes. Private resource reuse compares values directly, never hashes.
pub struct ScopedCredential {
    consumer: CredentialConsumer,
    destination: String,
    value: String,
}
impl std::fmt::Debug for ScopedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScopedCredential([redacted])")
    }
}
impl ScopedCredential {
    pub fn expose_for(
        &self,
        consumer: CredentialConsumer,
        destination: &str,
    ) -> Result<&str, ConfigError> {
        if consumer != self.consumer || destination != self.destination {
            return Err(error("config_credential_scope", "/credentials"));
        }
        Ok(&self.value)
    }
    /// This result is host-private resource-cache state, not portable identity.
    pub fn same_private_value(&self, other: &Self) -> bool {
        self.consumer == other.consumer
            && self.destination == other.destination
            && self.value == other.value
    }
}

impl PreparedRun {
    pub fn credential(
        &self,
        consumer: CredentialConsumer,
        inputs: &dyn CredentialInputs,
    ) -> Result<Option<ScopedCredential>, ConfigError> {
        let options = self.deployment().options();
        let model = self.model_route().map_or_else(
            || self.deployment().selected_model(),
            |route| &self.deployment().options()["models"][route.entries()[0].name()],
        );
        let (reference, destination) = match consumer {
            CredentialConsumer::McpEnvironment
            | CredentialConsumer::McpHeaders
            | CredentialConsumer::A2aBearer => {
                return Err(error("config_credential_scope", "/credentials"));
            }
            CredentialConsumer::Vercel
            | CredentialConsumer::OpenRouter
            | CredentialConsumer::OpenResponses => {
                (&model["credential"], model["endpoint"].as_str().unwrap())
            }
            CredentialConsumer::OtelHeaders => (
                &options["otel"]["headers"],
                options["otel"]["endpoint"].as_str().unwrap(),
            ),
        };
        self.resolve_credential(consumer, reference, destination, inputs)
    }
    /// Resolve only the named host definition, scoped to its exact RPC endpoint.
    /// Public card retrieval must never use this credential.
    pub fn a2a_credential(
        &self,
        name: &str,
        inputs: &dyn CredentialInputs,
    ) -> Result<Option<ScopedCredential>, ConfigError> {
        let settings = self.deployment().a2a()?;
        let remote = settings
            .remotes
            .get(name)
            .ok_or_else(|| error("config_credential_scope", "/options/a2a/remotes"))?;
        match &remote.bearer {
            None => Ok(None),
            Some(bearer) => self.resolve_credential(
                CredentialConsumer::A2aBearer,
                &Value::String(bearer.credential.clone()),
                &remote.endpoint,
                inputs,
            ),
        }
    }
    /// Select only an entry of this prepared deployment's authorized route.
    pub fn route_credential(
        &self,
        index: usize,
        inputs: &dyn CredentialInputs,
    ) -> Result<ScopedCredential, ConfigError> {
        let entry = self
            .model_route()
            .and_then(|r| r.entries().get(index))
            .ok_or_else(|| error("config_credential_scope", "/model_route"))?;
        self.resolve_credential(
            entry.profile().provider.credential_consumer(),
            &Value::String(entry.credential().into()),
            &entry.profile().endpoint,
            inputs,
        )?
        .ok_or_else(|| error("config_credential_missing", "/model_route"))
    }
    pub(super) fn mcp_environment(
        &self,
        id: &str,
        server: &crate::mcp::Server,
        inputs: &dyn CredentialInputs,
    ) -> Result<BTreeMap<String, String>, ConfigError> {
        let crate::mcp::Server::Stdio { env, .. } = server else {
            return Err(error("config_credential_scope", "/credentials"));
        };
        let mut values = BTreeMap::new();
        let mut bytes = 0usize;
        for (binding, reference) in env {
            let destination = serde_json::json!([id, server, binding]).to_string();
            let credential = self
                .resolve_credential(
                    CredentialConsumer::McpEnvironment,
                    &Value::String(reference.clone()),
                    &destination,
                    inputs,
                )?
                .ok_or_else(|| error("config_credential_missing", "/credentials"))?;
            let value = credential.expose_for(CredentialConsumer::McpEnvironment, &destination)?;
            bytes = bytes
                .saturating_add(binding.len())
                .saturating_add(value.len());
            if bytes > 65536 {
                return Err(error("config_credential_invalid", "/credentials"));
            }
            values.insert(binding.clone(), value.to_owned());
        }
        Ok(values)
    }
    pub(super) fn mcp_headers(
        &self,
        id: &str,
        server: &crate::mcp::Server,
        endpoint: &str,
        inputs: &dyn CredentialInputs,
    ) -> Result<reqwest::header::HeaderMap, ConfigError> {
        let crate::mcp::Server::Http { headers, .. } = server else {
            return Err(error("config_credential_scope", "/credentials"));
        };
        let mut values = reqwest::header::HeaderMap::new();
        let mut bytes = 0usize;
        for (binding, reference) in headers {
            let destination = serde_json::json!([id, server, binding, endpoint]).to_string();
            let credential = self
                .resolve_credential(
                    CredentialConsumer::McpHeaders,
                    &Value::String(reference.clone()),
                    &destination,
                    inputs,
                )?
                .ok_or_else(|| error("config_credential_missing", "/credentials"))?;
            let value = credential.expose_for(CredentialConsumer::McpHeaders, &destination)?;
            bytes = bytes
                .saturating_add(binding.len())
                .saturating_add(value.len());
            if bytes > 65536 {
                return Err(error("config_credential_invalid", "/credentials"));
            }
            let name = reqwest::header::HeaderName::from_bytes(binding.as_bytes())
                .map_err(|_| error("config_credential_invalid", "/credentials"))?;
            let mut value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| error("config_credential_invalid", "/credentials"))?;
            value.set_sensitive(true);
            values.insert(name, value);
        }
        Ok(values)
    }
    fn resolve_credential(
        &self,
        consumer: CredentialConsumer,
        reference: &Value,
        destination: &str,
        inputs: &dyn CredentialInputs,
    ) -> Result<Option<ScopedCredential>, ConfigError> {
        let config = self.deployment().config();
        if reference["unset"] == true {
            return Ok(None);
        }
        let id = reference
            .as_str()
            .ok_or_else(|| error("config_credential_scope", "/credentials"))?;
        let diagnostic = || error("config_credential_invalid", &format!("/credentials/{id}"));
        let record = &config["credentials"][id];
        if record["consumer"] != consumer.name()
            || matches!(
                consumer,
                CredentialConsumer::Vercel | CredentialConsumer::OpenRouter
            ) && destination
                != match consumer {
                    CredentialConsumer::Vercel => crate::gateway::VERCEL_ENDPOINT,
                    CredentialConsumer::OpenRouter => crate::gateway::OPENROUTER_ENDPOINT,
                    CredentialConsumer::OtelHeaders
                    | CredentialConsumer::McpEnvironment
                    | CredentialConsumer::McpHeaders
                    | CredentialConsumer::A2aBearer => {
                        unreachable!()
                    }
                    CredentialConsumer::OpenResponses => unreachable!(),
                }
        {
            return Err(error("config_credential_scope", "/credentials"));
        }
        for source in record["sources"].as_array().unwrap() {
            let bytes = match source["kind"].as_str().unwrap() {
                "environment" => inputs
                    .environment(source["name"].as_str().unwrap())
                    .map_err(|_| diagnostic())?,
                "host" => inputs
                    .host(source["name"].as_str().unwrap())
                    .map_err(|_| diagnostic())?,
                "file" => {
                    let path = &source["path"];
                    let root = match path["base"].as_str().unwrap() {
                        "config" => self.config_root.as_path(),
                        "workspace" => self.spec().workspace.as_path(),
                        "binding" => self
                            .path_bindings
                            .get(path["name"].as_str().unwrap())
                            .ok_or_else(diagnostic)?
                            .as_path(),
                        _ => return Err(diagnostic()),
                    };
                    read_file(root, Path::new(path["path"].as_str().unwrap()))
                        .map_err(|_| diagnostic())?
                }
                _ => return Err(diagnostic()),
            };
            let Some(bytes) = bytes else {
                continue;
            };
            if bytes.len() > MAX_CREDENTIAL_BYTES {
                return Err(diagnostic());
            }
            let text = String::from_utf8(bytes).map_err(|_| diagnostic())?;
            let value = if source["kind"] == "file" && source["encoding"] == "dotenv" {
                dotenv(&text, source["key"].as_str().unwrap()).map_err(|_| diagnostic())?
            } else if source["kind"] == "file" {
                text.strip_suffix("\r\n")
                    .or_else(|| text.strip_suffix('\n'))
                    .unwrap_or(&text)
                    .to_owned()
            } else {
                text
            };
            if value.trim().is_empty()
                || value.len() > MAX_CREDENTIAL_BYTES
                || value.chars().any(char::is_control)
                || value.starts_with('\u{feff}')
            {
                return Err(diagnostic());
            }
            if matches!(
                consumer,
                CredentialConsumer::Vercel
                    | CredentialConsumer::OpenRouter
                    | CredentialConsumer::OpenResponses
            ) && (value.len() > 8192
                || value.chars().any(char::is_whitespace)
                || reqwest::header::HeaderValue::from_str(&value).is_err())
            {
                return Err(diagnostic());
            }
            return Ok(Some(ScopedCredential {
                consumer,
                destination: destination.into(),
                value,
            }));
        }
        Err(error(
            "config_credential_missing",
            &format!("/credentials/{id}"),
        ))
    }
}

#[cfg(unix)]
fn read_file(root: &Path, path: &Path) -> Result<Option<Vec<u8>>, CredentialReadError> {
    use rustix::fs::{Mode, OFlags, open, openat};
    use std::{fs::File, io::Read, os::unix::fs::MetadataExt};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let mut parent =
        open(root, flags | OFlags::DIRECTORY, Mode::empty()).map_err(|_| CredentialReadError)?;
    let parts: Vec<_> = path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err(CredentialReadError);
        };
        let final_part = index + 1 == parts.len();
        let file = match openat(
            &parent,
            *name,
            flags
                | if final_part {
                    OFlags::empty()
                } else {
                    OFlags::DIRECTORY
                },
            Mode::empty(),
        ) {
            Ok(file) => file,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(CredentialReadError),
        };
        if final_part {
            let file = File::from(file);
            let before = file.metadata().map_err(|_| CredentialReadError)?;
            if !before.is_file() || before.len() > MAX_CREDENTIAL_BYTES as u64 {
                return Err(CredentialReadError);
            }
            let mut bytes = Vec::new();
            (&file)
                .take(MAX_CREDENTIAL_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| CredentialReadError)?;
            let after = file.metadata().map_err(|_| CredentialReadError)?;
            let stamp = |m: &std::fs::Metadata| {
                (
                    m.len(),
                    m.mtime(),
                    m.mtime_nsec(),
                    m.ctime(),
                    m.ctime_nsec(),
                )
            };
            if bytes.len() > MAX_CREDENTIAL_BYTES
                || bytes.len() as u64 != after.len()
                || stamp(&before) != stamp(&after)
            {
                return Err(CredentialReadError);
            }
            return Ok(Some(bytes));
        }
        parent = file;
    }
    Err(CredentialReadError)
}
#[cfg(not(unix))]
fn read_file(_: &Path, _: &Path) -> Result<Option<Vec<u8>>, CredentialReadError> {
    Err(CredentialReadError)
}

// A deliberately non-evaluating, single-line dotenv grammar. Parse the whole
// bounded file so duplicate keys and malformed unrelated assignments reject.
fn dotenv(text: &str, wanted: &str) -> Result<String, CredentialReadError> {
    if text.starts_with('\u{feff}')
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(CredentialReadError);
    }
    let mut keys = HashSet::new();
    let mut selected = None;
    for line in text.lines() {
        if line.contains('\r') {
            return Err(CredentialReadError);
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let (key, value) = line.split_once('=').ok_or(CredentialReadError)?;
        let key = key.trim();
        if key.is_empty()
            || !key
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
            || !keys.insert(key)
        {
            return Err(CredentialReadError);
        }
        let value = value.trim_start();
        let value = if let Some(quote @ ('\'' | '"')) = value.chars().next() {
            let mut chars = value[1..].char_indices();
            let mut output = String::new();
            let mut closed = false;
            while let Some((index, c)) = chars.next() {
                if c == quote {
                    let tail = value[index + 2..].trim();
                    if !tail.is_empty() && !tail.starts_with('#') {
                        return Err(CredentialReadError);
                    }
                    closed = true;
                    break;
                }
                if c == '\\' && quote == '"' {
                    output.push(match chars.next().ok_or(CredentialReadError)?.1 {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        '$' => '$',
                        _ => return Err(CredentialReadError),
                    });
                } else {
                    output.push(c);
                }
            }
            if !closed {
                return Err(CredentialReadError);
            }
            output
        } else {
            let end = value
                .char_indices()
                .find(|(index, c)| {
                    *c == '#' && (*index == 0 || value[..*index].ends_with(char::is_whitespace))
                })
                .map(|(index, _)| index)
                .unwrap_or(value.len());
            value[..end].trim_end().to_owned()
        };
        if key == wanted {
            selected = Some(value);
        }
    }
    selected.ok_or(CredentialReadError)
}
