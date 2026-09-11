use super::*;
use pablo_core::{
    FinishReason, Message, Provider, RunError, RunSpec, Runtime, ToolRegistry, Usage,
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Parent {
    calls: AtomicUsize,
    child_entered: Notify,
    child_id: Mutex<Option<String>>,
}
impl Provider for Parent {
    fn name(&self) -> &'static str {
        "parent-fixture"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> futures::future::BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            assert!(request.tools.iter().any(|tool| tool.name == "subagent"));
            assert!(
                !serde_json::to_string(request.messages)
                    .unwrap()
                    .contains("working"),
                "child transcript must not enter root history"
            );
            let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![ProviderEvent::ToolCallStart{id:"spawn".into(),name:"subagent".into()},
                    ProviderEvent::ToolCallArgumentsDelta{id:"spawn".into(),delta:json!({"action":"spawn","request":{"input":"selected child task","capabilities":{"tools":[],"model_route":["secondary"]}}}).to_string()},
                    ProviderEvent::Finished{reason:FinishReason::ToolCalls,usage:Usage::default()}]
            } else {
                let result = request
                    .messages
                    .iter()
                    .find_map(|message| match message {
                        Message::Tool { name, result, .. } if name == "subagent" => {
                            result.subagent.as_deref()
                        }
                        _ => None,
                    })
                    .unwrap();
                *self.child_id.lock().unwrap() =
                    Some(result["agent"]["agent_id"].as_str().unwrap().into());
                self.child_entered.notified().await;
                vec![
                    ProviderEvent::TextDelta("root finished".into()),
                    ProviderEvent::Finished {
                        reason: FinishReason::Stop,
                        usage: Usage::default(),
                    },
                ]
            };
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}
#[tokio::test]
async fn native_root_completion_joins_model_spawned_child_and_rolls_up_accounting() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture=Fixture::new(&format!("http://{}",listener.local_addr().unwrap()));
        let sdk=opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let runtime=Runtime::new(pablo_core::telemetry::tracer(&sdk)).with_root_owner(fixture.supervisor.clone()).unwrap();
        let provider=Parent{calls:AtomicUsize::new(0),child_entered:Notify::new(),child_id:Mutex::new(None)};
        let mut events: Vec<RunEvent>=vec![];
        let sink=|event:&RunEvent| {
                if matches!(event.kind,EventKind::RunFinished{..}) && event.agent.as_ref().unwrap().depth() == 0 {
                    let id=provider.child_id.lock().unwrap().clone().unwrap();
                    assert_eq!(fixture.supervisor.inspect(&id).unwrap().agent.state(),AgentState::Settled);
                    assert_eq!(fixture.ledger.resources().active_children,0);
                    let child_trace=fixture.supervisor.inspect(&id).unwrap().trace.unwrap();
                    let spawn=events.iter().find(|event| matches!(event.kind,EventKind::ToolStarted{ref call} if call.name=="subagent")).unwrap();
                    assert_eq!(child_trace.trace_id,event.trace_id);
                    assert_eq!(child_trace.parent_span_id.as_deref(),Some(spawn.span_id.as_str()));
                    assert_eq!(event.accounting.as_ref().unwrap().model_calls,3);
                    assert_eq!(event.accounting.as_ref().unwrap().tool_calls,1);
                    assert_eq!(event.run_id,fixture.supervisor.inner.root.root_run_id());
                    assert_eq!(event.session_id,fixture.supervisor.inner.root.root_session_id());
                }
                events.push(event.clone());Ok(())
            };
        let mut sink=pablo_core::events::tree::TreeSink::new(sink,&fixture.ledger,fixture.supervisor.inner.root.clone()).unwrap();
        let consumer=Supervisor::consume_updates(fixture.updates.clone(),sink.clone(),fixture.root_cancel.clone());
        let root=runtime.run_with_tools(fixture.supervisor.inner.parent.spec(),&provider,
            &fixture.supervisor.inner.parent_tools,&fixture.root_cancel,&mut sink);
        let child=async {
            let (mut stream,_)=listener.accept().await.unwrap();
            let body=request(&mut stream).await;
            assert!(body.to_string().contains("selected child task"));
            assert!(!body.to_string().contains("parent private transcript"));
            held(&mut stream).await;provider.child_entered.notify_one();
            assert_closed(&mut stream).await;
        };
        let (outcome,(),drained)=tokio::join!(root,child,consumer);
        drained.unwrap();
        drop(sink);
        assert!(outcome.unwrap().is_completed());
        assert!(!fixture.root_cancel.is_cancelled(),"normal root completion must stay successful");
        let id=provider.child_id.lock().unwrap().clone().unwrap();
        assert_eq!(fixture.supervisor.inspect(&id).unwrap().outcome,Some(RunOutcome::Cancelled));
        let child_events:Vec<_>=events.iter().filter(|event|event.agent.as_ref().unwrap().agent_id()==id).collect();
        assert!(matches!(child_events.first().unwrap().kind,EventKind::RunStarted));
        assert!(matches!(child_events.last().unwrap().kind,EventKind::RunFinished{outcome:RunOutcome::Cancelled}));
        assert!(matches!(child_events[child_events.len()-2].kind,EventKind::ModelFinished{..}));
        for (i,event) in child_events.iter().enumerate(){assert_eq!(event.seq,i as u64+1);}
        assert_eq!(child_events.iter().filter(|e|matches!(e.kind,EventKind::RunFinished{..})).count(),1);
        assert_eq!(events.last().unwrap().agent.as_ref().unwrap().depth(),0);
        for (i,event) in events.iter().enumerate(){assert_eq!(event.root_seq,Some(i as u64+1));}
        assert_eq!(fixture.ledger.event_counts().used,events.len() as u64);
        assert!(fixture.supervisor.inspect(&id).unwrap().error.is_none());
        assert_eq!(fixture.ledger.total().model_calls,3);
        assert_eq!(fixture.ledger.agent(fixture.supervisor.inner.root.agent_id()).unwrap().model_calls,2);
        assert_eq!(events.iter().filter(|event|matches!(event.kind,EventKind::ToolFinished{ref name,ref result,..} if name=="subagent"&&result.status==pablo_core::tool::ToolStatus::Completed)).count(),1);
        assert!(matches!(runtime.run_with_tools(fixture.supervisor.inner.parent.spec(),&provider,
            &fixture.supervisor.inner.parent_tools,&fixture.root_cancel,&mut |_:&RunEvent|Ok(())).await,Err(RunError::InvalidSpec("child root is owned by one run"))));
        sdk.shutdown().unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn invalid_root_preflight_still_joins_already_admitted_host_children() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let child = fixture.spawn("host child before root preflight");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let runtime = Runtime::new(pablo_core::telemetry::tracer(&sdk))
            .with_root_owner(fixture.supervisor.clone())
            .unwrap();
        let mut invalid = RunSpec::new("invalid", fixture.cwd.clone(), "");
        invalid.session_id = Some("root-session".into());
        assert!(
            runtime
                .run_with_tools(
                    &invalid,
                    &pablo_core::ScriptedProvider::text(["must not run"]),
                    &ToolRegistry::default(),
                    &fixture.root_cancel,
                    &mut |_: &RunEvent| Ok(())
                )
                .await
                .is_err()
        );
        let snapshot = fixture.supervisor.inspect(child.agent_id()).unwrap();
        assert_eq!(snapshot.agent.state(), AgentState::Settled);
        assert_eq!(snapshot.outcome, Some(RunOutcome::Cancelled));
        assert_eq!(fixture.ledger.resources().active_children, 0);
        assert_closed(&mut stream).await;
        sdk.shutdown().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn owner_cancellation_reaches_convenience_root_run_and_joins_host_child() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture = Fixture::new(&format!("http://{}", listener.local_addr().unwrap()));
        let child = fixture.spawn("owned host child");
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        held(&mut stream).await;
        let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let runtime = Runtime::new(pablo_core::telemetry::tracer(&sdk))
            .with_root_owner(fixture.supervisor.clone())
            .unwrap();
        let provider = pablo_core::ScriptedProvider::new(vec![(
            Duration::from_secs(30),
            Ok(ProviderEvent::TextDelta("too late".into())),
        )]);
        let entered = Notify::new();
        let mut terminal = false;
        let mut sink = |event: &RunEvent| {
            if matches!(event.kind, EventKind::ModelStarted { .. }) {
                entered.notify_one();
            }
            if let EventKind::RunFinished { outcome } = &event.kind {
                assert_eq!(*outcome, RunOutcome::Cancelled);
                assert_eq!(
                    fixture
                        .supervisor
                        .inspect(child.agent_id())
                        .unwrap()
                        .agent
                        .state(),
                    AgentState::Settled
                );
                assert_eq!(fixture.ledger.resources().active_children, 0);
                terminal = true;
            }
            Ok(())
        };
        let running = runtime.run(fixture.supervisor.inner.parent.spec(), &provider, &mut sink);
        let stop = async {
            entered.notified().await;
            fixture.root_cancel.cancel();
        };
        let (outcome, ()) = tokio::join!(running, stop);
        assert_eq!(outcome.unwrap(), RunOutcome::Cancelled);
        assert!(terminal);
        assert_closed(&mut stream).await;
        sdk.shutdown().unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn ordinary_root_tool_policy_denies_subagent_without_child_admission() {
    for authority in [false, true] {
        let fixture = Fixture::with_settings(
            "http://127.0.0.1:1",
            900_000,
            if authority {
                "\n[[authority]]\nid=\"host\"\ntool_names=[\"fs.read\",\"fs.list\",\"fs.search\"]\n"
            } else {
                ""
            },
        );
        let sdk = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let runtime = Runtime::new(pablo_core::telemetry::tracer(&sdk))
            .with_root_owner(fixture.supervisor.clone())
            .unwrap();
        let tools = if authority {
            ToolRegistry::with_filesystem_reads().unwrap()
        } else {
            ToolRegistry::configured(
                false,
                false,
                pablo_core::policy::Policy {
                    tools: Some(pablo_core::policy::Rules {
                        default: pablo_core::policy::DefaultDecision::Deny,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let provider = pablo_core::ScriptedProvider::new(vec![
            (
                Duration::ZERO,
                Ok(ProviderEvent::ToolCallStart {
                    id: "denied".into(),
                    name: "subagent".into(),
                }),
            ),
            (
                Duration::ZERO,
                Ok(ProviderEvent::ToolCallArgumentsDelta {
                    id: "denied".into(),
                    delta: json!({"action":"spawn","request":{"input":"forbidden"}}).to_string(),
                }),
            ),
            (
                Duration::ZERO,
                Ok(ProviderEvent::Finished {
                    reason: FinishReason::ToolCalls,
                    usage: Usage::default(),
                }),
            ),
        ]);
        let outcome = runtime
            .run_with_tools(
                fixture.supervisor.inner.parent.spec(),
                &provider,
                &tools,
                &fixture.root_cancel,
                &mut |_: &RunEvent| Ok(()),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, RunOutcome::PolicyDenied { .. }));
        assert!(
            fixture
                .supervisor
                .inner
                .registry
                .lock()
                .unwrap()
                .entries
                .is_empty()
        );
        assert_eq!(fixture.ledger.total().tool_calls, 0);
        assert_eq!(fixture.ledger.total().model_calls, 1);
        sdk.shutdown().unwrap();
    }
}

#[tokio::test]
async fn subagent_actions_project_owned_state_and_reject_foreign_or_injected_authority() {
    use pablo_core::tool::{Tool, ToolContext, ToolStatus};
    async fn action(fixture: &Fixture, args: Value) -> pablo_core::tool::ToolResult {
        fixture
            .supervisor
            .execute(
                args,
                ToolContext {
                    policy_decisions: &[],
                    workspace: &fixture.cwd,
                    filesystem: None,
                    deadline: fixture.ledger.deadline(),
                    limits: &fixture.supervisor.inner.parent.spec().limits,
                    cancellation: &fixture.root_cancel,
                    context: opentelemetry::Context::new(),
                    on_started: &mut |_| true,
                },
            )
            .await
    }
    let fixture = Fixture::new("http://127.0.0.1:1");
    let spawned=action(&fixture,json!({"action":"spawn","request":{"input":"queued task","capabilities":{"tools":[],"model_route":["secondary"]}}})).await;
    assert_eq!(spawned.status, ToolStatus::Completed);
    let id = spawned.subagent.as_ref().unwrap()["agent"]["agent_id"]
        .as_str()
        .unwrap();
    let inspect = action(&fixture, json!({"action":"inspect","agent_id":id})).await;
    assert_eq!(
        inspect.subagent.unwrap()["snapshot"]["agent"]["state"],
        "queued"
    );
    let wait = action(
        &fixture,
        json!({"action":"wait","agent_ids":[id],"mode":"all","timeout_ms":0}),
    )
    .await;
    assert_eq!(wait.subagent.unwrap()["remaining"][0]["agent_id"], id);
    let stop = action(&fixture, json!({"action":"stop","agent_id":id})).await;
    assert_eq!(
        stop.subagent.unwrap()["snapshot"]["outcome"]["status"],
        "cancelled"
    );
    assert_eq!(fixture.ledger.total().model_calls, 0);
    let foreign = action(
        &fixture,
        json!({"action":"inspect","agent_id":uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(foreign.status, ToolStatus::RecoverableError);
    let injected = action(
        &fixture,
        json!({"action":"spawn","request":{"input":"injected","parent_agent_id":id}}),
    )
    .await;
    assert_eq!(injected.status, ToolStatus::InvalidArguments);
    assert_eq!(
        fixture
            .supervisor
            .inner
            .registry
            .lock()
            .unwrap()
            .entries
            .len(),
        1
    );
    fixture.supervisor.close().await.unwrap();
}

pub(super) async fn assert_closed(stream: &mut tokio::net::TcpStream) {
    let mut byte = [0; 1];
    match stream.read(&mut byte).await {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        result => panic!("joined provider connection is still open: {result:?}"),
    }
}
