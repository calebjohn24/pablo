use super::{Server, session::Session};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "release-only sequential performance measurement, separate from acceptance tests"]
async fn measure_http() {
    fn stats(values: &[f64]) -> Value {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        json!({"n":sorted.len(),"min":sorted[0],"p50":sorted[14],"p95":sorted[28],"max":sorted[29]})
    }
    let mut reports = serde_json::Map::new();
    for mode in ["json", "sse"] {
        let fixture = Fixture::new(mode).await;
        let definition: Server = serde_json::from_value(
            json!({"transport":"http","url":"https://synthetic.example/mcp"}),
        )
        .unwrap();
        let mut startup = Vec::new();
        let mut call = Vec::new();
        let mut close = Vec::new();
        for i in 0..35 {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "x-fixture-token",
                reqwest::header::HeaderValue::from_static("synthetic-http-token"),
            );
            let cancellation = CancellationToken::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            let began = std::time::Instant::now();
            let mut session = Session::start_http(
                &definition,
                headers,
                Some(&fixture.endpoint),
                deadline,
                &cancellation,
            )
            .await
            .unwrap();
            let startup_ms = began.elapsed().as_secs_f64() * 1000.;
            let began = std::time::Instant::now();
            let result = session
                .call("read_evidence", json!({}), deadline, &cancellation)
                .await
                .unwrap();
            let call_ms = began.elapsed().as_secs_f64() * 1000.;
            assert_eq!(result.structured.unwrap()["text"], "fresh HTTP evidence");
            let began = std::time::Instant::now();
            session.close().await.unwrap();
            let close_ms = began.elapsed().as_secs_f64() * 1000.;
            if i >= 5 {
                startup.push(startup_ms);
                call.push(call_ms);
                close.push(close_ms);
            }
        }
        reports.insert(mode.into(),json!({"startup_ms":stats(&startup),"call_ms":stats(&call),"close_ms":stats(&close),"raw":{"startup_ms":startup,"call_ms":call,"close_ms":close}}));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let destination = root.join(".pablo/measurements/c3.17-http-native.json");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(destination,serde_json::to_vec_pretty(&json!({"optimized":!cfg!(debug_assertions),"samples":30,"warmup":5,"method":"Release shared MCP session, fresh reqwest client/session/catalog per sample; one independent Python SDK process per format. Startup includes initialize/discovery; call reads actual evidence; close includes local SDK join and DELETE. Server/fixture construction excluded; loopback HTTP, no TLS or live model latency.","reports":reports})).unwrap()).unwrap();
}

