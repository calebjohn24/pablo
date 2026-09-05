use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pablo-c11-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pablo"));
        command.current_dir(&self.0);
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn built_executable_streams_offline_and_writes_a_private_redacted_trace() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join(".env"),
        "AI_GATEWAY_API_KEY=synthetic-do-not-read\n",
    )
    .unwrap();
    let result = fixture
        .command()
        .args(["demo", "--trace", "run.jsonl"])
        .env("AI_GATEWAY_API_KEY", "synthetic-do-not-export")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"Hello from pablo.\n");
    assert!(result.stderr.is_empty());
    let trace = fs::read_to_string(fixture.0.join("run.jsonl")).unwrap();
    assert!(!trace.contains("Hello"));
    assert!(!trace.contains("synthetic-do-not"));
    let events: Vec<serde_json::Value> = trace
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 7);
    assert_eq!(events[6]["outcome"]["status"], "completed");
    assert_eq!(events[6]["outcome"]["output"], serde_json::Value::Null);
    assert_eq!(
        fs::read_to_string(fixture.0.join(".env")).unwrap(),
        "AI_GATEWAY_API_KEY=synthetic-do-not-read\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(fixture.0.join("run.jsonl"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn content_capture_is_explicit_and_existing_trace_files_are_preserved() {
    let fixture = Fixture::new();
    let result = fixture
        .command()
        .args(["demo", "--trace", "run.jsonl", "--capture-content"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let original = fs::read(fixture.0.join("run.jsonl")).unwrap();
    assert!(String::from_utf8_lossy(&original).contains("Hello from pablo."));
    let result = fixture
        .command()
        .args(["demo", "--trace", "run.jsonl"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(original, fs::read(fixture.0.join("run.jsonl")).unwrap());
}

#[test]
fn help_version_and_invalid_options_are_honest_about_the_checkpoint() {
    let fixture = Fixture::new();
    let help = fixture.command().arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("C1.2"));
    let version = fixture.command().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout)
            .contains("fee465db333bdd6a7d2faa320edab5cf3101a4f4")
    );
    for args in [
        vec!["acp", "--stdio"],
        vec!["demo", "--capture-content"],
        vec!["demo", "--trace"],
        vec!["demo", "--wat"],
        vec!["--help", "extra"],
    ] {
        let result = fixture.command().args(args).output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
    }
}
