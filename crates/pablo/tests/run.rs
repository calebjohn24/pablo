use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pablo-run-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pablo"));
        cmd.current_dir(&self.0)
            .env_remove("PABLO_FIXTURE_ENDPOINT")
            .env_remove("VERCEL_AI_GATEWAY")
            .env_remove("AI_GATEWAY_API_KEY");
        cmd
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn frame(delta: Value, finish: Value) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}
fn tool_reply(command: &str) -> String {
    let arguments = json!({"command":command,"cwd":"."}).to_string();
    let mut output = frame(
        json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"shell_run","arguments":""}}]}),
        Value::Null,
    );
    for part in arguments.as_bytes().chunks(3) {
        output += &frame(
            json!({"tool_calls":[{"index":0,"function":{"arguments":std::str::from_utf8(part).unwrap()}}]}),
            Value::Null,
        );
    }
    output + &frame(json!({}), json!("tool_calls")) + "data: [DONE]\n\n"
}
fn request(stream: &mut TcpStream) -> (String, Value) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 8192);
    }
    let header = String::from_utf8(bytes).unwrap();
    let count: usize = header
        .lines()
        .find_map(|line| {
            line.to_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_owned)
        })
        .unwrap()
        .parse()
        .unwrap();
    assert!(count < 2 * 1024 * 1024);
    let mut body = vec![0; count];
    stream.read_exact(&mut body).unwrap();
    (header, serde_json::from_slice(&body).unwrap())
}
fn serve(mut stream: TcpStream, body: &str, status: &str, content_type: &str) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    // Keep ordinary responses fragmented; oversized fixtures need not issue millions of writes.
    let chunk_bytes = if body.len() > 1024 * 1024 { 4096 } else { 7 };
    for chunk in body.as_bytes().chunks(chunk_bytes) {
        if stream.write_all(chunk).is_err() {
            break;
        }
    }
}
fn accept(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5))
            }
            other => panic!("fixture did not receive expected request: {other:?}"),
        }
    }
}
fn server() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    (listener, endpoint)
}
fn events(fixture: &Fixture) -> Vec<Value> {
    fs::read_to_string(fixture.0.join("trace.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn end_user_executable_reads_real_evidence_over_fragmented_http() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("evidence.txt"), "plum-713\n").unwrap();
    fs::write(
        fixture.0.join(".env"),
        "AI_GATEWAY_API_KEY=synthetic-private\n",
    )
    .unwrap();
    let (listener, endpoint) = server();
    let worker = thread::spawn(move || {
        let mut first = accept(&listener);
        let (headers, body) = request(&mut first);
        assert!(headers.contains("Bearer pablo-local-fixture"));
        assert!(!headers.contains("synthetic-private"));
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "google/gemini-3.8-flash");
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["tools"][0]["function"]["name"], "shell_run");
        serve(
            first,
            &tool_reply("cat evidence.txt"),
            "200 OK",
            "text/event-stream",
        );
        let mut second = accept(&listener);
        let (_, next) = request(&mut second);
        assert_eq!(next["messages"][0], body["messages"][0]);
        assert_eq!(next["tools"], body["tools"]);
        assert_eq!(
            next["messages"][2]["tool_calls"][0]["function"]["name"],
            "shell_run"
        );
        let result: Value =
            serde_json::from_str(next["messages"][3]["content"].as_str().unwrap()).unwrap();
        assert_eq!(result["shell"]["stdout"], "plum-713\n");
        assert_eq!(result["shell"]["exit_code"], 0);
        assert!(!next.to_string().contains("synthetic-private"));
        let response = frame(json!({"content":"Evidence: plum-713 🟣"}), Value::Null)
            + &frame(json!({}), json!("stop"))
            + "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":8}}\n\ndata: [DONE]\n\n";
        serve(
            second,
            &response,
            "200 OK",
            "text/event-stream; charset=utf-8",
        );
    });
    let output = fixture
        .command()
        .args([
            "run",
            "Read evidence.txt and report its contents",
            "--trace",
            "trace.jsonl",
            "--timeout",
            "5",
        ])
        .env("PABLO_FIXTURE_ENDPOINT", endpoint)
        .env("AI_GATEWAY_API_KEY", "synthetic-private")
        .output()
        .unwrap();
    worker.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Evidence: plum-713 🟣\n"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("cat evidence.txt"));
    let records = events(&fixture);
    assert_eq!(records.last().unwrap()["outcome"]["status"], "completed");
    assert_eq!(
        records
            .iter()
            .filter(|e| e["type"] == "shell.started")
            .count(),
        1
    );
    let raw = fs::read_to_string(fixture.0.join("trace.jsonl")).unwrap();
    assert!(!raw.contains("plum-713"));
    assert!(!raw.contains("synthetic-private"));
    assert_eq!(
        fs::read_to_string(fixture.0.join(".env")).unwrap(),
        "AI_GATEWAY_API_KEY=synthetic-private\n"
    );
}

