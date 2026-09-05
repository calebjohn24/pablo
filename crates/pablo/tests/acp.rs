//! Interoperate through Unix pipes with the official Rust v1 client as well as
//! the TypeScript executable fixtures (which use Node's socket-backed stdio).
#![cfg(unix)]
use agent_client_protocol::{
    ByteStreams, Client,
    schema::{
        ProtocolVersion,
        v1::{InitializeRequest, NewSessionRequest},
    },
};
use std::{
    fs::File,
    os::fd::OwnedFd,
    process::{Command, Stdio},
    time::Duration,
};

#[tokio::test(flavor = "current_thread")]
async fn official_rust_client_initializes_the_actual_executable_over_pipes() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pablo"))
        .args(["acp", "--stdio", "--no-shell"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let input =
        async_io::Async::new(File::from(OwnedFd::from(child.stdout.take().unwrap()))).unwrap();
    let output =
        async_io::Async::new(File::from(OwnedFd::from(child.stdin.take().unwrap()))).unwrap();
    let connection = Client
        .builder()
        .connect_with(ByteStreams::new(output, input), async |cx| {
            let initialized = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
            assert!(!initialized.agent_capabilities.load_session);
            let session = cx
                .send_request(NewSessionRequest::new(std::env::current_dir().unwrap()))
                .block_task()
                .await?;
            assert!(!session.session_id.to_string().is_empty());
            Ok(())
        });
    let result = tokio::time::timeout(Duration::from_secs(3), connection).await;
    // Even a failed handshake must not leave a test subprocess behind.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let status = child.wait().unwrap();
    result
        .expect("ACP handshake timeout")
        .expect("ACP handshake failed");
    assert!(status.success());
}
