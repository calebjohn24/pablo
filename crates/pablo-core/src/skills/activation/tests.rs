use super::*;
use std::{fs, time::Duration};
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let base =
            std::env::temp_dir().join(format!("pablo-skill-activate-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&base).unwrap();
        Self(base.canonicalize().unwrap())
    }
    fn skill(&self, name: &str, body: &str) {
        fs::create_dir_all(self.0.join(name)).unwrap();
        fs::write(
            self.0.join(name).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Read evidence.\n---\n{body}"),
        )
        .unwrap();
    }
    fn roots(&self) -> Vec<Root> {
        vec![Root {
            id: "host".into(),
            path: self.0.clone(),
        }]
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(5)
}

#[tokio::test]
async fn instructions_are_sorted_hashed_and_resources_are_lazy() {
    let f = Fixture::new();
    f.skill("zebra", "Read references/evidence.txt when requested.");
    f.skill("alpha", "Use only permitted tools.");
    let cancel = CancellationToken::new();
    let active = ActivatedSkills::load(
        f.roots(),
        vec!["zebra".into(), "host/alpha".into()],
        cancel.clone(),
        deadline(),
    )
    .await
    .unwrap();
    let records = active.records();
    assert_eq!(
        records
            .iter()
            .map(|r| r.qualified_name.as_str())
            .collect::<Vec<_>>(),
        vec!["host/alpha", "host/zebra"]
    );
    assert_eq!(
        records[1].instruction_sha256,
        digest(&fs::read(f.0.join("zebra/SKILL.md")).unwrap())
    );
    assert_eq!(
        active.instructions()[1].1,
        "Read references/evidence.txt when requested."
    );
    assert!(
        !serde_json::to_string(&records)
            .unwrap()
            .contains("Read references")
    );
    assert!(!f.0.join("zebra/references").exists());
    fs::create_dir(f.0.join("zebra/references")).unwrap();
    fs::write(f.0.join("zebra/references/evidence.txt"), "fresh resource").unwrap();
    let resource = active
        .read_resource(
            "host/zebra".into(),
            "references/evidence.txt".into(),
            1024,
            cancel.clone(),
            deadline(),
        )
        .await
        .unwrap();
    assert_eq!(resource.text, "fresh resource");
    assert_eq!(resource.sha256, digest(b"fresh resource"));
    assert_eq!(resource.bytes, 14);
    fs::write(
        f.0.join("zebra/references/evidence.txt"),
        "changed resource",
    )
    .unwrap();
    assert_eq!(
        active
            .read_resource(
                "host/zebra".into(),
                "references/evidence.txt".into(),
                1024,
                cancel,
                deadline()
            )
            .await
            .unwrap()
            .text,
        "changed resource"
    );
}

#[tokio::test]
async fn resource_authority_never_expands_to_unselected_or_escaped_paths() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.skill("selected", "Selected.");
    f.skill("other", "Not selected.");
    fs::write(f.0.join("other/private.txt"), "PRIVATE_OUTSIDE").unwrap();
    symlink(f.0.join("other"), f.0.join("selected/linked")).unwrap();
    symlink(f.0.join("other/private.txt"), f.0.join("selected/link.txt")).unwrap();
    let cancel = CancellationToken::new();
    let active = ActivatedSkills::load(
        f.roots(),
        vec!["selected".into()],
        cancel.clone(),
        deadline(),
    )
    .await
    .unwrap();
    for path in [
        "../other/private.txt",
        "/etc/passwd",
        "linked/private.txt",
        "link.txt",
    ] {
        assert!(
            active
                .read_resource(
                    "host/selected".into(),
                    path.into(),
                    1024,
                    cancel.clone(),
                    deadline()
                )
                .await
                .is_err(),
            "{path}"
        );
    }
    assert_eq!(
        active
            .read_resource(
                "host/other".into(),
                "private.txt".into(),
                1024,
                cancel.clone(),
                deadline()
            )
            .await
            .unwrap_err(),
        Error::NotFound
    );
    assert!(matches!(
        ActivatedSkills::load(
            f.roots(),
            vec!["selected".into(), "host/selected".into()],
            cancel.clone(),
            deadline()
        )
        .await,
        Err(Error::Duplicate)
    ));
    fs::write(f.0.join("selected/large.txt"), vec![b'x'; 1025]).unwrap();
    assert_eq!(
        active
            .read_resource(
                "host/selected".into(),
                "large.txt".into(),
                1024,
                cancel.clone(),
                deadline()
            )
            .await
            .unwrap_err(),
        Error::ResourceBound
    );
    cancel.cancel();
    assert_eq!(
        active
            .read_resource(
                "host/selected".into(),
                "large.txt".into(),
                1024,
                cancel.clone(),
                deadline()
            )
            .await
            .unwrap_err(),
        Error::Cancelled
    );
    assert!(matches!(
        ActivatedSkills::load(f.roots(), vec!["selected".into()], cancel, deadline()).await,
        Err(Error::Cancelled)
    ));
}

#[tokio::test]
async fn instructions_and_special_resource_files_cannot_bypass_bounds() {
    let f = Fixture::new();
    f.skill("large", &"x".repeat(MAX_INSTRUCTION_BYTES));
    let cancel = CancellationToken::new();
    assert!(matches!(
        ActivatedSkills::load(f.roots(), vec!["large".into()], cancel.clone(), deadline()).await,
        Err(Error::ResourceBound)
    ));
    f.skill("large", "Read only selected regular resources.");
    let active = ActivatedSkills::load(f.roots(), vec!["large".into()], cancel.clone(), deadline())
        .await
        .unwrap();
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(f.0.join("large/fifo"))
            .env_clear()
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        active
            .read_resource("host/large".into(), "fifo".into(), 1024, cancel, deadline())
            .await
            .unwrap_err(),
        Error::NotRegular
    );
}

#[tokio::test]
async fn cancellation_during_an_owned_read_waits_for_file_work_to_join() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    struct ControlledRead {
        file: File,
        entered: Option<mpsc::Sender<()>>,
        release: mpsc::Receiver<()>,
        closed: Arc<AtomicBool>,
    }
    impl Read for ControlledRead {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
                self.release.recv().unwrap();
            }
            self.file.read(buffer)
        }
    }
    impl Drop for ControlledRead {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }
    let f = Fixture::new();
    fs::write(f.0.join("resource"), vec![b'x'; 65536]).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let closed = Arc::new(AtomicBool::new(false));
    let reader = ControlledRead {
        file: File::open(f.0.join("resource")).unwrap(),
        entered: Some(entered_tx),
        release: release_rx,
        closed: closed.clone(),
    };
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = tokio::task::spawn_blocking(move || {
        bounded_read(
            reader,
            MAX_RESOURCE_BYTES,
            &worker_cancel,
            Instant::now() + Duration::from_secs(5),
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cancel.cancel();
    assert!(!closed.load(Ordering::SeqCst));
    release_tx.send(()).unwrap();
    assert_eq!(worker.await.unwrap().unwrap_err(), Error::Cancelled);
    assert!(closed.load(Ordering::SeqCst));
}
