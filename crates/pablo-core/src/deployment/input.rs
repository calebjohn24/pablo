use super::{canonical::bytes_digest, *};
use serde_json::json;
use std::{
    collections::HashSet,
    path::{Component, Path},
    sync::OnceLock,
};

pub(crate) fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(DOCUMENT_SCHEMA).expect("checked-in deployment schema")
    })
}
pub(crate) fn defaults() -> Value {
    serde_json::from_str(DEFAULT_OPTIONS).expect("checked-in deployment defaults")
}
pub(crate) fn shape(document: &Value) -> Result<(), ConfigError> {
    if document.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err(error("config_schema_version", "/schema_version"));
    }
    unsupported(document, "")?;
    super::validate::strings(document, "")?;
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    validate(
        VALIDATOR.get_or_init(|| {
            crate::tool::compile_schema(schema()).expect("deployment schema compiles")
        }),
        document,
    )?;
    super::validate::declarations(document, "")
}
pub(crate) fn resolved_shape(config: &Value) -> Result<(), ConfigError> {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    let validator = VALIDATOR.get_or_init(|| {
        crate::tool::compile_schema(
            &json!({"$ref":"#/$defs/resolvedConfig", "$defs":schema()["$defs"]}),
        )
        .expect("resolved schema compiles")
    });
    validate(validator, config)
}
fn validate(validator: &jsonschema::Validator, value: &Value) -> Result<(), ConfigError> {
    if let Some(issue) = validator.iter_errors(value).next() {
        if exceeds_bound(&issue) {
            return Err(limit());
        }
        let mut path = issue.instance_path().to_string();
        let code =
            if let jsonschema::error::ValidationErrorKind::AdditionalProperties { unexpected } =
                issue.kind()
            {
                if let Some(name) = unexpected.first() {
                    path = pointer(&path, name);
                }
                "config_unknown_option"
            } else {
                "config_invalid_value"
            };
        return Err(error(code, &path));
    }
    Ok(())
}
fn exceeds_bound(issue: &jsonschema::ValidationError<'_>) -> bool {
    use jsonschema::error::ValidationErrorKind;
    match issue.kind() {
        ValidationErrorKind::MaxItems { .. } | ValidationErrorKind::MaxProperties { .. } => true,
        ValidationErrorKind::OneOfNotValid { context } => {
            context.iter().flatten().any(exceeds_bound)
        }
        _ => false,
    }
}
fn unsupported(value: &Value, path: &str) -> Result<(), ConfigError> {
    if let Some(options) = value.get("options").and_then(Value::as_object) {
        for (key, owner) in [
            ("children", "C3.21"),
            ("a2a", "C3.26"),
            ("diagnostics", "C3.31"),
            ("compatibility", "C3.33"),
        ] {
            if options.contains_key(key) {
                return Err(feature(&format!("{path}/options/{key}"), owner));
            }
        }
        for (section, key, owner) in [
            ("skills", "activate", "C3.20"),
            ("interfaces", "tui", "C3.29"),
            ("interfaces", "acp", "C3.33"),
        ] {
            if options.get(section).and_then(|v| v.get(key)).is_some() {
                return Err(feature(&format!("{path}/options/{section}/{key}"), owner));
            }
        }
    }
    if value.get("requires").is_some() {
        return Err(feature(&format!("{path}/requires"), "C3.33"));
    }
    if let Some(profiles) = value.get("profiles").and_then(Value::as_object) {
        for (name, profile) in profiles {
            unsupported(profile, &pointer(&format!("{path}/profiles"), name))?;
        }
    }
    Ok(())
}
fn feature(path: &str, owner: &'static str) -> ConfigError {
    let mut result = error("config_unsupported_feature", path);
    result.owner = Some(owner);
    result
}

