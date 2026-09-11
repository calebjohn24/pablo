//! Explicit host-root Agent Skills discovery. No instruction activation or execution.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

pub const FORMAT_REVISION: &str = "69ef37e9424c0a7ea9dd2293b559e43ec8176379";
pub const MAX_METADATA_BYTES: usize = 16 * 1024;
pub const MAX_ROOTS: usize = 16;
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_SKILLS: usize = 256;
pub const MAX_SCAN_BYTES: usize = 1024 * 1024;
pub const MAX_DIAGNOSTICS: usize = 64;

/// Host-approved, explicitly selected root; no ambient root discovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub id: String,
    pub path: std::path::PathBuf,
}
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub qualified_name: String,
    pub root: String,
    pub relative_path: String,
    /// Frontmatter identity only. Full instructions are hashed on activation.
    pub metadata_sha256: String,
    pub metadata: Metadata,
}
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub root: String,
    pub relative_path: Option<String>,
    pub code: Error,
}
#[derive(Clone, Debug, Serialize)]
pub struct Catalog {
    pub format_revision: &'static str,
    pub roots: Vec<Root>,
    pub entries: Vec<Entry>,
    pub diagnostics: Vec<Diagnostic>,
    pub scanned_entries: usize,
    pub metadata_bytes: usize,
}
impl Catalog {
    pub fn resolve(&self, name: &str) -> Result<&Entry, Error> {
        let mut found = self
            .entries
            .iter()
            .filter(|e| e.qualified_name == name || e.metadata.name == name);
        let entry = found.next().ok_or(Error::NotFound)?;
        if found.next().is_some() {
            return Err(Error::Ambiguous);
        }
        Ok(entry)
    }
}

#[cfg(unix)]
mod discovery;
#[cfg(unix)]
pub use discovery::discover;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    InvalidMetadata,
    MetadataBound,
    InvalidName,
    DirectoryMismatch,
    Duplicate,
    Ambiguous,
    NotFound,
    InvalidRoot,
    Symlink,
    NotRegular,
    Io,
    ScanBound,
    Cancelled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "skill discovery: {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Descriptive portable metadata; never grants execution authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

pub(crate) fn normalized_name(value: &str) -> Result<String, Error> {
    let name: String = value.trim().nfkc().collect();
    if name.is_empty()
        || name.chars().count() > 64
        || name != name.to_lowercase()
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name.chars().all(|c| c.is_alphanumeric() || c == '-')
    {
        return Err(Error::InvalidName);
    }
    Ok(name)
}

/// Parse only the YAML frontmatter, without delimiters or Markdown body.
/// Scalars remain strings, matching the pinned reference's StrictYAML behavior.
pub fn parse_metadata(yaml: &str, directory: &str) -> Result<Metadata, Error> {
    use yaml_rust2::parser::{Event, Parser};
    if yaml.len() > MAX_METADATA_BYTES {
        return Err(Error::MetadataBound);
    }
    let mut parser = Parser::new_from_str(yaml);
    let mut events = Vec::new();
    loop {
        let (event, _) = parser.next_token().map_err(|_| Error::InvalidMetadata)?;
        if events.len() == 256 {
            return Err(Error::MetadataBound);
        }
        let end = event == Event::StreamEnd;
        events.push(event);
        if end {
            break;
        }
    }
    let mut events = events.into_iter();
    if events.next() != Some(Event::StreamStart)
        || events.next() != Some(Event::DocumentStart)
        || events.next() != Some(Event::MappingStart(0, None))
    {
        return Err(Error::InvalidMetadata);
    }
    let scalar = |event: Option<Event>| match event {
        Some(Event::Scalar(value, _, 0, None)) if !value.contains('\0') => Ok(value),
        _ => Err(Error::InvalidMetadata),
    };
    let mut fields = BTreeMap::new();
    let mut extra = BTreeMap::new();
    let mut seen_metadata = false;
    loop {
        let event = events.next();
        if event == Some(Event::MappingEnd) {
            break;
        }
        let key = scalar(event)?;
        if key == "metadata" {
            if seen_metadata {
                return Err(Error::Duplicate);
            }
            seen_metadata = true;
            if events.next() != Some(Event::MappingStart(0, None)) {
                return Err(Error::InvalidMetadata);
            }
            loop {
                let event = events.next();
                if event == Some(Event::MappingEnd) {
                    break;
                }
                let key = scalar(event)?;
                let value = scalar(events.next())?;
                if key.is_empty() || key.len() > 256 || value.len() > 4096 || extra.len() == 64 {
                    return Err(Error::MetadataBound);
                }
                if extra.insert(key, value).is_some() {
                    return Err(Error::Duplicate);
                }
            }
        } else {
            if !matches!(
                key.as_str(),
                "name" | "description" | "license" | "compatibility" | "allowed-tools"
            ) {
                return Err(Error::InvalidMetadata);
            }
            if fields.insert(key, scalar(events.next())?).is_some() {
                return Err(Error::Duplicate);
            }
        }
    }
    if events.next() != Some(Event::DocumentEnd)
        || events.next() != Some(Event::StreamEnd)
        || events.next().is_some()
    {
        return Err(Error::InvalidMetadata);
    }
    let name = normalized_name(&fields.remove("name").ok_or(Error::InvalidMetadata)?)?;
    if directory.nfkc().collect::<String>() != name {
        return Err(Error::DirectoryMismatch);
    }
    let description = fields.remove("description").ok_or(Error::InvalidMetadata)?;
    if description.trim().is_empty() || description.chars().count() > 1024 {
        return Err(Error::InvalidMetadata);
    }
    let compatibility = fields.remove("compatibility");
    if compatibility
        .as_ref()
        .is_some_and(|s| s.trim().is_empty() || s.chars().count() > 500)
    {
        return Err(Error::InvalidMetadata);
    }
    Ok(Metadata {
        name,
        description: description.trim().into(),
        license: fields.remove("license"),
        compatibility,
        allowed_tools: fields.remove("allowed-tools"),
        metadata: extra,
    })
}

#[cfg(test)]
mod tests;
