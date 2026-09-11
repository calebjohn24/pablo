use super::*;
use crate::CancellationToken;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Component, Path},
    time::Instant,
};

fn os_error(e: rustix::io::Errno) -> Error {
    match e {
        rustix::io::Errno::LOOP => Error::Symlink,
        rustix::io::Errno::NOENT => Error::NotFound,
        _ => Error::Io,
    }
}
fn at(parent: &File, name: &std::ffi::OsStr, directory: bool) -> Result<File, Error> {
    let expected = if directory {
        FileType::Directory
    } else {
        FileType::RegularFile
    };
    let kind = FileType::from_raw_mode(
        fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(os_error)?
            .st_mode,
    );
    if kind == FileType::Symlink {
        return Err(Error::Symlink);
    }
    if kind != expected {
        return Err(Error::NotRegular);
    }
    let fd = fs::openat(
        parent,
        name,
        OFlags::RDONLY
            | OFlags::CLOEXEC
            | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | if directory {
                OFlags::DIRECTORY
            } else {
                OFlags::empty()
            },
        Mode::empty(),
    )
    .map_err(os_error)?;
    if FileType::from_raw_mode(fs::fstat(&fd).map_err(os_error)?.st_mode) != expected {
        return Err(Error::NotRegular);
    }
    Ok(File::from(fd))
}
fn open_root(path: &Path) -> Result<File, Error> {
    if !path.is_absolute()
        || path.as_os_str().len() > 4096
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::InvalidRoot);
    }
    let mut fd = File::from(
        fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(os_error)?,
    );
    for component in path.components() {
        if let Component::Normal(name) = component {
            fd = at(&fd, name, true)?;
        }
    }
    Ok(fd)
}
fn check(cancel: &CancellationToken, deadline: Instant) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else if Instant::now() >= deadline {
        Err(Error::ScanBound)
    } else {
        Ok(())
    }
}
fn frontmatter(
    file: File,
    cancel: &CancellationToken,
    deadline: Instant,
    total: &mut usize,
) -> Result<Vec<u8>, Error> {
    let mut reader = BufReader::with_capacity(4096, file).take((MAX_METADATA_BYTES + 1) as u64);
    let mut line = Vec::new();
    let mut yaml = Vec::new();
    let mut first = true;
    loop {
        check(cancel, deadline)?;
        line.clear();
        let n = reader.read_until(b'\n', &mut line).map_err(|_| Error::Io)?;
        *total += n;
        if *total > MAX_SCAN_BYTES {
            return Err(Error::ScanBound);
        }
        if reader.limit() == 0 {
            return Err(Error::MetadataBound);
        }
        if n == 0 {
            return Err(Error::InvalidMetadata);
        }
        let content = line.strip_suffix(b"\n").unwrap_or(&line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if first {
            if content != b"---" {
                return Err(Error::InvalidMetadata);
            }
            first = false;
        } else if content == b"---" {
            return Ok(yaml);
        } else {
            yaml.extend_from_slice(&line);
        }
    }
}

/// Synchronous bounded discovery; the caller owns the work and cancellation lifetime.
/// Scan immediate package directories only, never scripts/resources or ambient roots.
pub fn discover(
    roots: &[Root],
    cancel: &CancellationToken,
    deadline: Instant,
) -> Result<Catalog, Error> {
    if roots.len() > MAX_ROOTS {
        return Err(Error::ScanBound);
    }
    let mut roots = roots.to_vec();
    roots.sort_by(|a, b| a.id.cmp(&b.id));
    let mut ids = BTreeSet::new();
    for root in &roots {
        if root.id.is_empty()
            || root.id.len() > 32
            || !root
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || !ids.insert(root.id.clone())
        {
            return Err(Error::InvalidRoot);
        }
    }
    let mut catalog = Catalog {
        format_revision: FORMAT_REVISION,
        roots: roots.clone(),
        entries: vec![],
        diagnostics: vec![],
        scanned_entries: 0,
        metadata_bytes: 0,
    };
    let mut physical = BTreeSet::new();
    for root in roots {
        check(cancel, deadline)?;
        let directory = match open_root(&root.path) {
            Ok(dir) => dir,
            Err(code) => {
                diagnostic(&mut catalog, &root.id, None, code)?;
                continue;
            }
        };
        let stat = fs::fstat(&directory).map_err(os_error)?;
        if !physical.insert((stat.st_dev, stat.st_ino)) {
            diagnostic(&mut catalog, &root.id, None, Error::Duplicate)?;
            continue;
        }
        let mut names = Vec::new();
        for entry in fs::Dir::read_from(&directory).map_err(os_error)? {
            check(cancel, deadline)?;
            let entry = entry.map_err(os_error)?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            catalog.scanned_entries += 1;
            if catalog.scanned_entries > MAX_ENTRIES {
                return Err(Error::ScanBound);
            }
            names.push(name.to_vec());
        }
        names.sort();
        for name in names {
            check(cancel, deadline)?;
            let Ok(name) = std::str::from_utf8(&name) else {
                diagnostic(&mut catalog, &root.id, None, Error::InvalidName)?;
                continue;
            };
            let result = (|| {
                let skill = at(&directory, std::ffi::OsStr::new(name), true)?;
                let file = at(&skill, std::ffi::OsStr::new("SKILL.md"), false)?;
                let yaml = frontmatter(file, cancel, deadline, &mut catalog.metadata_bytes)?;
                let metadata = parse_metadata(
                    std::str::from_utf8(&yaml).map_err(|_| Error::InvalidMetadata)?,
                    name,
                )?;
                let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &yaml);
                let hash: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
                Ok(Entry {
                    qualified_name: format!("{}/{}", root.id, metadata.name),
                    root: root.id.clone(),
                    relative_path: format!("{name}/SKILL.md"),
                    metadata_sha256: hash,
                    metadata,
                })
            })();
            match result {
                Ok(entry) => {
                    if catalog.entries.len() == MAX_SKILLS {
                        return Err(Error::ScanBound);
                    }
                    catalog.entries.push(entry);
                }
                Err(Error::ScanBound | Error::Cancelled) => return result.map(|_| catalog),
                Err(code) => diagnostic(&mut catalog, &root.id, Some(name), code)?,
            }
        }
    }
    catalog
        .entries
        .sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    let mut duplicates = BTreeSet::new();
    for pair in catalog.entries.windows(2) {
        if pair[0].qualified_name == pair[1].qualified_name {
            duplicates.insert(pair[0].qualified_name.clone());
        }
    }
    let mut names = BTreeMap::new();
    for entry in catalog
        .entries
        .iter()
        .filter(|e| !duplicates.contains(&e.qualified_name))
    {
        *names.entry(entry.metadata.name.clone()).or_insert(0usize) += 1;
    }
    for entry in catalog.entries.clone() {
        if duplicates.contains(&entry.qualified_name) {
            diagnostic(
                &mut catalog,
                &entry.root,
                Some(&entry.relative_path),
                Error::Duplicate,
            )?;
        } else if names[&entry.metadata.name] > 1 {
            diagnostic(
                &mut catalog,
                &entry.root,
                Some(&entry.relative_path),
                Error::Ambiguous,
            )?;
        }
    }
    catalog
        .entries
        .retain(|e| !duplicates.contains(&e.qualified_name));
    Ok(catalog)
}
fn diagnostic(
    catalog: &mut Catalog,
    root: &str,
    relative: Option<&str>,
    code: Error,
) -> Result<(), Error> {
    if catalog.diagnostics.len() == MAX_DIAGNOSTICS {
        return Err(Error::ScanBound);
    }
    catalog.diagnostics.push(Diagnostic {
        root: root.into(),
        relative_path: relative.filter(|p| p.len() <= 512).map(Into::into),
        code,
    });
    Ok(())
}