pub(crate) fn check_value(
    value: &Value,
    nodes: &mut usize,
    depth: usize,
) -> Result<(), ConfigError> {
    *nodes = nodes.checked_add(1).ok_or_else(limit)?;
    if *nodes > MAX_NODES || depth > MAX_DEPTH {
        return Err(limit());
    }
    match value {
        Value::Object(map) => {
            if map.len() > 1024 {
                return Err(limit());
            }
            for (key, value) in map {
                if key.len() > 4096 {
                    return Err(limit());
                }
                check_value(value, nodes, depth + 1)?;
            }
        }
        Value::Array(values) => {
            if values.len() > 1024 {
                return Err(limit());
            }
            for value in values {
                check_value(value, nodes, depth + 1)?;
            }
        }
        Value::String(s) if s.len() > MAX_FILE_BYTES => return Err(limit()),
        Value::Number(n) if !n.is_i64() && !n.is_u64() => {
            return Err(error("config_invalid_value", "/"));
        }
        _ => {}
    }
    Ok(())
}
fn convert(value: toml::Value, nodes: &mut usize, depth: usize) -> Result<Value, ConfigError> {
    *nodes = nodes.checked_add(1).ok_or_else(limit)?;
    if *nodes > MAX_NODES || depth > MAX_DEPTH {
        return Err(limit());
    }
    Ok(match value {
        toml::Value::String(s) => {
            if s.contains('\0') {
                return Err(error("config_invalid_value", "/"));
            }
            Value::String(s)
        }
        toml::Value::Integer(n) => n.into(),
        toml::Value::Boolean(b) => b.into(),
        toml::Value::Array(values) => {
            if values.len() > 1024 {
                return Err(limit());
            }
            Value::Array(
                values
                    .into_iter()
                    .map(|v| convert(v, nodes, depth + 1))
                    .collect::<Result<_, _>>()?,
            )
        }
        toml::Value::Table(table) => {
            if table.len() > 1024 {
                return Err(limit());
            }
            Value::Object(
                table
                    .into_iter()
                    .map(|(k, v)| Ok((k, convert(v, nodes, depth + 1)?)))
                    .collect::<Result<_, ConfigError>>()?,
            )
        }
        toml::Value::Float(_) | toml::Value::Datetime(_) => {
            return Err(error("config_invalid_value", "/"));
        }
    })
}