struct Fixture {
    child: Child,
    cwd: PathBuf,
    endpoint: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.cwd);
    }
}
impl Fixture {
    async fn new(mode: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let cwd = std::env::temp_dir().join(format!("pablo-mcp-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("evidence.txt"), "fresh HTTP evidence").unwrap();
        let child = Command::new(root.join(".pablo/mcp-fixture-venv/bin/python"))
            .arg(root.join(if matches!(mode, "json" | "sse") {
                "tests/fixtures/mcp/http_server.py"
            } else {
                "tests/fixtures/mcp/http_fault_server.py"
            }))
            .arg(mode)
            .current_dir(&cwd)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut fixture = Self {
            child,
            cwd,
            endpoint: String::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(endpoint) = std::fs::read_to_string(fixture.cwd.join("endpoint"))
                && reqwest::Url::parse(&endpoint).is_ok()
            {
                fixture.endpoint = endpoint;
                break;
            }
            assert!(
                fixture.child.try_wait().unwrap().is_none(),
                "independent HTTP server exited"
            );
            assert!(
                Instant::now() < deadline,
                "independent HTTP startup timed out"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        fixture
    }
}

#[tokio::test]
#[ignore = "requires the isolated Python fixture interpreter"]
async fn http_failures_do_not_redirect_replay_or_claim_remote_cancellation() {
    for mode in [
        "redirect",
        "session_bound",
        "disconnect",
        "frame_bound",
        "progress_bound",
        "wrong_id",
        "session_changed",
        "session_expired",
        "call_hang",
        "delete_405",
    ] {
        let fixture = Fixture::new(mode).await;
        let definition: Server = serde_json::from_value(
            json!({"transport":"http","url":"https://synthetic.example/mcp"}),
        )
        .unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-fixture-token",
            reqwest::header::HeaderValue::from_static("synthetic-http-token"),
        );
        let cancellation = CancellationToken::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let started = Session::start_http(
            &definition,
            headers,
            Some(&fixture.endpoint),
            deadline,
            &cancellation,
        )
        .await;
        if matches!(mode, "redirect" | "session_bound") {
            assert!(started.is_err(), "{mode}");
        } else {
            let mut session = started.unwrap();
            let cancel = cancellation.clone();
            let trigger = tokio::spawn(async move {
                if mode == "call_hang" {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    cancel.cancel();
                }
            });
            let result = session
                .call("read", json!({}), deadline, &cancellation)
                .await;
            trigger.await.unwrap();
            if mode == "delete_405" {
                assert!(result.is_ok());
                assert!(!session.remote_completion_uncertain());
            } else {
                assert!(result.is_err(), "{mode}");
                assert!(session.remote_completion_uncertain(), "{mode}");
            }
            if mode == "call_hang" {
                assert_eq!(result.unwrap_err(), super::session::Error::Cancelled);
            }
            session.close().await.unwrap();
        }
        let records: Vec<Value> = std::fs::read_to_string(fixture.cwd.join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            records
                .iter()
                .filter(|r| r["method"] == "initialize")
                .count(),
            1,
            "{mode}"
        );
        assert!(
            records.iter().all(|r| r["path"] == "/mcp"),
            "redirect was followed"
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| r["method"] == "tools/call")
                .count(),
            usize::from(!matches!(mode, "redirect" | "session_bound")),
            "tool call replayed"
        );
        if mode == "call_hang" || mode == "disconnect" {
            let call = records
                .iter()
                .find(|r| r["method"] == "tools/call")
                .unwrap();
            let cancel = records
                .iter()
                .find(|r| r["method"] == "notifications/cancelled")
                .expect("protocol cancellation must reach peer");
            assert_eq!(cancel["params"]["requestId"], call["id"]);
        }
    }
}

#[tokio::test]
#[ignore = "requires the pinned independent Python SDK fixture environment"]
async fn independent_stateful_http_json_and_sse_preserve_requests_and_teardown() {
    for mode in ["json", "sse"] {
        let fixture = Fixture::new(mode).await;
        let definition: Server = serde_json::from_value(
            json!({"transport":"http","url":"https://synthetic.example/mcp"}),
        )
        .unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-fixture-token",
            reqwest::header::HeaderValue::from_static("synthetic-http-token"),
        );
        let cancellation = CancellationToken::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut session = Session::start_http(
            &definition,
            headers,
            Some(&fixture.endpoint),
            deadline,
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(session.tools().len(), 1);
        let result = session
            .call("read_evidence", json!({}), deadline, &cancellation)
            .await
            .unwrap();
        assert_eq!(result.structured.unwrap()["text"], "fresh HTTP evidence");
        assert!(!session.remote_completion_uncertain());
        session.close().await.unwrap();
        session.close().await.unwrap();
        let records: Vec<Value> = std::fs::read_to_string(fixture.cwd.join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            records.len(),
            5,
            "initialize, initialized, list, call, DELETE only"
        );
        assert_eq!(records[0]["rpc"], "initialize");
        assert_eq!(records[1]["rpc"], "notifications/initialized");
        assert_eq!(records[2]["rpc"], "tools/list");
        assert_eq!(records[3]["rpc"], "tools/call");
        assert_eq!(records[4]["method"], "DELETE");
        for record in &records[1..] {
            assert_eq!(record["protocol"], "2025-11-25");
            assert_eq!(record["session"], records[1]["session"]);
            assert!(!record["session"].as_str().unwrap().is_empty());
        }
        assert_ne!(records[0]["id"], records[2]["id"]);
        assert_ne!(records[2]["id"], records[3]["id"]);
    }
}
