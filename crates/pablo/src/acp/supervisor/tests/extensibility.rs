//! Direct Rust Runtime embedding with the same independent E01 fixture as ACP.
use super::*;
use pablo_core::{EventSink, JsonlSink, Runtime, telemetry};
use std::io::BufWriter;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test]
#[ignore = "requires Node 24 and the isolated MCP Python SDK fixture"]
async fn e01_rust_embedding_combines_capabilities_handoffs_compaction_and_boundaries() {
    tokio::time::timeout(Duration::from_secs(45), async {
        let repository = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        for mode in ["complete", "cancel", "denied"] {
            let mut peer = tokio::process::Command::new("node")
                .arg("tests/fixtures/extensibility-host.ts")
                .arg(mode)
                .current_dir(&repository)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut input = peer.stdin.take().unwrap();
            let mut lines = BufReader::new(peer.stdout.take().unwrap()).lines();
            let ready: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let cwd = std::path::PathBuf::from(ready["cwd"].as_str().unwrap());
            let args = std::iter::once(std::ffi::OsString::from("--stdio")).chain(
                ready["args"].as_array().unwrap().iter().map(|v|std::ffi::OsString::from(v.as_str().unwrap()))
            );
            let options = Options::parse("acp".into(),args).unwrap();
            let prepared = options.prepare_run(
                Some("PRIVATE_E01_ROOT: combine evidence, preserve workspace scope.".into()),
                Some(cwd.clone()),Some(uuid::Uuid::new_v4().to_string()),
            ).unwrap().unwrap();
            let secrets = crate::deployment::Secrets::read(&prepared,options.deployment.as_ref().unwrap()).unwrap();
            let provider = secrets.provider(options.deployment.as_ref().unwrap()).unwrap();
            let factory = super::super::factory::RootFactory::new(Arc::new(options),prepared.clone()).unwrap();
            let ledger = factory.ledger.clone();
            let file = prepared.create_trace_file().unwrap().unwrap();
            let mut trace = JsonlSink::for_tree(BufWriter::new(file),prepared.spec(),factory.root.root_run_id().into()).unwrap();
            let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
            let runtime = Runtime::new(telemetry::tracer(&sdk));
            let cancel = CancellationToken::new();
            let mut events = Vec::new();
            let outcome = {
            let mut sink = |event:&RunEvent| { trace.emit(event)?; events.push(event.clone()); Ok(()) };
            let run = factory.run(runtime,provider.as_ref(),&cancel,&mut sink);
            tokio::pin!(run);
            if mode=="cancel" {
                tokio::select! {
                    result=&mut run => panic!("run settled before explicit cancellation: {result:?}"),
                    signal=lines.next_line() => assert_eq!(serde_json::from_str::<Value>(&signal.unwrap().unwrap()).unwrap(),json!({"cancel":true})),
                }
                cancel.cancel();
                run.await.unwrap().unwrap()
            } else { run.await.unwrap().unwrap() }
            };
            drop(trace);
            let terminal = events.last().unwrap();
            assert!(matches!(terminal.kind,EventKind::RunFinished{..}));
            assert_eq!(terminal.agent.as_ref().unwrap().depth(),0);
            assert_eq!(ledger.resources(),Resources::default());
            let compactions:Vec<_>=events.iter().filter(|e|matches!(e.kind,EventKind::CompactionFinished{..})).collect();
            assert_eq!(compactions.len(),1);
            assert_eq!(compactions[0].compaction.as_ref().unwrap().status,"completed");
            let task=json!({"outcome":outcome,"accounting":terminal.accounting});
            input.write_all(format!("{}\n",json!({"task":task})).as_bytes()).await.unwrap();
            input.shutdown().await.unwrap();
            let proof:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(proof["verified"],true);
            assert!(peer.wait().await.unwrap().success());
            sdk.shutdown().unwrap();
        }
    }).await.unwrap();
}
