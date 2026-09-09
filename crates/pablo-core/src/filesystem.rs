//! Bounded explicit filesystem operations anchored to an admitted workspace.
use crate::{
    PolicyRule, RunLimits,
    policy::PolicySet,
    tool::{
        Tool, ToolContext, ToolDescriptor, ToolResult, ToolSetupError, ToolStatus, compile_schema,
    },
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilesystemLimits {
    pub max_file_bytes: usize,
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_scan_bytes: usize,
}
impl Default for FilesystemLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 8 * 1024 * 1024,
            max_entries: 10_000,
            max_depth: 32,
            max_scan_bytes: 64 * 1024 * 1024,
        }
    }
}
impl FilesystemLimits {
    pub(crate) fn valid(&self) -> bool {
        self.max_file_bytes > 0
            && self.max_entries > 0
            && self.max_depth > 0
            && self.max_scan_bytes > 0
            && self.max_file_bytes < usize::MAX
            && self.max_scan_bytes < usize::MAX
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsError {
    NotFound,
    NotFile,
    NotDirectory,
    UnsupportedEncoding,
    InvalidRange,
    Conflict,
    MatchNotUnique,
    IoError,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub r#type: EntryType,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FilesystemResult {
    Read {
        path: String,
        text: String,
        offset: usize,
        next_offset: Option<usize>,
        size_bytes: usize,
        revision: String,
        truncated: bool,
    },
    List {
        path: String,
        entries: Vec<Entry>,
        next_offset: Option<usize>,
        truncated: bool,
    },
    Search {
        path: String,
        matches: Vec<SearchMatch>,
        truncated: bool,
    },
    Mutation {
        path: String,
        revision: String,
        size_bytes: usize,
        created: bool,
        committed: bool,
    },
    Error {
        code: FsError,
    },
}
#[derive(Serialize)]
pub(crate) struct RedactedFilesystem {
    kind: &'static str,
    bytes: usize,
    count: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<FsError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<bool>,
}
impl FilesystemResult {
    pub(crate) fn redacted(&self) -> RedactedFilesystem {
        match self {
            Self::Read {
                text, truncated, ..
            } => RedactedFilesystem {
                kind: "read",
                bytes: text.len(),
                count: 1,
                truncated: *truncated,
                code: None,
                committed: None,
            },
            Self::List {
                entries, truncated, ..
            } => RedactedFilesystem {
                kind: "list",
                bytes: 0,
                count: entries.len(),
                truncated: *truncated,
                code: None,
                committed: None,
            },
            Self::Search {
                matches, truncated, ..
            } => RedactedFilesystem {
                kind: "search",
                bytes: matches.iter().map(|m| m.text.len()).sum(),
                count: matches.len(),
                truncated: *truncated,
                code: None,
                committed: None,
            },
            Self::Mutation {
                size_bytes,
                committed,
                ..
            } => RedactedFilesystem {
                kind: "mutation",
                bytes: *size_bytes,
                count: 1,
                truncated: false,
                code: None,
                committed: Some(*committed),
            },
            Self::Error { code } => RedactedFilesystem {
                kind: "error",
                bytes: 0,
                count: 0,
                truncated: false,
                code: Some(*code),
                committed: None,
            },
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Read,
    List,
    Search,
    Write,
    Edit,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "fs.read",
            Self::List => "fs.list",
            Self::Search => "fs.search",
            Self::Write => "fs.write",
            Self::Edit => "fs.edit",
        }
    }
}
pub(crate) struct FilesystemTool {
    operation: Operation,
    descriptor: ToolDescriptor,
    validator: jsonschema::Validator,
    policy: Arc<PolicySet>,
}
impl FilesystemTool {
    pub(crate) fn new(
        operation: Operation,
        policy: Arc<PolicySet>,
    ) -> Result<Self, ToolSetupError> {
        let mut props = json!({"path":{"type":"string","minLength":1,"maxLength":4096},"timeout_ms":{"type":"integer","minimum":1},"max_output_bytes":{"type":"integer","minimum":1024}});
        let mut required = vec!["path"];
        match operation {
            Operation::Read => {
                props["offset"] = json!({"type":"integer","minimum":0});
                props["max_bytes"] = json!({"type":"integer","minimum":1});
            }
            Operation::List => {
                props["offset"] = json!({"type":"integer","minimum":0});
                props["max_entries"] = json!({"type":"integer","minimum":1});
            }
            Operation::Search => {
                props["query"] = json!({"type":"string","minLength":1,"maxLength":4096});
                props["max_matches"] = json!({"type":"integer","minimum":1});
                required.push("query");
            }
            Operation::Write | Operation::Edit => {
                props["expected_revision"] = if matches!(operation, Operation::Write) {
                    json!({"anyOf":[{"type":"null"},{"type":"string","pattern":"^[0-9a-f]{64}$"}]})
                } else {
                    json!({"type":"string","pattern":"^[0-9a-f]{64}$"})
                };
                required.push("expected_revision");
                if matches!(operation, Operation::Write) {
                    props["text"] = json!({"type":"string"});
                    required.push("text");
                } else {
                    props["old_text"] = json!({"type":"string","minLength":1});
                    props["new_text"] = json!({"type":"string"});
                    required.extend(["old_text", "new_text"]);
                }
            }
        }
        let schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":props,"required":required});
        let validator = compile_schema(&schema)?;
        let description = match operation {
            Operation::Read => {
                "Read a bounded UTF-8 text file. Byte pages carry a full-file SHA-256 revision. No symlinks or parent traversal."
            }
            Operation::List => {
                "List sorted direct children, including hidden entries. Symlinks are listed without resolving targets. No symlink traversal."
            }
            Operation::Search => {
                "Search UTF-8 files recursively for a case-sensitive literal query. Returns matching lines; skips binary files and symlinks. Hard traversal and result limits apply."
            }
            Operation::Write => {
                "Create or atomically replace one UTF-8 text file. expected_revision null is create-only; replacement requires its SHA-256. Existing parents only. Requires explicit write authority. Replacement preserves ordinary mode but not ACLs, ownership, extended attributes or hard-link identity; hosts isolate concurrent writers."
            }
            Operation::Edit => {
                "Replace exactly one literal occurrence in a UTF-8 file with a matching SHA-256 expected_revision. Missing/ambiguous matches conflict without mutation. Atomic visibility, not external-writer isolation or crash durability. ACLs, ownership, extended attributes and hard-link identity are not preserved."
            }
        };
        Ok(Self {
            operation,
            descriptor: ToolDescriptor {
                name: operation.name().into(),
                description: description.into(),
                input_schema: schema,
            },
            validator,
            policy,
        })
    }
}
impl Tool for FilesystemTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }
    fn execute<'a>(
        &'a self,
        arguments: Value,
        context: ToolContext<'a>,
    ) -> BoxFuture<'a, ToolResult> {
        Box::pin(async move {
            if !self.validator.is_valid(&arguments)
                || arguments.as_object().is_none_or(|a| {
                    a.iter().any(|(key, v)| {
                        v.as_str().is_some_and(|s| {
                            s.contains('\0')
                                || (matches!(key.as_str(), "path" | "query") && s.len() > 4096)
                        })
                    })
                })
                || arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .is_some_and(|q| q.contains(['\n', '\r']))
            {
                return ToolResult::status(ToolStatus::InvalidArguments);
            }
            #[cfg(unix)]
            {
                let Some(workspace) = context.filesystem else {
                    return ToolResult::denied(PolicyRule::Workspace);
                };
                let Ok(workspace) = workspace.try_clone() else {
                    return *recover(FsError::IoError);
                };
                let limits = context.limits.clone();
                let requested = arguments
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(limits.max_tool_duration_ms)
                    .min(limits.max_tool_duration_ms);
                let Some(tool_deadline) =
                    Instant::now().checked_add(Duration::from_millis(requested))
                else {
                    return ToolResult::status(ToolStatus::InvalidArguments);
                };
                let deadline = context.deadline.min(tool_deadline);
                let cancellation = context.cancellation.clone();
                let operation = self.operation;
                let policy = self.policy.clone();
                let dispatch_decisions = context.policy_decisions.to_vec();
                // Await the join even after cancellation: the bounded worker owns
                // file handles and must not survive tool.finish or run.finish.
                tokio::task::spawn_blocking(move || {
                    let mut worker = unix::Worker {
                        dispatch_decisions,
                        workspace,
                        limits,
                        deadline,
                        cancellation,
                        policy,
                        visited: 0,
                        scanned: 0,
                        retained: 0,
                        #[cfg(test)]
                        after_chunk: None,
                        #[cfg(test)]
                        mutation_hooks: unix::MutationHooks::default(),
                    };
                    worker.execute(operation, &arguments)
                })
                .await
                .unwrap_or_else(|_| ToolResult::status(ToolStatus::CleanupFailed))
            }
            #[cfg(not(unix))]
            {
                let _ = context;
                ToolResult::denied(PolicyRule::UnsupportedPlatform)
            }
        })
    }
}
fn recover(code: FsError) -> Box<ToolResult> {
    let mut r = ToolResult::status(ToolStatus::RecoverableError);
    r.filesystem = Some(Box::new(FilesystemResult::Error { code }));
    Box::new(r)
}
fn stop(status: ToolStatus) -> Box<ToolResult> {
    Box::new(ToolResult::status(status))
}
fn deny(rule: PolicyRule) -> Box<ToolResult> {
    Box::new(ToolResult::denied(rule))
}
fn revision(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    let mut result = String::with_capacity(64);
    for byte in digest.as_ref() {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 15)] as char);
    }
    result
}
fn success(data: FilesystemResult) -> ToolResult {
    let mut r = ToolResult::status(ToolStatus::Completed);
    r.filesystem = Some(Box::new(data));
    r
}
fn number(args: &Value, key: &str, default: usize) -> Result<usize, Box<ToolResult>> {
    args.get(key)
        .map(|v| {
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| stop(ToolStatus::InvalidArguments))
        })
        .unwrap_or(Ok(default))
}

