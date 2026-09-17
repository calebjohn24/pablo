//! Explicit deployment bootstrap and local inspection. No ambient compatibility
//! options or credentials enter the configured path.
use pablo_core::deployment::{self, ConfigInput, ResolveRequest, ResolvedDeployment};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

pub const HELP: &str = "\nOffline deployment inspection:\n  pablo config validate|explain|render --config PATH [--profile NAME]\n    [--config-root PATH] [--bind NAME=PATH ...] [--locked]\n    [--user-config PATH] [--workspace-config PATH]\nPaths are anchored at invocation; config-root defaults to the entry directory.\nBind workspace explicitly, for example --bind workspace=./project.\nInspection reads only config files and declared non-secret environment values.\n";

#[derive(Clone)]
pub struct Bootstrap {
    invocation: PathBuf,
    pub fixture_endpoint: Option<String>,
    fixture_mcp_endpoints: BTreeMap<String, String>,
    pub fixture_a2a_endpoints: BTreeMap<String, String>,
    entry: PathBuf,
    root: PathBuf,
    profile: Option<String>,
    bindings: BTreeMap<String, PathBuf>,
    user: Option<PathBuf>,
    workspace: Option<PathBuf>,
    locked: bool,
}

struct ProviderSecret {
    credential: Option<deployment::ScopedCredential>,
    profile: pablo_core::gateway::ModelProfile,
}
pub struct Secrets {
    router: Option<deployment::ScopedCredential>,
    providers: Vec<ProviderSecret>,
    route: Option<deployment::ResolvedRoute>,
    pub headers: Option<deployment::ScopedCredential>,
}
impl Secrets {
    pub fn read(prepared: &deployment::PreparedRun, bootstrap: &Bootstrap) -> Result<Self, String> {
        let route = prepared.model_route().cloned();
        let profiles = if let Some(route) = &route {
            route
                .entries()
                .iter()
                .map(|entry| entry.profile().clone())
                .collect()
        } else {
            vec![prepared.model_profile().map_err(|e| e.to_string())?]
        };
        let mut providers = Vec::new();
        for (index, profile) in profiles.into_iter().enumerate() {
            profile.provider.ensure_available()?;
            let credential = if bootstrap.fixture_endpoint.is_some() {
                None
            } else if route.is_some() {
                Some(
                    prepared
                        .route_credential(index, &deployment::ProcessCredentials)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                prepared
                    .credential(
                        profile.provider.credential_consumer(),
                        &deployment::ProcessCredentials,
                    )
                    .map_err(|e| e.to_string())?
            };
            providers.push(ProviderSecret {
                credential,
                profile,
            });
        }
        let otel = &prepared.deployment().options()["otel"];
        let headers = if otel["sdk_disabled"] != true && otel["exporter"] == "otlp" {
            prepared
                .credential(
                    deployment::CredentialConsumer::OtelHeaders,
                    &deployment::ProcessCredentials,
                )
                .map_err(|e| e.to_string())?
        } else {
            None
        };
        Ok(Self {
            router: if bootstrap.fixture_endpoint.is_some() {
                None
            } else {
                prepared
                    .router_credential(&deployment::ProcessCredentials)
                    .map_err(|e| e.to_string())?
            },
            providers,
            route,
            headers,
        })
    }
    pub fn provider(&self, bootstrap: &Bootstrap) -> Result<Box<dyn pablo_core::Provider>, String> {
        let mut providers: Vec<Box<dyn pablo_core::Provider>> = Vec::new();
        for entry in &self.providers {
            let adapter = if let Some(endpoint) = &bootstrap.fixture_endpoint {
                pablo_core::gateway::GatewayProvider::configured_fixture(&entry.profile, endpoint)
                    .map_err(|_| "config_invalid_value at /fixture_endpoint")?
            } else {
                let secret = entry
                    .credential
                    .as_ref()
                    .ok_or("config_credential_missing at /options/model/credential")?;
                let value = secret
                    .expose_for(
                        entry.profile.provider.credential_consumer(),
                        &entry.profile.endpoint,
                    )
                    .map_err(|e| e.to_string())?;
                pablo_core::gateway::GatewayProvider::configured(&entry.profile, value)
                    .map_err(|_| "config_credential_invalid at /options/model/credential")?
            };
            providers.push(Box::new(adapter));
        }
        if let Some(route) = &self.route {
            if let Some(config) = route.router() {
                let router = if let Some(endpoint) = &bootstrap.fixture_endpoint {
                    pablo_core::gateway::JevProvider::local_fixture(config.clone(), endpoint)
                } else {
                    let key = self
                        .router
                        .as_ref()
                        .ok_or("config_credential_missing at /model_route/router")?
                        .expose_for(
                            deployment::CredentialConsumer::Vercel,
                            pablo_core::gateway::JEV_ENDPOINT,
                        )
                        .map_err(|e| e.to_string())?;
                    pablo_core::gateway::JevProvider::new(config.clone(), key)
                }
                .map_err(|_| "config_invalid_value at /model_route/router")?;
                return Ok(Box::new(
                    pablo_core::provider::ProviderRoute::with_router(
                        route.clone(),
                        providers,
                        router,
                    )
                    .map_err(|_| "config_invalid_value at /model_route/router")?,
                ));
            }
            Ok(Box::new(
                pablo_core::provider::ProviderRoute::new(route.clone(), providers)
                    .map_err(|_| "config_invalid_value at /model_route")?,
            ))
        } else {
            Ok(providers.remove(0))
        }
    }
    pub fn same_private_values(&self, other: &Self) -> bool {
        let same = |a: &Option<deployment::ScopedCredential>,
                    b: &Option<deployment::ScopedCredential>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_private_value(b),
            _ => false,
        };
        self.route == other.route
            && same(&self.router, &other.router)
            && self.providers.len() == other.providers.len()
            && self
                .providers
                .iter()
                .zip(&other.providers)
                .all(|(a, b)| a.profile == b.profile && same(&a.credential, &b.credential))
            && same(&self.headers, &other.headers)
    }
}

