use super::*;
use futures::FutureExt;
#[tokio::test]
#[ignore = "requires isolated pinned A2A Python SDK"]
async fn supervised_sdk_remote_results_events_usage_and_cancellation() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let peer_cwd = std::env::temp_dir().join(format!(
        "pablo-supervisor-a2a-peer-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&peer_cwd).unwrap();
    let mut peer = tokio::process::Command::new(root.join(".pablo/a2a-fixture-venv/bin/python"))
        .arg(root.join("tests/fixtures/a2a/wire_server.py"))
        .current_dir(&peer_cwd)
        .env_clear()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let result=tokio::time::timeout(Duration::from_secs(15),std::panic::AssertUnwindSafe(async {
        let mut lines=BufReader::new(peer.stdout.take().unwrap()).lines();let ready:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();let url=ready["url"].as_str().unwrap();
        let extra="\n[options.a2a.remotes.peer]\ncard_url='https://agent.example.test/.well-known/agent-card.json'\nendpoint='https://agent.example.test/rpc'\n";
        let (cwd,mut options,parent)=Fixture::admission(url,10000,extra);options.deployment.as_mut().unwrap().fixture_a2a_endpoints.insert("peer".into(),url.into());
        let tools=Arc::new(parent.tools().unwrap());let root=AgentRef::root(uuid::Uuid::new_v4().to_string(),"root-session".into());let ledger=RootLedger::new(&root,parent.spec().limits.clone()).unwrap();let root_cancel=CancellationToken::new();let (supervisor,updates)=Supervisor::new(options,root,ledger.clone(),&root_cancel,parent,tools).unwrap();let fixture=Fixture{cwd,supervisor:Arc::new(supervisor),updates,root_cancel,ledger};
        let delivered=Arc::new(Mutex::new(Vec::new()));let sink=pablo_core::events::tree::TreeSink::new({let delivered=delivered.clone();move|event:&RunEvent|{delivered.lock().unwrap().push(event.clone());Ok(())}},&fixture.ledger,fixture.supervisor.inner.root.clone()).unwrap();
        let consume=tokio::spawn(Supervisor::consume_updates(fixture.updates.clone(),sink,fixture.root_cancel.clone()));
        for (input,stream) in [("immediate",false),("task",false),("assembly",true)] {
            let request=serde_json::from_value(json!({"remote":"peer","parts":[{"text":input}],"accepted_output_modes":["text/plain","application/octet-stream"],"stream":stream})).unwrap();
            let agent=fixture.supervisor.spawn_remote(&request,opentelemetry::Context::new()).unwrap();assert_eq!(agent.kind(),pablo_core::children::AgentKind::RemoteA2a);
            let result=fixture.supervisor.wait(&[agent.agent_id().into()],WaitMode::All,5000).await.unwrap();assert!(result.remaining.is_empty());let snapshot=&result.settled[0];assert!(snapshot.outcome.as_ref().unwrap().is_completed(),"{snapshot:?}");
            let remote=snapshot.remote.as_ref().unwrap();assert_eq!(remote.remote.context_id.as_deref(),Some("remote-context"));assert_eq!(remote.protocol_version,"1.0");assert_eq!(remote.binding,"JSONRPC");assert_eq!(remote.endpoint.as_deref(),Some("https://agent.example.test/rpc"));assert!(remote.card_sha256.is_some());assert_eq!(snapshot.accounting.model_calls,0);assert_eq!(snapshot.accounting.tool_calls,0);
            if input=="assembly" {let result=remote.result.as_ref().unwrap();assert_eq!(result.artifacts[0].parts.len(),3);assert_eq!(result.remote_reported_usage.unwrap().total_tokens,Some(20));assert_eq!(remote.remote_reported_usage.unwrap().cost_microusd,Some(42));}
            let events=delivered.lock().unwrap();let own=events.iter().filter(|event|event.agent.as_ref().unwrap().agent_id()==agent.agent_id()).collect::<Vec<_>>();assert!(matches!(own.first().unwrap().kind,EventKind::RunStarted));assert!(matches!(own.last().unwrap().kind,EventKind::RunFinished{..}));assert!(own.iter().any(|e|matches!(e.kind,EventKind::A2aUpdate{..})));assert!(own.iter().all(|e|fixture.ledger.event_source_matches(e)));assert_ne!(own[0].session_id,"remote-context");
        }
        let request=serde_json::from_value(json!({"remote":"peer","parts":[{"text":"hold"}],"accepted_output_modes":["text/plain"],"stream":true})).unwrap();let agent=fixture.supervisor.spawn_remote(&request,opentelemetry::Context::new()).unwrap();
        loop {if fixture.supervisor.inspect(agent.agent_id()).unwrap().remote.unwrap().remote.task_id.is_some(){break}tokio::task::yield_now().await;}
        let stopped=fixture.supervisor.stop(agent.agent_id()).await.unwrap();assert_eq!(stopped.outcome,Some(RunOutcome::Cancelled));assert_eq!(stopped.remote.unwrap().cancellation,pablo_core::a2a::transport::CancelReceipt::Received(pablo_core::a2a::wire::TaskState::Canceled));
        let mut active=Vec::new();
        for _ in 0..2 {active.push(fixture.supervisor.spawn_remote(&request,opentelemetry::Context::new()).unwrap());}
        loop {if active.iter().all(|agent|fixture.supervisor.inspect(agent.agent_id()).unwrap().remote.unwrap().remote.task_id.is_some()){break}tokio::task::yield_now().await;}
        let before=std::fs::read_to_string(peer_cwd.join("calls.jsonl")).unwrap();
        let queued=fixture.supervisor.spawn_remote(&request,opentelemetry::Context::new()).unwrap();let local=fixture.spawn("queued local task");
        assert_eq!(fixture.ledger.resources().active_children,2);assert_eq!(fixture.ledger.resources().pending_children,2);
        let queued=fixture.supervisor.stop(queued.agent_id()).await.unwrap();assert_eq!(queued.outcome,Some(RunOutcome::Cancelled));assert_eq!(queued.remote.as_ref().unwrap().delivery,pablo_core::DeliveryCertainty::NotSent);assert!(queued.remote.as_ref().unwrap().card_sha256.is_none());
        fixture.supervisor.stop(local.agent_id()).await.unwrap();assert_eq!(std::fs::read_to_string(peer_cwd.join("calls.jsonl")).unwrap(),before);
        for agent in active {assert_eq!(fixture.supervisor.stop(agent.agent_id()).await.unwrap().outcome,Some(RunOutcome::Cancelled));}
        fixture.supervisor.close().await.unwrap();consume.await.unwrap().unwrap();assert_eq!(fixture.ledger.resources().active_children,0);assert_eq!(fixture.ledger.resources().pending_children,0);
        let (cwd,mut options,parent)=Fixture::admission(url,10000,&format!("\nmax_output_bytes=4\n{extra}"));options.deployment.as_mut().unwrap().fixture_a2a_endpoints.insert("peer".into(),url.into());
        let tools=Arc::new(parent.tools().unwrap());let root=AgentRef::root(uuid::Uuid::new_v4().to_string(),"root-session".into());let ledger=RootLedger::new(&root,parent.spec().limits.clone()).unwrap();let root_cancel=CancellationToken::new();let (supervisor,updates)=Supervisor::new(options,root,ledger.clone(),&root_cancel,parent,tools).unwrap();let limited=Fixture{cwd,supervisor:Arc::new(supervisor),updates,root_cancel,ledger};
        let receiver=limited.updates.clone();let consume_limited=tokio::spawn(async move{while let Ok(update)=receiver.recv().await{let _=update.update.consumed.send(());}});
        let selected=serde_json::from_value(json!({"remote":"peer","parts":[{"text":"assembly"}],"accepted_output_modes":["text/plain","application/octet-stream"],"stream":true})).unwrap();let agent=limited.supervisor.spawn_remote(&selected,opentelemetry::Context::new()).unwrap();let result=limited.supervisor.wait(&[agent.agent_id().into()],WaitMode::All,5000).await.unwrap();assert_eq!(result.settled[0].outcome,Some(RunOutcome::LimitExceeded{limit:LimitKind::OutputBytes}));assert!(result.settled[0].remote.as_ref().unwrap().result.is_none());
        limited.supervisor.close().await.unwrap();consume_limited.await.unwrap();
        let text=serde_json::to_string(&*delivered.lock().unwrap()).unwrap();assert!(!text.contains("parent private transcript"));assert!(!text.contains("inert.bin"));
    }).catch_unwind()).await;
    peer.kill().await.unwrap();
    peer.wait().await.unwrap();
    std::fs::remove_dir_all(peer_cwd).unwrap();
    result.unwrap().unwrap();
}