pub(crate) struct Document {
    pub value: Value,
    pub locator: String,
    pub digest: String,
    pub file: bool,
}
pub(crate) struct Reader {
    root: PathBuf,
    #[cfg(unix)]
    directory: rustix::fd::OwnedFd,
    seen: HashSet<PathBuf>,
    identities: HashSet<(u64, u64)>,
    pub bytes: usize,
    pub nodes: usize,
}
impl Reader {
    pub fn new(root: &Path) -> Result<Self, ConfigError> {
        if !absolute_root(root) {
            return Err(error("config_import_path", "/config_root"));
        }
        #[cfg(unix)]
        let directory = {
            use rustix::fs::{Mode, OFlags, open, openat};
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let mut directory = open("/", flags, Mode::empty())
                .map_err(|_| error("config_import_path", "/config_root"))?;
            for component in root.components() {
                if let Component::Normal(name) = component {
                    directory = openat(&directory, name, flags, Mode::empty())
                        .map_err(|_| error("config_import_path", "/config_root"))?;
                }
            }
            directory
        };
        Ok(Self {
            root: root.into(),
            #[cfg(unix)]
            directory,
            seen: HashSet::new(),
            identities: HashSet::new(),
            bytes: 0,
            nodes: 0,
        })
    }
    pub fn entry_path(&self, path: &Path) -> Result<String, ConfigError> {
        let path = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .map_err(|_| error("config_import_path", "/entry"))?
        } else {
            path
        };
        relative(
            path.to_str()
                .ok_or_else(|| error("config_import_path", "/entry"))?,
        )
    }
    pub fn document(&mut self, value: &Value, locator: &str) -> Result<Document, ConfigError> {
        check_value(value, &mut self.nodes, 1)?;
        let bytes = canonical::encode(value)?;
        self.charge(bytes.len())?;
        shape(value).map_err(|e| e.at(locator))?;
        Ok(Document {
            value: value.clone(),
            locator: locator.into(),
            digest: bytes_digest(&bytes),
            file: false,
        })
    }
    fn charge(&mut self, count: usize) -> Result<(), ConfigError> {
        if count > MAX_FILE_BYTES || count > MAX_INPUT_BYTES.saturating_sub(self.bytes) {
            return Err(limit());
        }
        self.bytes += count;
        Ok(())
    }
    pub fn read(&mut self, locator: &str) -> Result<Document, ConfigError> {
        if self.seen.len() == MAX_FILES {
            return Err(limit().at(locator));
        }
        if !self.seen.insert(locator.into()) {
            return Err(error("config_duplicate_import", "/imports").at(locator));
        }
        let available = MAX_FILE_BYTES.min(MAX_INPUT_BYTES.saturating_sub(self.bytes));
        if available == 0 {
            return Err(limit().at(locator));
        }
        let (bytes, identity) = self
            .read_file(locator, available)
            .map_err(|e| e.at(locator))?;
        if !self.identities.insert(identity) {
            return Err(error("config_duplicate_import", "/imports").at(locator));
        }
        self.charge(bytes.len())?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| error("config_parse", "/").at(locator))?;
        if text.starts_with('\u{feff}') || text.contains('\0') {
            return Err(error("config_parse", "/").at(locator));
        }
        let text = text.replace("\r\n", "\n");
        // The pinned parser retains its recursion limit (unbounded is disabled).
        // Byte admission precedes parsing; semantic nodes are charged before
        // growing the canonical value tree, including all inactive profiles.
        let parsed: toml::Value = toml::from_str(&text).map_err(|issue: toml::de::Error| {
            let mut e = error("config_parse", "/").at(locator);
            e.line = issue.span().map(|span| {
                text.as_bytes()[..span.start.min(text.len())]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count()
                    + 1
            });
            e
        })?;
        let value = convert(parsed, &mut self.nodes, 1).map_err(|e| e.at(locator))?;
        shape(&value).map_err(|e| e.at(locator))?;
        Ok(Document {
            value,
            locator: locator.into(),
            digest: bytes_digest(&bytes),
            file: true,
        })
    }
    #[cfg(unix)]
    fn read_file(
        &self,
        locator: &str,
        available: usize,
    ) -> Result<(Vec<u8>, (u64, u64)), ConfigError> {
        use rustix::fs::{Mode, OFlags, openat};
        let path = Path::new(locator);
        let mut parent =
            rustix::io::dup(&self.directory).map_err(|_| error("config_io", "/imports"))?;
        let names: Vec<_> = path.components().collect();
        for (index, component) in names.iter().enumerate() {
            let Component::Normal(name) = component else {
                return Err(error("config_import_path", "/imports"));
            };
            let final_part = index + 1 == names.len();
            let flags = OFlags::RDONLY
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC
                | OFlags::NONBLOCK
                | if final_part {
                    OFlags::empty()
                } else {
                    OFlags::DIRECTORY
                };
            let next = openat(&parent, *name, flags, Mode::empty())
                .map_err(|_| error("config_import_path", "/imports"))?;
            if final_part {
                return snapshot(std::fs::File::from(next), available, || {});
            }
            parent = next;
        }
        Err(error("config_import_path", "/imports"))
    }
    #[cfg(not(unix))]
    fn read_file(&self, _: &str, _: usize) -> Result<(Vec<u8>, (u64, u64)), ConfigError> {
        Err(error("config_unsupported_platform", "/imports"))
    }
}