/// Counts serialized bytes without allocating another content-sized buffer.
pub(crate) fn fits(value: &impl Serialize, limit: usize) -> bool {
    encoded_size(value, limit).is_some()
}
fn encoded_size(value: &impl Serialize, limit: usize) -> Option<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(std::io::ErrorKind::FileTooLarge.into());
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.bytes)
}

#[cfg(unix)]
pub use unix::Workspace;
#[cfg(not(unix))]
pub struct Workspace;
#[cfg(unix)]
mod unix {
    use super::*;
    use crate::{CancellationToken, policy::relative_path};
    use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
    use std::{fs::File, io::Read};
    pub struct Workspace {
        root: File,
        path: PathBuf,
    }
    impl Workspace {
        pub(crate) fn new(path: &Path, policy: &PolicySet) -> Result<Self, &'static str> {
            policy.validate()?;
            let path = path
                .canonicalize()
                .map_err(|_| "workspace is unavailable")?;
            let root = fs::open(
                &path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map(File::from)
            .map_err(|_| "workspace is unavailable")?;
            let workspace = Self { root, path };
            for (dimension, rules) in policy.dimensions() {
                if dimension.ends_with("roots")
                    && let Some(rules) = rules
                {
                    for rule in rules.allow.iter().chain(&rules.deny) {
                        workspace
                            .open(
                                &relative_path(&rule.value).map_err(|_| "invalid policy root")?,
                                true,
                            )
                            .map_err(|_| "policy root must be an existing no-follow directory")?;
                    }
                }
            }
            Ok(workspace)
        }
        pub(super) fn try_clone(&self) -> std::io::Result<Self> {
            Ok(Self {
                root: self.root.try_clone()?,
                path: self.path.clone(),
            })
        }
        fn relative(&self, path: &str) -> Result<PathBuf, Box<ToolResult>> {
            let p = Path::new(path);
            if p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(deny(PolicyRule::Workspace));
            }
            let p = if p.is_absolute() {
                p.strip_prefix(&self.path)
                    .map_err(|_| deny(PolicyRule::Workspace))?
            } else {
                p
            };
            relative_path(
                p.to_str()
                    .ok_or_else(|| recover(FsError::UnsupportedEncoding))?,
            )
            .map_err(deny)
        }
        fn open(&self, path: &Path, directory: bool) -> Result<File, Box<ToolResult>> {
            let mut dir = self
                .root
                .try_clone()
                .map_err(|_| recover(FsError::IoError))?;
            let components: Vec<_> = path.components().collect();
            if components.is_empty() {
                return if directory {
                    Ok(dir)
                } else {
                    Err(recover(FsError::NotFile))
                };
            }
            for (i, c) in components.iter().enumerate() {
                let is_dir = i + 1 < components.len() || directory;
                let name = c.as_os_str();
                let stat = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(os_error)?;
                let kind = FileType::from_raw_mode(stat.st_mode);
                if kind == FileType::Symlink {
                    return Err(deny(PolicyRule::Symlink));
                }
                if is_dir && kind != FileType::Directory {
                    return Err(recover(FsError::NotDirectory));
                }
                if !is_dir && kind != FileType::RegularFile {
                    return Err(recover(FsError::NotFile));
                }
                let flags = OFlags::RDONLY
                    | OFlags::CLOEXEC
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | if is_dir {
                        OFlags::DIRECTORY
                    } else {
                        OFlags::empty()
                    };
                dir = fs::openat(&dir, name, flags, Mode::empty())
                    .map(File::from)
                    .map_err(os_error)?;
                let actual = FileType::from_raw_mode(fs::fstat(&dir).map_err(os_error)?.st_mode);
                if (is_dir && actual != FileType::Directory)
                    || (!is_dir && actual != FileType::RegularFile)
                {
                    return Err(recover(if is_dir {
                        FsError::NotDirectory
                    } else {
                        FsError::NotFile
                    }));
                }
            }
            Ok(dir)
        }
    }
    fn os_error(e: rustix::io::Errno) -> Box<ToolResult> {
        match e {
            rustix::io::Errno::NOENT => recover(FsError::NotFound),
            rustix::io::Errno::LOOP => deny(PolicyRule::Symlink),
            rustix::io::Errno::NOTDIR => recover(FsError::NotDirectory),
            _ => recover(FsError::IoError),
        }
    }
    fn regular_at(parent: &File, name: &std::ffi::OsStr) -> Result<File, Box<ToolResult>> {
        let stat = fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(os_error)?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Symlink => return Err(deny(PolicyRule::Symlink)),
            FileType::RegularFile => {}
            _ => return Err(recover(FsError::NotFile)),
        }
        let file = File::from(
            fs::openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(os_error)?,
        );
        if FileType::from_raw_mode(fs::fstat(&file).map_err(os_error)?.st_mode)
            != FileType::RegularFile
        {
            return Err(recover(FsError::NotFile));
        }
        Ok(file)
    }
    struct Temporary<'a> {
        parent: &'a File,
        name: String,
        active: bool,
    }
    impl Temporary<'_> {
        fn cleanup(&mut self) -> rustix::io::Result<()> {
            if self.active {
                fs::unlinkat(self.parent, self.name.as_str(), AtFlags::empty())?;
                self.active = false;
            }
            Ok(())
        }
    }
    impl Drop for Temporary<'_> {
        fn drop(&mut self) {
            let _ = self.cleanup();
        }
    }
    #[cfg(test)]
    #[derive(Default)]
    pub(super) struct MutationHooks {
        before_commit: Option<Box<dyn FnMut() + Send>>,
        after_commit: Option<Box<dyn FnMut() + Send>>,
        fail_write: bool,
        fail_rename: bool,
        fail_cleanup: bool,
    }
    pub(super) struct Worker {
        pub(super) dispatch_decisions: Vec<String>,
        pub workspace: Workspace,
        pub limits: RunLimits,
        pub deadline: Instant,
        pub cancellation: CancellationToken,
        pub policy: Arc<PolicySet>,
        pub visited: usize,
        pub scanned: usize,
        pub retained: usize,
        #[cfg(test)]
        pub after_chunk: Option<Box<dyn FnMut() + Send>>,
        #[cfg(test)]
        pub mutation_hooks: MutationHooks,
    }
    impl Worker {
        fn check(&self) -> Result<(), Box<ToolResult>> {
            if self.cancellation.is_cancelled() {
                Err(stop(ToolStatus::Cancelled))
            } else if Instant::now() >= self.deadline {
                Err(stop(ToolStatus::TimedOut))
            } else {
                Ok(())
            }
        }
        fn allow(&self, path: &Path) -> Result<Vec<String>, Box<ToolResult>> {
            self.policy
                .decide("read_roots", path.to_str().unwrap_or(""), false)
                .map_err(deny)
        }
        pub(super) fn execute(&mut self, operation: Operation, args: &Value) -> ToolResult {
            let dispatch_len = self.dispatch_decisions.len();
            let mut run = || -> Result<ToolResult, Box<ToolResult>> {
                self.check()?;
                let path = self.workspace.relative(args["path"].as_str().unwrap())?;
                let cap = number(args, "max_output_bytes", self.limits.max_tool_output_bytes)?
                    .min(self.limits.max_tool_output_bytes);
                if matches!(operation, Operation::Write | Operation::Edit) {
                    return self.mutate(&path, args, operation, cap);
                }
                let rule = self.allow(&path)?;
                self.dispatch_decisions.extend(rule);
                let mut result = match operation {
                    Operation::Read => self.read(&path, args)?,
                    Operation::List => self.list(&path, args)?,
                    Operation::Search => self.search(&path, args, cap)?,
                    Operation::Write | Operation::Edit => unreachable!(),
                };
                self.check()?;
                result.policy_decisions = self.dispatch_decisions.clone().into_boxed_slice();
                if !fits(&result, cap) {
                    return Err(stop(ToolStatus::OutputLimit));
                }
                Ok(result)
            };
            let mut result = run().unwrap_or_else(|e| *e);
            if result.policy_decisions.is_empty() {
                result.policy_decisions = self.dispatch_decisions.clone().into_boxed_slice();
            }
            self.dispatch_decisions.truncate(dispatch_len);
            result
        }
        fn mutate(
            &mut self,
            path: &Path,
            args: &Value,
            operation: Operation,
            cap: usize,
        ) -> Result<ToolResult, Box<ToolResult>> {
            use std::io::Write;
            let expected = args["expected_revision"].as_str();
            let mut decisions = Vec::new();
            if expected.is_some() {
                decisions.extend(self.allow(path)?);
            }
            decisions.extend(
                self.policy
                    .decide("write_roots", path.to_str().unwrap_or(""), true)
                    .map_err(deny)?,
            );
            self.dispatch_decisions.extend(decisions);
            let name = path.file_name().ok_or_else(|| recover(FsError::NotFile))?;
            let parent_path = path.parent().unwrap_or(Path::new(""));
            let parent = self.workspace.open(parent_path, true)?;
            let stat = match fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => Some(stat),
                Err(rustix::io::Errno::NOENT) => None,
                Err(e) => return Err(os_error(e)),
            };
            if stat
                .as_ref()
                .is_some_and(|s| FileType::from_raw_mode(s.st_mode) == FileType::Symlink)
            {
                return Err(deny(PolicyRule::Symlink));
            }
            if stat
                .as_ref()
                .is_some_and(|s| FileType::from_raw_mode(s.st_mode) != FileType::RegularFile)
            {
                return Err(recover(FsError::NotFile));
            }
            if expected.is_none() && stat.is_some() {
                return Err(recover(FsError::Conflict));
            }
            let original = if let Some(expected) = expected {
                if stat.is_none() {
                    return Err(recover(FsError::NotFound));
                }
                let original = self.read_text(regular_at(&parent, name)?, false)?;
                if revision(original.as_bytes()) != expected {
                    return Err(recover(FsError::Conflict));
                }
                Some(original)
            } else {
                None
            };
            let text = if matches!(operation, Operation::Edit) {
                let original = original.as_deref().expect("edit schema requires revision");
                let old = args["old_text"].as_str().unwrap();
                let new = args["new_text"].as_str().unwrap();
                let mut occurrences = original.match_indices(old);
                let Some((offset, _)) = occurrences.next() else {
                    return Err(recover(FsError::MatchNotUnique));
                };
                if occurrences.next().is_some() {
                    return Err(recover(FsError::MatchNotUnique));
                }
                let size = original
                    .len()
                    .checked_sub(old.len())
                    .and_then(|n| n.checked_add(new.len()))
                    .filter(|n| *n <= self.limits.filesystem.max_file_bytes)
                    .ok_or_else(|| stop(ToolStatus::WorkLimit))?;
                let mut text = String::with_capacity(size);
                text.push_str(&original[..offset]);
                text.push_str(new);
                text.push_str(&original[offset + old.len()..]);
                text
            } else {
                args["text"].as_str().unwrap().to_owned()
            };
            if text.len() > self.limits.filesystem.max_file_bytes {
                return Err(stop(ToolStatus::WorkLimit));
            }
            let mut result = success(FilesystemResult::Mutation {
                path: display(path),
                revision: revision(text.as_bytes()),
                size_bytes: text.len(),
                created: expected.is_none(),
                committed: true,
            });
            result.policy_decisions = self.dispatch_decisions.clone().into_boxed_slice();
            // Admission of the complete result precedes every side effect. A
            // long path or escaped metadata can never hide an already committed write.
            if !fits(&result, cap) {
                return Err(stop(ToolStatus::OutputLimit));
            }
            self.check()?;
            let temp_name = format!(".pablo-tmp-{}", uuid::Uuid::new_v4());
            let mut file = File::from(
                fs::openat(
                    &parent,
                    temp_name.as_str(),
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )
                .map_err(os_error)?,
            );
            let mut temporary = Temporary {
                parent: &parent,
                name: temp_name,
                active: true,
            };
            let mut committed = false;
            let outcome = (|| -> Result<(), Box<ToolResult>> {
                for chunk in text.as_bytes().chunks(8192) {
                    self.check()?;
                    #[cfg(test)]
                    if self.mutation_hooks.fail_write {
                        return Err(recover(FsError::IoError));
                    }
                    file.write_all(chunk)
                        .map_err(|_| recover(FsError::IoError))?;
                }
                // Permissions are applied to complete bytes, before installation.
                let mode = stat.as_ref().map(|s| s.st_mode & 0o777).unwrap_or(0o600);
                fs::fchmod(&file, Mode::from_bits_truncate(mode)).map_err(os_error)?;
                #[cfg(test)]
                if let Some(hook) = &mut self.mutation_hooks.before_commit {
                    hook();
                }
                self.check()?;
                if let Some(expected) = expected {
                    self.allow(path)?;
                    let current = self.read_text(regular_at(&parent, name)?, false)?;
                    if revision(current.as_bytes()) != expected {
                        return Err(recover(FsError::Conflict));
                    }
                }
                self.policy
                    .decide("write_roots", path.to_str().unwrap_or(""), true)
                    .map_err(deny)?;
                self.check()?;
                #[cfg(test)]
                if self.mutation_hooks.fail_rename {
                    return Err(recover(FsError::IoError));
                }
                if expected.is_none() {
                    // Atomic no-replace installation, portable to macOS/Linux.
                    fs::linkat(
                        &parent,
                        temporary.name.as_str(),
                        &parent,
                        name,
                        AtFlags::empty(),
                    )
                    .map_err(|e| {
                        if e == rustix::io::Errno::EXIST {
                            recover(FsError::Conflict)
                        } else {
                            os_error(e)
                        }
                    })?;
                } else {
                    fs::renameat(&parent, temporary.name.as_str(), &parent, name)
                        .map_err(os_error)?;
                    temporary.active = false;
                }
                committed = true;
                #[cfg(test)]
                if let Some(hook) = &mut self.mutation_hooks.after_commit {
                    hook();
                }
                Ok(())
            })();
            let cleanup = temporary.cleanup();
            #[cfg(test)]
            let cleanup = if self.mutation_hooks.fail_cleanup {
                Err(rustix::io::Errno::IO)
            } else {
                cleanup
            };
            if cleanup.is_err() {
                if committed {
                    result.status = ToolStatus::CleanupFailed;
                    return Ok(result);
                }
                return Err(stop(ToolStatus::CleanupFailed));
            }
            outcome?;
            // No post-commit cancellation conversion: the runtime checks its
            // token after publishing this truthful committed tool result.
            Ok(result)
        }
        fn text(&mut self, path: &Path, scan: bool) -> Result<String, Box<ToolResult>> {
            self.check()?;
            let file = self.workspace.open(path, false)?;
            self.read_text(file, scan)
        }
        fn read_text(&mut self, mut file: File, scan: bool) -> Result<String, Box<ToolResult>> {
            let mut bytes = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                self.check()?;
                let file_remaining = self
                    .limits
                    .filesystem
                    .max_file_bytes
                    .saturating_sub(bytes.len());
                let scan_remaining = if scan {
                    self.limits
                        .filesystem
                        .max_scan_bytes
                        .saturating_sub(self.scanned)
                } else {
                    usize::MAX
                };
                let length = buf
                    .len()
                    .min(file_remaining.min(scan_remaining).saturating_add(1));
                let n = file
                    .read(&mut buf[..length])
                    .map_err(|_| recover(FsError::IoError))?;
                if n == 0 {
                    break;
                }
                if n > file_remaining || n > scan_remaining {
                    return Err(stop(ToolStatus::WorkLimit));
                }
                if scan {
                    self.scanned += n;
                }
                bytes.extend_from_slice(&buf[..n]);
                #[cfg(test)]
                if let Some(hook) = &mut self.after_chunk {
                    hook();
                }
            }
            if bytes.contains(&0) {
                return Err(recover(FsError::UnsupportedEncoding));
            }
            String::from_utf8(bytes).map_err(|_| recover(FsError::UnsupportedEncoding))
        }
        fn read(&mut self, path: &Path, args: &Value) -> Result<ToolResult, Box<ToolResult>> {
            let text = self.text(path, false)?;
            let offset = number(args, "offset", 0)?;
            let max = number(args, "max_bytes", 65_536)?;
            if offset > text.len() || !text.is_char_boundary(offset) {
                return Err(recover(FsError::InvalidRange));
            }
            let mut end = offset.saturating_add(max).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == offset && offset < text.len() {
                return Err(recover(FsError::InvalidRange));
            }
            let revision = revision(text.as_bytes());
            Ok(success(FilesystemResult::Read {
                path: display(path),
                text: text[offset..end].into(),
                offset,
                next_offset: (end < text.len()).then_some(end),
                size_bytes: text.len(),
                revision,
                truncated: end < text.len(),
            }))
        }
        fn entries(&mut self, path: &Path) -> Result<Vec<Entry>, Box<ToolResult>> {
            self.check()?;
            let dir = self.workspace.open(path, true)?;
            let mut entries = Vec::new();
            for entry in fs::Dir::read_from(&dir).map_err(os_error)? {
                self.check()?;
                let entry = entry.map_err(os_error)?;
                let name = entry.file_name().to_bytes();
                if name == b"." || name == b".." {
                    continue;
                }
                if self.visited >= self.limits.filesystem.max_entries {
                    return Err(stop(ToolStatus::WorkLimit));
                }
                self.visited += 1;
                let name =
                    std::str::from_utf8(name).map_err(|_| recover(FsError::UnsupportedEncoding))?;
                let child = path.join(name);
                if child.as_os_str().len() > 4096 {
                    return Err(stop(ToolStatus::WorkLimit));
                }
                if self.allow(&child).is_err() {
                    continue;
                }
                let stat = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(os_error)?;
                let kind = match FileType::from_raw_mode(stat.st_mode) {
                    FileType::RegularFile => EntryType::File,
                    FileType::Directory => EntryType::Directory,
                    FileType::Symlink => EntryType::Symlink,
                    _ => EntryType::Other,
                };
                entries.push(Entry {
                    path: display(&child),
                    r#type: kind,
                });
            }
            entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
            Ok(entries)
        }
        fn list(&mut self, path: &Path, args: &Value) -> Result<ToolResult, Box<ToolResult>> {
            let entries = self.entries(path)?;
            let offset = number(args, "offset", 0)?;
            let max = number(args, "max_entries", 1000)?;
            if offset > entries.len() {
                return Err(recover(FsError::InvalidRange));
            }
            let end = offset.saturating_add(max).min(entries.len());
            let truncated = end < entries.len();
            Ok(success(FilesystemResult::List {
                path: display(path),
                entries: entries.into_iter().skip(offset).take(max).collect(),
                next_offset: truncated.then_some(end),
                truncated,
            }))
        }
        fn search(
            &mut self,
            path: &Path,
            args: &Value,
            cap: usize,
        ) -> Result<ToolResult, Box<ToolResult>> {
            let mut matches = Vec::new();
            let max = number(args, "max_matches", 100)?;
            let truncated = self.walk(
                path,
                args["query"].as_str().unwrap(),
                max,
                0,
                cap,
                &mut matches,
            )?;
            Ok(success(FilesystemResult::Search {
                path: display(path),
                matches,
                truncated,
            }))
        }
        fn walk(
            &mut self,
            path: &Path,
            query: &str,
            max: usize,
            depth: usize,
            cap: usize,
            matches: &mut Vec<SearchMatch>,
        ) -> Result<bool, Box<ToolResult>> {
            for entry in self.entries(path)? {
                self.check()?;
                let child = Path::new(&entry.path);
                match entry.r#type {
                    EntryType::Directory => {
                        if depth >= self.limits.filesystem.max_depth {
                            return Err(stop(ToolStatus::WorkLimit));
                        }
                        if self.walk(child, query, max, depth + 1, cap, matches)? {
                            return Ok(true);
                        }
                    }
                    EntryType::File => {
                        let text = match self.text(child, true) {
                            Ok(text) => text,
                            Err(e)
                                if matches!(
                                    e.filesystem.as_deref(),
                                    Some(FilesystemResult::Error {
                                        code: FsError::UnsupportedEncoding
                                    })
                                ) =>
                            {
                                continue;
                            }
                            Err(e) => return Err(e),
                        };
                        for (i, line) in text.split('\n').enumerate() {
                            self.check()?;
                            let line = line.strip_suffix('\r').unwrap_or(line);
                            if line.contains(query) {
                                if matches.len() >= max {
                                    return Ok(true);
                                }
                                if line.len() > cap {
                                    return Err(stop(ToolStatus::OutputLimit));
                                }
                                let found = SearchMatch {
                                    path: entry.path.clone(),
                                    line: i + 1,
                                    text: line.into(),
                                };
                                let bytes = encoded_size(&found, cap.saturating_sub(self.retained))
                                    .ok_or_else(|| stop(ToolStatus::OutputLimit))?;
                                self.retained =
                                    self.retained.saturating_add(bytes).saturating_add(1);
                                matches.push(found);
                            }
                        }
                    }
                    EntryType::Symlink | EntryType::Other => {}
                }
            }
            Ok(false)
        }
    }
    fn display(path: &Path) -> String {
        if path.as_os_str().is_empty() {
            ".".into()
        } else {
            path.to_str().expect("validated UTF-8 path").into()
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;
        struct Fixture(PathBuf);
        impl Fixture {
            fn new() -> Self {
                let p =
                    std::env::temp_dir().join(format!("pablo-fs-race-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(p.join("root/nested")).unwrap();
                std::fs::create_dir(p.join("outside")).unwrap();
                Self(p.canonicalize().unwrap())
            }
            fn worker(&self) -> Worker {
                Worker {
                    dispatch_decisions: Vec::new(),
                    workspace: Workspace::new(&self.0.join("root"), &PolicySet::default()).unwrap(),
                    limits: RunLimits::default(),
                    deadline: Instant::now() + Duration::from_secs(10),
                    cancellation: CancellationToken::new(),
                    policy: Arc::new(PolicySet::default()),
                    visited: 0,
                    scanned: 0,
                    retained: 0,
                    after_chunk: None,
                    mutation_hooks: MutationHooks::default(),
                }
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        #[test]
        fn anchored_workspace_survives_path_replacement_and_rejects_swapped_component() {
            let f = Fixture::new();
            std::fs::write(f.0.join("root/a"), "original").unwrap();
            std::fs::write(f.0.join("outside/a"), "outside").unwrap();
            let mut w = f.worker();
            std::fs::rename(f.0.join("root"), f.0.join("moved")).unwrap();
            std::os::unix::fs::symlink(f.0.join("outside"), f.0.join("root")).unwrap();
            assert_eq!(w.text(Path::new("a"), false).unwrap(), "original");
            std::fs::remove_dir(f.0.join("moved/nested")).unwrap();
            std::os::unix::fs::symlink(f.0.join("outside"), f.0.join("moved/nested")).unwrap();
            assert_eq!(
                w.text(Path::new("nested/a"), false)
                    .unwrap_err()
                    .policy_rule,
                Some(PolicyRule::Symlink)
            );
        }
        #[test]
        fn mutation_rechecks_external_changes_and_atomic_create_races() {
            let f = Fixture::new();
            let path = f.0.join("root/a");
            std::fs::write(&path, "original").unwrap();
            let mut w = f.worker();
            let target = path.clone();
            w.mutation_hooks.before_commit = Some(Box::new(move || {
                std::fs::write(&target, "external").unwrap()
            }));
            let result = w.execute(
                Operation::Write,
                &json!({"path":"a","text":"replacement","expected_revision":revision(b"original")}),
            );
            assert_eq!(
                result.filesystem.as_deref(),
                Some(&FilesystemResult::Error {
                    code: FsError::Conflict
                })
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
            let path = f.0.join("root/new");
            let target = path.clone();
            let mut w = f.worker();
            w.mutation_hooks.before_commit = Some(Box::new(move || {
                std::fs::write(&target, "created elsewhere").unwrap()
            }));
            let result = w.execute(
                Operation::Write,
                &json!({"path":"new","text":"replacement","expected_revision":null}),
            );
            assert_eq!(
                result.filesystem.as_deref(),
                Some(&FilesystemResult::Error {
                    code: FsError::Conflict
                })
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "created elsewhere");
            assert!(!std::fs::read_dir(f.0.join("root")).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".pablo-tmp-")
            }));
        }
        #[test]
        fn mutation_faults_and_precommit_cancellation_preserve_targets_and_cleanup() {
            let f = Fixture::new();
            let path = f.0.join("root/a");
            std::fs::write(&path, "original").unwrap();
            for fault in 0..4 {
                let mut w = f.worker();
                match fault {
                    0 => w.mutation_hooks.fail_write = true,
                    1 => w.mutation_hooks.fail_rename = true,
                    2 => {
                        w.mutation_hooks.fail_write = true;
                        w.mutation_hooks.fail_cleanup = true;
                    }
                    _ => {
                        let cancel = w.cancellation.clone();
                        w.mutation_hooks.before_commit = Some(Box::new(move || cancel.cancel()));
                    }
                }
                let result=w.execute(Operation::Write,&json!({"path":"a","text":"replacement","expected_revision":revision(b"original")}));
                assert_eq!(
                    result.status,
                    match fault {
                        0 | 1 => ToolStatus::RecoverableError,
                        2 => ToolStatus::CleanupFailed,
                        _ => ToolStatus::Cancelled,
                    }
                );
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
                assert!(!std::fs::read_dir(f.0.join("root")).unwrap().any(|e| {
                    e.unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".pablo-tmp-")
                }));
            }
            let mut w = f.worker();
            w.mutation_hooks.fail_cleanup = true;
            let result = w.execute(
                Operation::Write,
                &json!({"path":"new","text":"created","expected_revision":null}),
            );
            assert_eq!(result.status, ToolStatus::CleanupFailed);
            assert!(matches!(
                result.filesystem.as_deref(),
                Some(FilesystemResult::Mutation {
                    committed: true,
                    ..
                })
            ));
            assert_eq!(
                std::fs::read_to_string(f.0.join("root/new")).unwrap(),
                "created"
            );
        }
        #[test]
        fn concurrent_reader_observes_complete_bytes_before_and_after_commit() {
            let f = Fixture::new();
            let path = f.0.join("root/a");
            let old = "o".repeat(32768);
            let new = "n".repeat(32768);
            std::fs::write(&path, &old).unwrap();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut signaled = false;
                loop {
                    let bytes = std::fs::read(&path).unwrap();
                    assert!(bytes == vec![b'o'; 32768] || bytes == vec![b'n'; 32768]);
                    if !signaled {
                        assert_eq!(bytes[0], b'o');
                        ready_tx.send(()).unwrap();
                        signaled = true;
                    }
                    if bytes[0] == b'n' {
                        done_tx.send(()).unwrap();
                        break;
                    }
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::yield_now();
                }
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let mut w = f.worker();
            let token = w.cancellation.clone();
            w.mutation_hooks.after_commit = Some(Box::new(move || {
                done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                token.cancel();
            }));
            let result = w.execute(
                Operation::Write,
                &json!({"path":"a","text":new,"expected_revision":revision(old.as_bytes())}),
            );
            reader.join().unwrap();
            assert_eq!(result.status, ToolStatus::Completed);
            assert!(w.cancellation.is_cancelled());
            assert!(matches!(
                result.filesystem.as_deref(),
                Some(FilesystemResult::Mutation {
                    committed: true,
                    ..
                })
            ));
        }
        #[test]
        fn growth_after_first_read_is_still_bounded_and_cancellation_stops_mid_read() {
            let f = Fixture::new();
            let path = f.0.join("root/a");
            std::fs::write(&path, vec![b'x'; 8192]).unwrap();
            let mut w = f.worker();
            w.limits.filesystem.max_file_bytes = 8192;
            w.after_chunk = Some(Box::new(move || {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"growth")
                    .unwrap();
            }));
            assert_eq!(
                w.text(Path::new("a"), false).unwrap_err().status,
                ToolStatus::WorkLimit
            );
            let mut w = f.worker();
            let token = w.cancellation.clone();
            w.after_chunk = Some(Box::new(move || token.cancel()));
            assert_eq!(
                w.text(Path::new("a"), false).unwrap_err().status,
                ToolStatus::Cancelled
            );
        }
    }
}
