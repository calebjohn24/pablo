use super::*;

#[cfg(unix)]
#[test]
fn discovery_enforces_aggregate_metadata_catalog_and_deadline_bounds() {
    use std::{
        fs,
        time::{Duration, Instant},
    };
    let base = std::env::temp_dir().join(format!("pablo-skills-budget-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let roots = [Root {
        id: "test".into(),
        path: base.clone(),
    }];
    let cancel = crate::CancellationToken::new();
    for n in 0..70 {
        let name = format!("s{n}");
        fs::create_dir(base.join(&name)).unwrap();
        fs::write(
            base.join(&name).join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Read.\nlicense: {}\n---\n",
                "x".repeat(16000)
            ),
        )
        .unwrap();
    }
    assert!(matches!(
        discover(&roots, &cancel, Instant::now() + Duration::from_secs(5)),
        Err(Error::ScanBound)
    ));
    for n in 0..MAX_SKILLS + 1 {
        let name = format!("s{n}");
        fs::create_dir_all(base.join(&name)).unwrap();
        fs::write(
            base.join(&name).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Read.\n---\n"),
        )
        .unwrap();
    }
    assert!(matches!(
        discover(&roots, &cancel, Instant::now() + Duration::from_secs(5)),
        Err(Error::ScanBound)
    ));
    assert!(matches!(
        discover(&roots, &cancel, Instant::now()),
        Err(Error::ScanBound)
    ));
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn discovery_bounds_total_entries_diagnostics_and_normalized_duplicates() {
    use std::{
        fs,
        time::{Duration, Instant},
    };
    let base = std::env::temp_dir().join(format!("pablo-skills-total-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let root = Root {
        id: "test".into(),
        path: base.clone(),
    };
    let cancel = crate::CancellationToken::new();
    for name in ["read", "ｒｅａｄ"] {
        fs::create_dir(base.join(name)).unwrap();
        fs::write(
            base.join(name).join("SKILL.md"),
            "---\nname: read\ndescription: Read.\n---\n",
        )
        .unwrap();
    }
    let catalog = discover(
        std::slice::from_ref(&root),
        &cancel,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert!(catalog.entries.is_empty());
    assert!(
        catalog
            .diagnostics
            .iter()
            .all(|d| d.code == Error::Duplicate)
    );
    for n in 0..MAX_DIAGNOSTICS + 1 {
        fs::write(base.join(format!("file{n}")), "").unwrap();
    }
    assert!(matches!(
        discover(
            std::slice::from_ref(&root),
            &cancel,
            Instant::now() + Duration::from_secs(5)
        ),
        Err(Error::ScanBound)
    ));
    for n in MAX_DIAGNOSTICS + 1..MAX_ENTRIES + 1 {
        fs::write(base.join(format!("file{n}")), "").unwrap();
    }
    assert!(matches!(
        discover(&[root], &cancel, Instant::now() + Duration::from_secs(5)),
        Err(Error::ScanBound)
    ));
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn discovery_is_explicit_deterministic_qualified_and_metadata_only() {
    use std::{
        fs,
        time::{Duration, Instant},
    };
    let base = std::env::temp_dir().join(format!("pablo-skills-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(base.join(".agents/skills/read")).unwrap();
    let base = base.canonicalize().unwrap();
    let root = base.join(".agents/skills");
    fs::write(
        root.join("read/SKILL.md"),
        "---\nname: read\ndescription: Read evidence.\n---\nPRIVATE_BODY\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("read/scripts")).unwrap();
    fs::write(
        root.join("read/scripts/never-run"),
        "touch SHOULD_NOT_EXIST",
    )
    .unwrap();
    fs::create_dir_all(base.join("other/read")).unwrap();
    fs::write(
        base.join("other/read/SKILL.md"),
        "---\nname: read\ndescription: Other source.\n---\n",
    )
    .unwrap();
    let roots = vec![
        Root {
            id: "workspace".into(),
            path: root,
        },
        Root {
            id: "host".into(),
            path: base.join("other"),
        },
    ];
    let cancel = crate::CancellationToken::new();
    let catalog = discover(&roots, &cancel, Instant::now() + Duration::from_secs(5)).unwrap();
    assert_eq!(
        catalog
            .entries
            .iter()
            .map(|e| e.qualified_name.as_str())
            .collect::<Vec<_>>(),
        vec!["host/read", "workspace/read"]
    );
    assert_eq!(catalog.resolve("read").unwrap_err(), Error::Ambiguous);
    assert_eq!(
        catalog
            .resolve("workspace/read")
            .unwrap()
            .metadata
            .description,
        "Read evidence."
    );
    assert_eq!(catalog.diagnostics.len(), 2);
    assert!(
        !serde_json::to_string(&catalog)
            .unwrap()
            .contains("PRIVATE_BODY")
    );
    assert!(!base.join("SHOULD_NOT_EXIST").exists());
    let reversed = discover(
        &roots.into_iter().rev().collect::<Vec<_>>(),
        &cancel,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(catalog).unwrap(),
        serde_json::to_value(reversed).unwrap()
    );
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symlinks_traversal_fifo_and_excessive_input() {
    use std::{
        fs,
        os::unix::fs::symlink,
        time::{Duration, Instant},
    };
    let base = std::env::temp_dir().join(format!("pablo-skills-bounds-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(base.join("roots/large")).unwrap();
    let base = base.canonicalize().unwrap();
    let root = base.join("roots");
    fs::write(
        root.join("large/SKILL.md"),
        format!(
            "---\nname: large\ndescription: {}",
            "x".repeat(MAX_METADATA_BYTES)
        ),
    )
    .unwrap();
    fs::create_dir(root.join("fifo")).unwrap();
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(root.join("fifo/SKILL.md"))
            .env_clear()
            .status()
            .unwrap()
            .success()
    );
    symlink(&base, root.join("escape")).unwrap();
    fs::create_dir(root.join("file-link")).unwrap();
    symlink("/dev/zero", root.join("file-link/SKILL.md")).unwrap();
    let cancel = crate::CancellationToken::new();
    let catalog = discover(
        &[Root {
            id: "test".into(),
            path: root.clone(),
        }],
        &cancel,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert!(catalog.entries.is_empty());
    assert_eq!(
        catalog
            .diagnostics
            .iter()
            .map(|d| d.code)
            .collect::<Vec<_>>(),
        vec![
            Error::Symlink,
            Error::NotRegular,
            Error::Symlink,
            Error::MetadataBound
        ]
    );
    let traversal = discover(
        &[Root {
            id: "test".into(),
            path: root.join("../roots"),
        }],
        &cancel,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(traversal.diagnostics[0].code, Error::InvalidRoot);
    symlink(&root, base.join("alias")).unwrap();
    let alias = discover(
        &[Root {
            id: "test".into(),
            path: base.join("alias"),
        }],
        &cancel,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(alias.diagnostics[0].code, Error::Symlink);
    cancel.cancel();
    assert!(matches!(
        discover(
            &[Root {
                id: "test".into(),
                path: root
            }],
            &cancel,
            Instant::now() + Duration::from_secs(5)
        ),
        Err(Error::Cancelled)
    ));
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn portable_yaml_scalars_unicode_and_optional_metadata() {
    let m = parse_metadata("name: café\ndescription: >-\n  Read a file.\n  Explain its result.\nlicense: MIT\ncompatibility: Linux\nallowed-tools: Bash(git:*) Read\nmetadata:\n  version: 1.0\n  enabled: true\n", "cafe\u{301}").unwrap();
    assert_eq!(m.name, "café");
    assert_eq!(m.description, "Read a file. Explain its result.");
    assert_eq!(m.metadata["version"], "1.0");
    assert_eq!(m.metadata["enabled"], "true");
    assert_eq!(m.allowed_tools.as_deref(), Some("Bash(git:*) Read"));
    assert_eq!(
        parse_metadata("name: 技能\ndescription: 'Read: carefully'\n", "技能")
            .unwrap()
            .description,
        "Read: carefully"
    );
}

#[test]
fn invalid_ambiguous_yaml_and_nonportable_fields_are_bounded_errors() {
    for input in [
        "name: test\nname: test\ndescription: text",
        "name: test\ndescription: text\nmetadata: {x: a, x: b}",
        "name: &name test\ndescription: *name",
        "name: test\ndescription: !!str text",
        "name: test\ndescription: [a, b]",
        "name: test\ndescription: text\nmetadata: {x: {y: z}}",
        "name: test\ndescription: text\nextra: unknown",
        "name: test\ndescription: text\n---\nname: test",
        "name: test\ndescription: ''",
        "name: test\ndescription: text\ncompatibility: ''",
        "name: Test\ndescription: text",
        "name: test--name\ndescription: text",
    ] {
        assert!(parse_metadata(input, "test").is_err(), "{input}");
    }
    assert_eq!(
        parse_metadata(&"a".repeat(MAX_METADATA_BYTES + 1), "test"),
        Err(Error::MetadataBound)
    );
    assert_eq!(
        parse_metadata("name: other\ndescription: text", "test"),
        Err(Error::DirectoryMismatch)
    );
}