#[test]
fn text_only_has_no_tools_and_flushes_before_response_finishes() {
    let fixture = Fixture::new();
    let (listener, endpoint) = server();
    let (ready, received) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let mut stream = accept(&listener);
        let (_, body) = request(&mut stream);
        assert!(body.get("tools").is_none());
        assert_eq!(body["model"], "fixture/test");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream
            .write_all(frame(json!({"content":"first"}), Value::Null).as_bytes())
            .unwrap();
        received.recv_timeout(Duration::from_secs(3)).unwrap();
        stream
            .write_all((frame(json!({}), json!("stop")) + "data: [DONE]\n\n").as_bytes())
            .unwrap();
    });
    let mut child = fixture
        .command()
        .args([
            "run",
            "Say hello",
            "--no-shell",
            "--no-filesystem",
            "--model",
            "fixture/test",
            "--timeout",
            "5",
        ])
        .env("PABLO_FIXTURE_ENDPOINT", endpoint)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut prefix = [0; 5];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut prefix)
        .unwrap();
    assert_eq!(&prefix, b"first");
    ready.send(()).unwrap();
    let output = child.wait_with_output().unwrap();
    worker.join().unwrap();
    assert!(output.status.success());
}

#[test]
fn malformed_oversized_rejected_and_redirect_responses_are_safe() {
    let cases = [
        (
            "200 OK",
            "text/event-stream",
            "data: bad-synthetic-secret\n\n".to_owned(),
            "MalformedStream",
        ),
        (
            "200 OK",
            "text/event-stream",
            frame(json!({"content":"partial"}), Value::Null),
            "MalformedStream",
        ),
        (
            "200 OK",
            "text/event-stream",
            "data: ".to_owned() + &"x".repeat(32 * 1024 * 1024),
            "MalformedStream",
        ),
        (
            "200 OK",
            "application/json",
            "synthetic-secret".into(),
            "MalformedStream",
        ),
        (
            "401 Unauthorized",
            "application/json",
            "synthetic-secret".into(),
            "ProviderRejected",
        ),
        (
            "302 Found",
            "text/plain",
            "synthetic-secret".into(),
            "ProviderRejected",
        ),
    ];
    for (status, content_type, body, code) in cases {
        let fixture = Fixture::new();
        let (listener, endpoint) = server();
        let worker = thread::spawn(move || {
            let mut stream = accept(&listener);
            request(&mut stream);
            serve(stream, &body, status, content_type);
        });
        let output = fixture
            .command()
            .args([
                "run",
                "Hello",
                "--no-shell",
                "--timeout",
                "3",
                "--trace",
                "trace.jsonl",
            ])
            .env("PABLO_FIXTURE_ENDPOINT", endpoint)
            .output()
            .unwrap();
        worker.join().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(code), "{stderr}");
        assert!(!stderr.contains("synthetic-secret"));
        let records = events(&fixture);
        assert_eq!(
            records
                .iter()
                .filter(|e| e["type"] == "run.finished")
                .count(),
            1
        );
        assert_eq!(records.last().unwrap()["outcome"]["status"], "failed");
    }
}