fn invalid() -> String {
    "config_invalid_value at /bootstrap".into()
}

// Anchor paths once without probing workspace/secret roots during inspection.
fn absolute(cwd: &Path, path: &Path) -> Result<PathBuf, String> {
    let joined = cwd.join(path);
    let mut result = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                if !result.pop() {
                    return Err(invalid());
                }
            }
            Component::CurDir => {}
            component => result.push(component),
        }
    }
    if result
        .to_str()
        .is_none_or(|p| p.contains('\0') || p.len() > 4096)
    {
        return Err(invalid());
    }
    Ok(result)
}

impl Bootstrap {
    pub async fn tools(
        &self,
        prepared: &deployment::PreparedRun,
        deadline: tokio::time::Instant,
        cancellation: &pablo_core::CancellationToken,
    ) -> Result<pablo_core::ToolRegistry, String> {
        prepared
            .tools_with_mcp_fixture(
                &deployment::ProcessCredentials,
                deadline,
                cancellation,
                &self.fixture_mcp_endpoints,
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// Remove host bootstrap flags, leaving ordinary task flags untouched.
    pub fn extract(arguments: &mut Vec<OsString>) -> Result<Option<Self>, String> {
        if !arguments.iter().take_while(|arg| *arg != "--").any(|arg| {
            matches!(
                arg.to_str(),
                Some(
                    "--config"
                        | "--config-root"
                        | "--profile"
                        | "--bind"
                        | "--locked"
                        | "--user-config"
                        | "--workspace-config"
                        | "--fixture-endpoint"
                        | "--fixture-mcp-endpoint"
                        | "--fixture-a2a-endpoint"
                )
            )
        }) {
            return Ok(None);
        }
        let cwd = std::env::current_dir().map_err(|_| invalid())?;
        let mut entry = None;
        let mut root = None;
        let mut profile = None;
        let mut bindings = BTreeMap::new();
        let mut user = None;
        let mut workspace = None;
        let mut locked = false;
        let mut fixture_endpoint = None;
        let mut fixture_mcp_endpoints = BTreeMap::new();
        let mut fixture_a2a_endpoints = BTreeMap::new();
        let mut seen = HashSet::new();
        let mut rest = Vec::new();
        let mut args = std::mem::take(arguments).into_iter();
        while let Some(argument) = args.next() {
            let name = argument.to_str().ok_or_else(invalid)?;
            if name == "--" {
                rest.push(argument);
                rest.extend(args);
                break;
            }
            if !matches!(
                name,
                "--config"
                    | "--config-root"
                    | "--profile"
                    | "--bind"
                    | "--user-config"
                    | "--workspace-config"
                    | "--locked"
                    | "--fixture-endpoint"
                    | "--fixture-mcp-endpoint"
                    | "--fixture-a2a-endpoint"
            ) {
                rest.push(argument);
                continue;
            }
            if !matches!(
                name,
                "--bind" | "--fixture-mcp-endpoint" | "--fixture-a2a-endpoint"
            ) && !seen.insert(name.to_owned())
            {
                return Err(invalid());
            }
            if name == "--locked" {
                locked = true;
                continue;
            }
            let value = args.next().ok_or_else(invalid)?;
            let value = value.to_str().ok_or_else(invalid)?;
            if value.is_empty() || value.len() > 4096 {
                return Err(invalid());
            }
            match name {
                "--config" => entry = Some(absolute(&cwd, Path::new(value))?),
                "--config-root" => root = Some(absolute(&cwd, Path::new(value))?),
                "--profile" => profile = Some(value.to_owned()),
                "--fixture-endpoint" => fixture_endpoint = Some(value.to_owned()),
                "--fixture-a2a-endpoint" => {
                    let (id, endpoint) = value.split_once('=').ok_or_else(invalid)?;
                    let target = reqwest::Url::parse(endpoint).map_err(|_| invalid())?;
                    if id.is_empty()
                        || id.len() > 128
                        || !id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                        || target.scheme() != "http"
                        || !matches!(target.host_str(), Some("127.0.0.1" | "[::1]"))
                        || !target.username().is_empty()
                        || target.password().is_some()
                        || target.query().is_some()
                        || target.fragment().is_some()
                        || fixture_a2a_endpoints.len() >= 16
                        || fixture_a2a_endpoints
                            .insert(id.into(), endpoint.into())
                            .is_some()
                    {
                        return Err(invalid());
                    }
                }
                "--fixture-mcp-endpoint" => {
                    let (id, endpoint) = value.split_once('=').ok_or_else(invalid)?;
                    if id.is_empty()
                        || endpoint.is_empty()
                        || fixture_mcp_endpoints.len() >= 16
                        || fixture_mcp_endpoints
                            .insert(id.to_owned(), endpoint.to_owned())
                            .is_some()
                    {
                        return Err(invalid());
                    }
                }
                "--user-config" => user = Some(absolute(&cwd, Path::new(value))?),
                "--workspace-config" => workspace = Some(absolute(&cwd, Path::new(value))?),
                "--bind" => {
                    let (name, path) = value.split_once('=').ok_or_else(invalid)?;
                    if path.is_empty()
                        || bindings.len() >= 64
                        || bindings
                            .insert(name.to_owned(), absolute(&cwd, Path::new(path))?)
                            .is_some()
                    {
                        return Err(invalid());
                    }
                }
                _ => unreachable!(),
            }
        }
        *arguments = rest;
        let Some(entry) = entry else {
            return if seen.is_empty()
                && bindings.is_empty()
                && fixture_mcp_endpoints.is_empty()
                && fixture_a2a_endpoints.is_empty()
            {
                Ok(None)
            } else {
                Err(invalid())
            };
        };
        if (!fixture_mcp_endpoints.is_empty() || !fixture_a2a_endpoints.is_empty())
            && fixture_endpoint.is_none()
        {
            return Err(invalid());
        }
        let root = root.unwrap_or_else(|| entry.parent().unwrap().to_path_buf());
        Ok(Some(Self {
            invocation: cwd,
            fixture_endpoint,
            fixture_mcp_endpoints,
            fixture_a2a_endpoints,
            entry,
            root,
            profile,
            bindings,
            user,
            workspace,
            locked,
        }))
    }

    pub fn resolve(&self) -> Result<ResolvedDeployment, String> {
        let mut request = ResolveRequest::new(self.root.clone(), &self.entry);
        request.profile = self.profile.clone();
        request.path_bindings = self.bindings.clone();
        request.user_config = self.user.clone().map(ConfigInput::File);
        request.workspace_config = self.workspace.clone().map(ConfigInput::File);
        request.locked = self.locked;
        let loaded = deployment::load(request).map_err(|e| e.to_string())?;
        let mut environment = BTreeMap::new();
        for name in loaded.environment_names() {
            if let Some(value) = std::env::var_os(name) {
                let value = value
                    .into_string()
                    .map_err(|_| "config_environment_value at /environment")?;
                if value.len() > 65_536 {
                    return Err("config_environment_value at /environment".into());
                }
                environment.insert(name.to_owned(), value);
            }
        }
        let resolved = loaded
            .with_environment(environment)
            .and_then(|loaded| loaded.resolve())
            .map_err(|e| e.to_string())?;
        if !self.fixture_mcp_endpoints.is_empty() {
            resolved
                .validate_mcp_fixture_endpoints(&self.fixture_mcp_endpoints)
                .map_err(|e| e.to_string())?;
        }
        Ok(resolved)
    }

    pub fn path_reference(&self, path: &Path) -> Result<serde_json::Value, String> {
        let path = absolute(&self.invocation, path)?;
        let mut choices: Vec<_>=self.bindings.iter().filter_map(|(name,root)| path.strip_prefix(root).ok().map(|relative|(root.components().count(),serde_json::json!({"base":"binding","name":name,"path":if relative.as_os_str().is_empty(){"."}else{relative.to_str().unwrap()}})))).collect();
        if let Ok(relative) = path.strip_prefix(&self.root) {
            choices.push((self.root.components().count(),serde_json::json!({"base":"config","path":if relative.as_os_str().is_empty(){"."}else{relative.to_str().unwrap()}})));
        }
        choices.sort_by_key(|(depth, _)| *depth);
        choices
            .pop()
            .map(|(_, value)| value)
            .ok_or_else(|| "config_authority_violation at /path".into())
    }
}

pub fn inspect(mut arguments: Vec<OsString>) -> Result<(), String> {
    if arguments.is_empty() {
        return Err(invalid());
    }
    let command = arguments.remove(0);
    if !matches!(command.to_str(), Some("validate" | "explain" | "render")) {
        return Err(invalid());
    }
    let bootstrap = Bootstrap::extract(&mut arguments)?.ok_or_else(invalid)?;
    if !arguments.is_empty() {
        return Err(invalid());
    }
    let resolved = bootstrap.resolve()?;
    // Complete encoding before writing anything: failures leave stdout empty.
    let output = match command.to_str().unwrap() {
        "validate" => format!("valid {}\n", resolved.fingerprint()),
        "explain" => serde_json::to_string(&resolved).map_err(|_| "config_limit at /")? + "\n",
        "render" => resolved.render().map_err(|e| e.to_string())?,
        _ => unreachable!(),
    };
    io::stdout()
        .lock()
        .write_all(output.as_bytes())
        .map_err(|_| "cannot write configuration inspection".into())
}