#[cfg(unix)]
fn snapshot(
    file: std::fs::File,
    available: usize,
    after_metadata: impl FnOnce(),
) -> Result<(Vec<u8>, (u64, u64)), ConfigError> {
    use std::{io::Read, os::unix::fs::MetadataExt};
    let before = file
        .metadata()
        .map_err(|_| error("config_io", "/imports"))?;
    if !before.is_file() {
        return Err(error("config_import_path", "/imports"));
    }
    if before.len() > available as u64 {
        return Err(limit());
    }
    after_metadata();
    let mut bytes = Vec::new();
    (&file)
        .take(available as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("config_io", "/imports"))?;
    let after = file
        .metadata()
        .map_err(|_| error("config_io", "/imports"))?;
    let stamp = |m: &std::fs::Metadata| {
        (
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if stamp(&before) != stamp(&after) || after.len() != bytes.len() as u64 {
        return Err(error("config_input_changed", "/imports"));
    }
    if bytes.len() > available {
        return Err(limit());
    }
    Ok((bytes, (before.dev(), before.ino())))
}

pub(crate) fn absolute_root(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
pub(crate) fn relative(path: &str) -> Result<String, ConfigError> {
    if path.is_empty() || path.len() > 4096 || path.contains('\0') || Path::new(path).is_absolute()
    {
        return Err(error("config_import_path", "/path"));
    }
    let mut names = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                names
                    .pop()
                    .ok_or_else(|| error("config_import_path", "/path"))?;
            }
            name => names.push(name),
        }
    }
    Ok(if names.is_empty() {
        ".".into()
    } else {
        names.join("/")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_data_budgets_accept_bound_and_reject_next_node() {
        let mut nested = Value::Null;
        for _ in 1..MAX_DEPTH {
            nested = json!([nested]);
        }
        assert!(check_value(&nested, &mut 0, 1).is_ok());
        assert_eq!(
            check_value(&json!([nested]), &mut 0, 1).unwrap_err().code,
            "config_limit"
        );
        let mut groups = vec![json!(vec![Value::Null; 1023]); 64];
        groups[63].as_array_mut().unwrap().pop();
        let mut nodes = 0;
        check_value(&json!(groups), &mut nodes, 1).unwrap();
        assert_eq!(nodes, MAX_NODES);
        assert_eq!(
            check_value(&Value::Null, &mut nodes, 1).unwrap_err().code,
            "config_limit"
        );
        groups[63].as_array_mut().unwrap().push(Value::Null);
        assert_eq!(
            check_value(&json!(groups), &mut 0, 1).unwrap_err().code,
            "config_limit"
        );
        for size in [1024, 1025] {
            let array = json!(vec![Value::Null; size]);
            let map: Map<_, _> = (0..size).map(|i| (format!("k{i}"), Value::Null)).collect();
            for value in [array, json!(map)] {
                if size == 1024 {
                    assert!(check_value(&value, &mut 0, 1).is_ok());
                } else {
                    assert_eq!(
                        check_value(&value, &mut 0, 1).unwrap_err().code,
                        "config_limit"
                    );
                }
            }
        }
    }
    #[test]
    fn environment_declaration_bound_is_independent_of_target_conflicts() {
        // Only 35 distinct scalar targets exist today. Exercise the 64-name
        // structural cap here; resolution separately rejects repeated targets.
        for size in [64, 65] {
            let bindings: Map<_, _> = (0..size)
                .map(|i| {
                    (
                        format!("ENV_{i}"),
                        json!({"option":"limits.max_tool_calls"}),
                    )
                })
                .collect();
            let document = json!({"schema_version":1,"environment":bindings});
            if size == 64 {
                assert!(shape(&document).is_ok());
            } else {
                assert_eq!(shape(&document).unwrap_err().code, "config_limit");
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn changed_open_file_is_rejected_without_diagnostic_content() {
        let path = std::env::temp_dir().join(format!("pablo-config-read-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "schema_version=1\n").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let result = snapshot(file, MAX_FILE_BYTES, || {
            std::fs::write(&path, "PRIVATE-SENTINEL").unwrap()
        });
        std::fs::remove_file(path).unwrap();
        let e = result.unwrap_err();
        assert_eq!(e.code, "config_input_changed");
        assert!(!e.to_string().contains("PRIVATE-SENTINEL"));
    }
}