#[test]
fn private_config_errors_and_fixture_endpoint_validation_do_not_connect() {
    let fixture = Fixture::new();
    let missing = fixture.command().args(["run", "Hello"]).output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("AI_GATEWAY_API_KEY"));
    fs::write(
        fixture.0.join("config.env"),
        "AI_GATEWAY_API_KEY='synthetic secret'\n",
    )
    .unwrap();
    let invalid = fixture
        .command()
        .args(["run", "Hello", "--env-file", "config.env"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid AI_GATEWAY_API_KEY"));
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("synthetic secret"));
    fs::write(
        fixture.0.join(".env"),
        "VERCEL_AI_GATEWAY='synthetic alias secret'\n",
    )
    .unwrap();
    let alias = fixture.command().args(["run", "Hello"]).output().unwrap();
    assert!(!alias.status.success());
    assert!(String::from_utf8_lossy(&alias.stderr).contains("invalid AI_GATEWAY_API_KEY"));
    assert!(!String::from_utf8_lossy(&alias.stderr).contains("synthetic alias secret"));
    for endpoint in [
        "https://example.com",
        "http://localhost:1",
        "http://127.0.0.1:1/?key=x",
        "http://name:secret@127.0.0.1:1/",
    ] {
        let output = fixture
            .command()
            .args(["run", "Hello"])
            .env("PABLO_FIXTURE_ENDPOINT", endpoint)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("fixture endpoint"));
    }
}

