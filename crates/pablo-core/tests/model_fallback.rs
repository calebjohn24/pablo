use futures_util::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{
    deployment::{self, ConfigInput, ResolveRequest},
    provider::{
        AccountingBounds, ModelRequest, ProviderError, ProviderEvent, ProviderRoute,
        ProviderStream, RetryClass,
    },
    *,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

type Observed = Arc<Mutex<Vec<(String, Vec<Message>, u32)>>>;
type Turn = Result<Vec<Result<ProviderEvent, ProviderError>>, ProviderError>;
struct Fixture {
    name: &'static str,
    turns: Mutex<VecDeque<Turn>>,
    seen: Observed,
    delay: Duration,
    bounds: AccountingBounds,
}
impl Provider for Fixture {
    fn name(&self) -> &'static str {
        self.name
    }
    fn accounting_bounds(&self, _: &str, _: u32) -> AccountingBounds {
        self.bounds
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            self.seen.lock().unwrap().push((
                request.model.into(),
                request.messages.to_vec(),
                request.max_output_tokens,
            ));
            tokio::time::sleep(self.delay).await;
            let events = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected attempt")?;
            Ok(Box::pin(stream::iter(events)) as ProviderStream<'a>)
        })
    }
}
fn error(delivery: DeliveryCertainty, class: Option<RetryClass>) -> ProviderError {
    ProviderError {
        code: if class.is_some() {
            FailureCode::ProviderRejected
        } else {
            FailureCode::ProviderTransport
        },
        delivery,
        retry_class: class,
    }
}
fn turn(tool: bool) -> Turn {
    let mut events = if tool {
        vec![
            ProviderEvent::ToolCallStart {
                id: "read-once".into(),
                name: "fs.read".into(),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: "read-once".into(),
                delta: r#"{"path":"absent"}"#.into(),
            },
        ]
    } else {
        vec![ProviderEvent::TextDelta("done".into())]
    };
    events.push(ProviderEvent::Cost { microusd: 2 });
    events.push(ProviderEvent::Finished {
        reason: if tool {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        },
        usage: Usage {
            input_tokens: Some(3),
            output_tokens: Some(2),
            cache_read_input_tokens: Some(0),
            cache_write_input_tokens: Some(0),
        },
    });
    Ok(events.into_iter().map(Ok).collect())
}
struct Setup {
    route: ProviderRoute,
    spec: RunSpec,
    seen: Vec<Observed>,
}
fn setup(turns: [Vec<Turn>; 3], policy: Value, delay: Duration, unattested_last: bool) -> Setup {
    let workspace = std::env::temp_dir().canonicalize().unwrap();
    let mut config = json!({"schema_version":1,"options":{"shell":{"enabled":false},"models":{
        "a":{"provider":"vercel","id":"zai/glm-5.3-flash","credential":"a","model_options":{"max_output_tokens":64}},
        "b":{"provider":"openrouter","id":"z-ai/glm-5.3-flash","credential":"b","model_options":{"max_output_tokens":128}},
        "c":{"provider":"vercel","id":"zai/glm-5.3-flash","credential":"c"}},
        "routes":{"main":{"entries":[{"model":"a"},{"model":"b"},{"model":"c"}]}},"model_route":"main"},
        "credentials":{"a":{"consumer":"provider.vercel","sources":[{"kind":"environment","name":"A"}]},"b":{"consumer":"provider.openrouter","sources":[{"kind":"environment","name":"B"}]},"c":{"consumer":"provider.vercel","sources":[{"kind":"environment","name":"C"}]}}});
    for (key, value) in policy.as_object().unwrap() {
        config["options"]["routes"]["main"][key] = value.clone();
    }
    let mut request = ResolveRequest::new(workspace.clone(), "unused.toml");
    request.entry = ConfigInput::Document(config);
    request
        .path_bindings
        .insert("workspace".into(), workspace.clone());
    let deployment = deployment::resolve(request).unwrap();
    let mut seen = Vec::new();
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    for (i, turns) in turns.into_iter().enumerate() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        seen.push(calls.clone());
        providers.push(Box::new(Fixture {
            name: if i == 1 { "openrouter" } else { "vercel" },
            turns: Mutex::new(turns.into()),
            seen: calls,
            delay: if i == 0 { delay } else { Duration::ZERO },
            bounds: if i == 2 && unattested_last {
                AccountingBounds::default()
            } else {
                AccountingBounds {
                    tokens: Some(10),
                    cost_microusd: Some(8),
                }
            },
        }));
    }
    let route = ProviderRoute::new(deployment.model_route().unwrap().clone(), providers).unwrap();
    let mut spec = RunSpec::new("read", workspace, "zai/glm-5.3-flash");
    spec.limits.max_model_calls = Some(5);
    spec.limits.max_tool_calls = Some(1);
    spec.limits.max_total_tokens = Some(50);
    spec.limits.max_cost_microusd = Some(40);
    Setup { route, spec, seen }
}
async fn run(
    setup: &Setup,
    cancel: bool,
) -> Result<(RunOutcome, Vec<RunEvent>), runtime::RunError> {
    let sdk = SdkTracerProvider::builder().build();
    let mut events = Vec::new();
    let token = CancellationToken::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run_with_tools(
            &setup.spec,
            &setup.route,
            &ToolRegistry::with_filesystem_reads().unwrap(),
            &token,
            &mut |event: &RunEvent| {
                if cancel && matches!(event.kind, EventKind::ModelFinished { .. }) {
                    token.cancel();
                }
                events.push(event.clone());
                Ok(())
            },
        )
        .await?;
    Ok((outcome, events))
}
fn accounting(events: &[RunEvent]) -> Accounting {
    *TaskResult::from_terminal(events.last().unwrap())
        .unwrap()
        .accounting
        .unwrap()
}
#[tokio::test]
async fn eligible_failure_switches_once_and_sticks_across_real_tool_history() {
    for later in [false, true] {
        let fail = Err(error(DeliveryCertainty::NotSent, None));
        let s = if later {
            setup(
                [vec![turn(true), fail], vec![turn(false)], vec![]],
                json!({}),
                Duration::ZERO,
                false,
            )
        } else {
            setup(
                [vec![fail], vec![turn(true), turn(false)], vec![]],
                json!({}),
                Duration::ZERO,
                false,
            )
        };
        let (outcome, events) = run(&s, false).await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));
        assert_eq!(s.seen[2].lock().unwrap().len(), 0);
        let second = s.seen[1].lock().unwrap();
        assert_eq!(second.len(), if later { 1 } else { 2 });
        assert_eq!(second.last().unwrap().1.len(), 3);
        assert_eq!(second.last().unwrap().2, 128);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::ToolStarted { .. }))
                .count(),
            1
        );
        let starts: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::ModelStarted { .. }))
            .collect();
        assert_eq!(starts.len(), 3);
        let spans: std::collections::HashSet<_> = starts.iter().map(|e| &e.span_id).collect();
        assert_eq!(spans.len(), 3);
        assert!(
            starts
                .iter()
                .all(|e| e.run_id == starts[0].run_id && e.trace_id == starts[0].trace_id)
        );
        assert!(
            matches!(&starts[0].kind,EventKind::ModelStarted{provider,model} if provider=="vercel"&&model=="zai/glm-5.3-flash")
        );
        assert!(
            matches!(&starts[2].kind,EventKind::ModelStarted{provider,model} if provider=="openrouter"&&model=="z-ai/glm-5.3-flash")
        );
        let a = accounting(&events);
        assert_eq!(a.model_calls, 3);
        assert_eq!(a.tool_calls, 1);
        assert_eq!(a.charged_tokens, Some(10));
        assert_eq!(a.charged_cost_microusd, Some(4));
    }
}
#[tokio::test]
async fn received_transient_and_opted_in_uncertain_attempts_retain_shared_reservations() {
    for (failure, policy) in [
        (
            error(
                DeliveryCertainty::ResponseReceived,
                Some(RetryClass::RateLimited),
            ),
            json!({}),
        ),
        (
            error(
                DeliveryCertainty::ResponseReceived,
                Some(RetryClass::ServiceUnavailable),
            ),
            json!({}),
        ),
        (
            error(DeliveryCertainty::MayHaveBeenSent, None),
            json!({"eligible_errors":["transport_uncertain"],"retry_uncertain_delivery":true}),
        ),
    ] {
        let s = setup(
            [vec![Err(failure)], vec![turn(false)], vec![]],
            policy,
            Duration::ZERO,
            false,
        );
        let (outcome, events) = run(&s, false).await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));
        let a = accounting(&events);
        assert_eq!(a.model_calls, 2);
        assert_eq!(a.charged_tokens, Some(15));
        assert_eq!(a.charged_cost_microusd, Some(10));
        assert_eq!(a.usage.input_tokens, None);
        assert_eq!(a.cost_microusd, None);
    }
}
#[tokio::test]
async fn default_uncertainty_partial_output_cancellation_and_attempt_budget_stop() {
    let cases = [
        (
            vec![Err(error(DeliveryCertainty::MayHaveBeenSent, None))],
            json!({}),
            false,
        ),
        (
            vec![Ok(vec![
                Ok(ProviderEvent::TextDelta("escaped".into())),
                Err(error(
                    DeliveryCertainty::ResponseReceived,
                    Some(RetryClass::ServiceUnavailable),
                )),
            ])],
            json!({}),
            false,
        ),
        (
            vec![Err(error(DeliveryCertainty::NotSent, None))],
            json!({}),
            true,
        ),
        (
            vec![Err(error(DeliveryCertainty::NotSent, None))],
            json!({"max_attempts":1}),
            false,
        ),
    ];
    for (turns, policy, cancel) in cases {
        let s = setup([turns, vec![], vec![]], policy, Duration::ZERO, false);
        let (outcome, _) = run(&s, cancel).await.unwrap();
        assert!(!matches!(outcome, RunOutcome::Completed { .. }));
        assert!(s.seen[1].lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn distinct_attempt_deadline_is_bounded_and_opt_in_preserves_uncertain_charge() {
    for retry in [false, true] {
        let mut policy = json!({"per_attempt_timeout_ms":10});
        if retry {
            policy["eligible_errors"] = json!(["transport_uncertain"]);
            policy["retry_uncertain_delivery"] = true.into();
        }
        let s = setup(
            [vec![turn(false)], vec![turn(false)], vec![]],
            policy,
            Duration::from_secs(5),
            false,
        );
        let start = std::time::Instant::now();
        let (outcome, events) = run(&s, false).await.unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        if retry {
            assert!(matches!(outcome, RunOutcome::Completed { .. }));
            assert_eq!(accounting(&events).charged_tokens, Some(15));
        } else {
            assert!(matches!(
                outcome,
                RunOutcome::Failed {
                    code: FailureCode::ModelAttemptTimedOut,
                    ..
                }
            ));
            assert!(s.seen[1].lock().unwrap().is_empty());
        }
    }
}
#[tokio::test]
async fn all_entry_preflight_and_root_call_budget_prevent_dispatch() {
    let s = setup([vec![], vec![], vec![]], json!({}), Duration::ZERO, true);
    assert!(run(&s, false).await.is_err());
    assert!(s.seen.iter().all(|s| s.lock().unwrap().is_empty()));
    let mut s = setup(
        [
            vec![Err(error(DeliveryCertainty::NotSent, None))],
            vec![],
            vec![],
        ],
        json!({}),
        Duration::ZERO,
        false,
    );
    s.spec.limits.max_model_calls = Some(1);
    let (outcome, events) = run(&s, false).await.unwrap();
    assert!(matches!(
        outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::ModelCalls
        }
    ));
    assert_eq!(accounting(&events).model_calls, 1);
    assert!(s.seen[1].lock().unwrap().is_empty());
}

#[tokio::test]
async fn f03_exhaustion_has_exact_order_and_cumulative_settlement() {
    let s = setup(
        [
            vec![Err(error(DeliveryCertainty::NotSent, None))],
            vec![Err(error(
                DeliveryCertainty::ResponseReceived,
                Some(RetryClass::RateLimited),
            ))],
            vec![Err(error(
                DeliveryCertainty::ResponseReceived,
                Some(RetryClass::ServiceUnavailable),
            ))],
        ],
        json!({}),
        Duration::ZERO,
        false,
    );
    let (outcome, events) = run(&s, false).await.unwrap();
    assert!(matches!(
        outcome,
        RunOutcome::Failed {
            code: FailureCode::ProviderRejected,
            delivery: DeliveryCertainty::ResponseReceived
        }
    ));
    let records: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ModelFinished { .. }))
        .map(|e| e.model_route.as_ref().unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    for (i, record) in records.iter().enumerate() {
        assert_eq!(record.entry_index, i);
        assert_eq!(record.attempt, i + 1);
        assert_eq!(record.operation, "1");
        assert_eq!(
            record.selection_reason,
            if i == 0 { "initial" } else { "fallback" }
        );
        assert!(record.dispatched);
        assert_eq!(record.phase, "finished");
        let a = record.accounting.as_ref().unwrap();
        assert_eq!(a.model_calls, i as u64 + 1);
        assert_eq!(a.charged_tokens, Some(i as u64 * 10));
        assert_eq!(a.charged_cost_microusd, Some(i as u64 * 8));
    }
    assert_eq!(
        events.last().unwrap().model_route.as_ref().unwrap(),
        records[2]
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn f03_received_event_cannot_be_refunded_as_unsent_or_bypass_opt_in() {
    for retry in [false, true] {
        let s = setup(
            [
                vec![Ok(vec![
                    Ok(ProviderEvent::Cost { microusd: 1 }),
                    Err(error(DeliveryCertainty::NotSent, None)),
                ])],
                vec![turn(false)],
                vec![],
            ],
            if retry {
                json!({"eligible_errors":["not_sent","transport_uncertain"],"retry_uncertain_delivery":true})
            } else {
                json!({})
            },
            Duration::ZERO,
            false,
        );
        let (outcome, events) = run(&s, false).await.unwrap();
        assert_eq!(outcome.is_completed(), retry);
        assert_eq!(s.seen[1].lock().unwrap().len(), usize::from(retry));
        let first = events
            .iter()
            .find(|e| matches!(e.kind, EventKind::ModelFinished { .. }))
            .unwrap()
            .model_route
            .as_ref()
            .unwrap();
        assert_eq!(first.delivery, DeliveryCertainty::ResponseReceived);
        assert_eq!(first.retry_class.as_deref(), Some("transport_uncertain"));
        assert_eq!(first.accounting.as_ref().unwrap().charged_tokens, Some(10));
        // Known cost settles accurately; unknown tokens retain their reservation.
        assert_eq!(
            first.accounting.as_ref().unwrap().charged_cost_microusd,
            Some(1)
        );
        if !retry {
            assert!(matches!(
                outcome,
                RunOutcome::Failed {
                    delivery: DeliveryCertainty::ResponseReceived,
                    ..
                }
            ));
        }
    }
}

#[tokio::test]
async fn f03_remaining_root_allowances_stop_before_next_delivery() {
    for limit in [
        LimitKind::TotalTokens,
        LimitKind::Cost,
        LimitKind::ModelCalls,
        LimitKind::Events,
    ] {
        let mut s = setup(
            [
                vec![Err(error(
                    DeliveryCertainty::ResponseReceived,
                    Some(RetryClass::ServiceUnavailable),
                ))],
                vec![],
                vec![],
            ],
            json!({}),
            Duration::ZERO,
            false,
        );
        match limit {
            LimitKind::TotalTokens => s.spec.limits.max_total_tokens = Some(10),
            LimitKind::Cost => s.spec.limits.max_cost_microusd = Some(8),
            LimitKind::ModelCalls => s.spec.limits.max_model_calls = Some(1),
            LimitKind::Events => s.spec.limits.max_events = 4,
            _ => unreachable!(),
        }
        let (outcome, events) = run(&s, false).await.unwrap();
        assert_eq!(outcome, RunOutcome::LimitExceeded { limit });
        assert_eq!(accounting(&events).model_calls, 1);
        assert_eq!(accounting(&events).charged_tokens, Some(10));
        assert_eq!(accounting(&events).charged_cost_microusd, Some(8));
        assert!(s.seen[1].lock().unwrap().is_empty());
        let terminal = events.last().unwrap();
        let record = terminal.model_route.as_ref().unwrap();
        assert_eq!(record.entry, "b");
        assert!(!record.dispatched);
        assert_eq!(record.limit, Some(limit));
        assert!(matches!(record.phase.as_str(), "blocked" | "finished"));
    }
}

#[tokio::test]
async fn f03_root_deadline_and_inflight_cancellation_retain_uncertain_charge() {
    for cancel in [false, true] {
        let mut s = setup(
            [vec![turn(false)], vec![], vec![]],
            json!({"per_attempt_timeout_ms":1000,"eligible_errors":["transport_uncertain"],"retry_uncertain_delivery":true}),
            Duration::from_secs(5),
            false,
        );
        s.spec.limits.max_run_duration_ms = if cancel { 1000 } else { 10 };
        let token = CancellationToken::new();
        let trigger = token.clone();
        let guard = tokio::spawn(async move {
            if cancel {
                tokio::time::sleep(Duration::from_millis(10)).await;
                trigger.cancel();
            }
        });
        let provider = SdkTracerProvider::builder().build();
        let mut events = Vec::new();
        let outcome = Runtime::new(opentelemetry::trace::TracerProvider::tracer(
            &provider, "test",
        ))
        .run_with_tools(
            &s.spec,
            &s.route,
            &ToolRegistry::default(),
            &token,
            &mut |event: &RunEvent| {
                events.push(event.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
        guard.await.unwrap();
        assert_eq!(
            outcome,
            if cancel {
                RunOutcome::Cancelled
            } else {
                RunOutcome::TimedOut
            }
        );
        assert_eq!(accounting(&events).model_calls, 1);
        assert_eq!(accounting(&events).charged_tokens, Some(10));
        let record = events.last().unwrap().model_route.as_ref().unwrap();
        assert_eq!(record.delivery, DeliveryCertainty::MayHaveBeenSent);
        assert_eq!(record.status.as_deref(), Some(outcome.label()));
        assert!(s.seen[1].lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn f03_begun_tool_call_and_closing_sink_failure_never_fallback() {
    let s = setup(
        [
            vec![Ok(vec![
                Ok(ProviderEvent::ToolCallStart {
                    id: "unfinished".into(),
                    name: "fs.read".into(),
                }),
                Err(error(
                    DeliveryCertainty::ResponseReceived,
                    Some(RetryClass::ServiceUnavailable),
                )),
            ])],
            vec![],
            vec![],
        ],
        json!({}),
        Duration::ZERO,
        false,
    );
    let (_, events) = run(&s, false).await.unwrap();
    assert_eq!(accounting(&events).tool_calls, 0);
    assert!(s.seen[1].lock().unwrap().is_empty());
    let s = setup(
        [
            vec![Err(error(DeliveryCertainty::NotSent, None))],
            vec![],
            vec![],
        ],
        json!({}),
        Duration::ZERO,
        false,
    );
    let provider = SdkTracerProvider::builder().build();
    let mut events = Vec::new();
    let outcome = Runtime::new(opentelemetry::trace::TracerProvider::tracer(
        &provider, "test",
    ))
    .run(&s.spec, &s.route, &mut |e: &RunEvent| {
        if matches!(e.kind, EventKind::ModelFinished { .. }) {
            return Err(SinkError::Capacity);
        }
        events.push(e.clone());
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Failed {
            code: FailureCode::ProviderTransport,
            delivery: DeliveryCertainty::NotSent
        }
    );
    assert_eq!(accounting(&events).model_calls, 1);
    assert!(s.seen[1].lock().unwrap().is_empty());
}
