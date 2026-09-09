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

pub struct Bootstrap {
    entry: PathBuf,
    root: PathBuf,
    profile: Option<String>,
    bindings: BTreeMap<String, PathBuf>,
    user: Option<PathBuf>,
    workspace: Option<PathBuf>,
    locked: bool,
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
    /// Remove host bootstrap flags, leaving ordinary task flags untouched.
    pub fn extract(arguments: &mut Vec<OsString>) -> Result<Option<Self>, String> {
        let cwd = std::env::current_dir().map_err(|_| invalid())?;
        let mut entry = None;
        let mut root = None;
        let mut profile = None;
        let mut bindings = BTreeMap::new();
        let mut user = None;
        let mut workspace = None;
        let mut locked = false;
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
            ) {
                rest.push(argument);
                continue;
            }
            if name != "--bind" && !seen.insert(name.to_owned()) {
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
            return if seen.is_empty() && bindings.is_empty() {
                Ok(None)
            } else {
                Err(invalid())
            };
        };
        let root = root.unwrap_or_else(|| entry.parent().unwrap().to_path_buf());
        Ok(Some(Self {
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
        loaded
            .with_environment(environment)
            .and_then(|loaded| loaded.resolve())
            .map_err(|e| e.to_string())
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