#[cfg(unix)]
#[test]
fn ctrl_c_waits_for_shell_cleanup_and_reports_cancelled() {
    let fixture = Fixture::new();
    let (listener, endpoint) = server();
    let worker = thread::spawn(move || {
        let mut stream = accept(&listener);
        request(&mut stream);
        serve(
            stream,
            &tool_reply("echo $$ > child.pid; exec sleep 30"),
            "200 OK",
            "text/event-stream",
        );
    });
    let mut child = fixture
        .command()
        .args([
            "run",
            "Wait",
            "--json",
            "--timeout",
            "5",
            "--trace",
            "trace.jsonl",
        ])
        .env("PABLO_FIXTURE_ENDPOINT", endpoint)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while !fixture.0.join("child.pid").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(fixture.0.join("child.pid").exists());
    let pid: i32 = fs::read_to_string(fixture.0.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline + Duration::from_secs(3)
    {
        thread::sleep(Duration::from_millis(5));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
        panic!("Ctrl-C did not settle");
    }
    let output = child.wait_with_output().unwrap();
    worker.join().unwrap();
    assert_eq!(
        output.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let task: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(task["outcome"]["status"], "cancelled");
    assert_eq!(task["accounting"]["tool_calls"], "1");
    assert_eq!(
        task["accounting"],
        events(&fixture).last().unwrap()["accounting"]
    );
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        events(&fixture).last().unwrap()["outcome"]["status"],
        "cancelled"
    );
}

#[test]
fn run_deadline_stops_a_stalled_http_response() {
    let fixture = Fixture::new();
    let (listener, endpoint) = server();
    let worker = thread::spawn(move || {
        let mut stream = accept(&listener);
        request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    });
    let start = Instant::now();
    let output = fixture
        .command()
        .args([
            "run",
            "Hello",
            "--no-shell",
            "--timeout",
            "1",
            "--trace",
            "trace.jsonl",
        ])
        .env("PABLO_FIXTURE_ENDPOINT", endpoint)
        .output()
        .unwrap();
    worker.join().unwrap();
    assert!(!output.status.success());
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_eq!(
        events(&fixture).last().unwrap()["outcome"]["status"],
        "timed_out"
    );
}

#[test]
fn json_and_shorthand_preserve_exact_accounting_and_terminal_truth() {
    for shorthand in [false, true] {
        let fixture = Fixture::new();
        let (listener, endpoint) = server();
        let worker = thread::spawn(move || {
            let mut stream = accept(&listener);
            request(&mut stream);
            let response = frame(json!({"content":"line\n\"🙂\" {\"ok\":true}"}), Value::Null)
                + &frame(json!({}), json!("stop"))
                + "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9007199254740993,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
            serve(stream, &response, "200 OK", "text/event-stream");
        });
        let mut command = fixture.command();
        if !shorthand {
            command.arg("run");
        }
        let output = command
            .args([
                "Hello",
                "--json",
                "--trace",
                "trace.jsonl",
                "--capture-content",
            ])
            .env("PABLO_FIXTURE_ENDPOINT", endpoint)
            .output()
            .unwrap();
        worker.join().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let raw = String::from_utf8(output.stdout).unwrap();
        assert_eq!(raw.lines().count(), 1);
        assert!(raw.ends_with('\n'));
        let task: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(task["outcome"]["output"], "line\n\"🙂\" {\"ok\":true}");
        assert_eq!(
            task["accounting"]["usage"]["input_tokens"],
            "9007199254740993"
        );
        assert_eq!(task["accounting"]["usage"]["output_tokens"], "2");
        assert_eq!(
            task["accounting"]["usage"]["cache_read_input_tokens"],
            Value::Null
        );
        assert_eq!(task["accounting"]["model_calls"], "1");
        assert_eq!(task["accounting"]["tool_calls"], "0");
        let records = events(&fixture);
        let terminal = records.last().unwrap();
        for key in ["run_id", "session_id", "trace_id", "accounting", "outcome"] {
            assert_eq!(task[key], terminal[key]);
        }
    }
}

#[test]
fn json_pre_admission_errors_are_closed_and_do_not_dispatch() {
    for (args, code, endpoint) in [
        (
            vec!["run", "private-task", "--json", "--bad"],
            "invalid_arguments",
            false,
        ),
        (
            vec![
                "run",
                "private-task",
                "--json",
                "--workspace",
                "/nonexistent-pablo-workspace",
            ],
            "invalid_configuration",
            false,
        ),
        (
            vec!["run", "private-task", "--json"],
            "credential_unavailable",
            false,
        ),
        (
            vec!["run", "private-task", "--json", "--trace", "absent/trace"],
            "trace_setup_failed",
            true,
        ),
    ] {
        let fixture = Fixture::new();
        let mut command = fixture.command();
        command.args(args);
        if endpoint {
            command.env(
                "PABLO_FIXTURE_ENDPOINT",
                "http://127.0.0.1:1/v1/chat/completions",
            );
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains("private-task"));
        let task: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(task["error"]["code"], code);
        for key in ["run_id", "session_id", "trace_id", "accounting", "outcome"] {
            assert!(task[key].is_null());
        }
    }
}

#[test]
fn json_failure_and_zero_limit_retain_native_accounting() {
    for fail in [false, true] {
        let fixture = Fixture::new();
        let (listener, endpoint) = server();
        let worker = fail.then(|| {
            thread::spawn(move || {
                let mut stream = accept(&listener);
                request(&mut stream);
                serve(stream, "data: invalid\n\n", "200 OK", "text/event-stream");
            })
        });
        let mut command = fixture.command();
        command.args(["run", "Hello", "--json", "--trace", "trace.jsonl"]);
        if !fail {
            command.args(["--max-model-calls", "0"]);
        }
        let output = command
            .env("PABLO_FIXTURE_ENDPOINT", endpoint)
            .output()
            .unwrap();
        if let Some(worker) = worker {
            worker.join().unwrap();
        }
        assert_eq!(output.status.code(), Some(1));
        let task: Value = serde_json::from_slice(&output.stdout).unwrap();
        let records = events(&fixture);
        assert_eq!(task["accounting"], records.last().unwrap()["accounting"]);
        assert_eq!(task["outcome"], records.last().unwrap()["outcome"]);
        assert_eq!(
            task["accounting"]["model_calls"],
            if fail { "1" } else { "0" }
        );
        assert_eq!(
            task["accounting"]["usage"]["input_tokens"],
            if fail { Value::Null } else { json!("0") }
        );
    }
}

#[test]
fn invalid_host_policy_is_rejected_before_provider_delivery() {
    for policy in [
        r#"{"unknown":{}}"#.to_owned(),
        r#"{"read_roots":{"default":"allow","allow":[{"id":"outside","value":"../outside"}]}}"#.into(),
        r#"{"tools":{"default":"allow","allow":[{"id":"duplicate","value":"fs.read"},{"id":"duplicate","value":"fs.list"}]}}"#.into(),
        " ".repeat(1024*1024+1),
    ] {
        let fixture=Fixture::new();fs::write(fixture.0.join("policy.json"),policy).unwrap();
        let output=fixture.command().args(["run","fixture","--json","--policy","policy.json"])
            .env("PABLO_FIXTURE_ENDPOINT","http://127.0.0.1:1/v1/chat/completions").output().unwrap();
        assert_eq!(output.status.code(),Some(2));let task:Value=serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(task["error"]["code"],"invalid_configuration");assert!(task["run_id"].is_null());
    }
}
