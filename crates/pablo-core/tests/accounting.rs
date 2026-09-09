use futures_util::{future::BoxFuture, stream};
use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{
    provider::{AccountingBounds, ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    *,
};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct FixtureProvider {
    bounds: AccountingBounds,
    turns: Mutex<VecDeque<Result<Vec<ProviderEvent>, ProviderError>>>,
    calls: AtomicUsize,
}
impl FixtureProvider {
    fn new(turns: Vec<Result<Vec<ProviderEvent>, ProviderError>>) -> Self {
        Self {
            bounds: AccountingBounds {
                tokens: Some(10),
                cost_microusd: Some(8),
            },
            turns: Mutex::new(turns.into()),
            calls: AtomicUsize::new(0),
        }
    }
}
impl Provider for FixtureProvider {
    fn name(&self) -> &'static str {
        "attested-fixture"
    }
    fn accounting_bounds(&self, _: &str, _: u32) -> AccountingBounds {
        self.bounds
    }
    fn stream<'a>(
        &'a self,
        _: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let events = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected dispatch")?;
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))) as ProviderStream<'a>)
        })
    }
}
fn turn(tool: bool, usage: Usage, cost: Option<u64>) -> Result<Vec<ProviderEvent>, ProviderError> {
    let mut events = if tool {
        vec![
            ProviderEvent::ToolCallStart {
                id: "read-1".into(),
                name: "fs.read".into(),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                id: "read-1".into(),
                delta: r#"{"path":"absent"}"#.into(),
            },
        ]
    } else {
        vec![ProviderEvent::TextDelta("done".into())]
    };
    if let Some(microusd) = cost {
        events.push(ProviderEvent::Cost { microusd });
    }
    events.push(ProviderEvent::Finished {
        reason: if tool {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        },
        usage,
    });
    Ok(events)
}
fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_input_tokens: Some(2),
        cache_write_input_tokens: None,
    }
}
async fn run(
    p: &FixtureProvider,
    caps: (Option<u64>, Option<u64>),
    cancel: Option<&str>,
) -> (RunOutcome, Accounting) {
    let sdk = SdkTracerProvider::builder().build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let mut spec = RunSpec::new(
        "fixture",
        std::env::temp_dir().canonicalize().unwrap(),
        "fixture/accounting",
    );
    spec.limits.max_total_tokens = caps.0;
    spec.limits.max_cost_microusd = caps.1;
    let token = CancellationToken::new();
    let mut events = vec![];
    let outcome = runtime
        .run_with_tools(
            &spec,
            p,
            &ToolRegistry::with_filesystem_reads().unwrap(),
            &token,
            &mut |e: &RunEvent| {
                if (cancel == Some("before") && matches!(e.kind, EventKind::ModelStarted { .. }))
                    || (cancel == Some("after")
                        && matches!(e.kind, EventKind::ModelFinished { .. }))
                    || (cancel == Some("during") && matches!(e.kind, EventKind::TextDelta { .. }))
                {
                    token.cancel();
                }
                events.push(e.clone());
                if cancel == Some("sink_tool") && matches!(e.kind, EventKind::ToolStarted { .. }) {
                    return Err(SinkError::Io(std::io::ErrorKind::BrokenPipe.into()));
                }
                Ok(())
            },
        )
        .await
        .unwrap();
    let terminal = events.last().unwrap();
    let task = TaskResult::from_terminal(terminal).unwrap();
    assert_eq!(task.outcome.as_ref(), Some(&outcome));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
    (outcome, *task.accounting.unwrap())
}
#[tokio::test]
async fn reservations_settle_actuals_then_prevent_the_next_delivery() {
    for (caps, expected, charged) in [
        ((Some(15), None), LimitKind::TotalTokens, (Some(6), None)),
        ((None, Some(10)), LimitKind::Cost, (None, Some(3))),
        (
            (Some(15), Some(10)),
            LimitKind::TotalTokens,
            (Some(6), Some(3)),
        ),
    ] {
        let p = FixtureProvider::new(vec![turn(true, usage(3, 3), Some(3))]);
        let (outcome, a) = run(&p, caps, None).await;
        assert_eq!(outcome, RunOutcome::LimitExceeded { limit: expected });
        assert_eq!(p.calls.load(Ordering::SeqCst), 1);
        assert_eq!((a.model_calls, a.tool_calls), (1, 1));
        assert_eq!(a.usage, usage(3, 3));
        assert_eq!(a.cost_microusd, Some(3));
        assert_eq!((a.charged_tokens, a.charged_cost_microusd), charged);
    }
}
#[tokio::test]
async fn unknown_usage_retains_reservations_and_fresh_tasks_start_empty() {
    let p = FixtureProvider::new(vec![
        turn(true, Usage::default(), None),
        turn(false, usage(1, 1), Some(1)),
    ]);
    let (outcome, a) = run(&p, (Some(15), Some(10)), None).await;
    assert_eq!(
        outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::TotalTokens
        }
    );
    assert_eq!(
        (a.charged_tokens, a.charged_cost_microusd),
        (Some(10), Some(8))
    );
    assert_eq!(a.usage, Usage::default());
    assert_eq!(a.cost_microusd, None);
    let (outcome, a) = run(&p, (Some(15), Some(10)), None).await;
    assert!(outcome.is_completed());
    assert_eq!(a.model_calls, 1);
    assert_eq!(
        (a.charged_tokens, a.charged_cost_microusd),
        (Some(2), Some(1))
    );
}
#[tokio::test]
async fn uncertain_failures_retain_charges_but_not_sent_releases_them() {
    for delivery in [
        DeliveryCertainty::NotSent,
        DeliveryCertainty::MayHaveBeenSent,
        DeliveryCertainty::ResponseReceived,
    ] {
        let p = FixtureProvider::new(vec![Err(ProviderError {
            code: FailureCode::ProviderTransport,
            delivery,
        })]);
        let (outcome, a) = run(&p, (Some(10), Some(8)), None).await;
        assert!(matches!(outcome, RunOutcome::Failed { .. }));
        assert_eq!(a.model_calls, 1);
        if delivery == DeliveryCertainty::NotSent {
            assert_eq!(
                (a.charged_tokens, a.charged_cost_microusd),
                (Some(0), Some(0))
            );
            assert_eq!(a.usage.input_tokens, Some(0));
            assert_eq!(a.cost_microusd, Some(0));
        } else {
            assert_eq!(
                (a.charged_tokens, a.charged_cost_microusd),
                (Some(10), Some(8))
            );
            assert_eq!(a.usage.input_tokens, None);
            assert_eq!(a.cost_microusd, None);
        }
    }
}
#[tokio::test]
async fn unsupported_zero_and_violated_bounds_never_allow_extra_work() {
    let sdk = SdkTracerProvider::builder().build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let mut spec = RunSpec::new("fixture", std::env::temp_dir(), "fixture/accounting");
    spec.limits.max_total_tokens = Some(0);
    let mut p = FixtureProvider::new(vec![]);
    p.bounds = AccountingBounds::default();
    assert!(matches!(
        runtime
            .run(&spec, &p, &mut |_: &RunEvent| panic!("pre-admission"))
            .await,
        Err(RunError::InvalidSpec(_))
    ));
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    let p = FixtureProvider::new(vec![]);
    let (outcome, a) = run(&p, (Some(0), Some(0)), None).await;
    assert_eq!(
        outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::TotalTokens
        }
    );
    assert_eq!(a.model_calls, 0);
    assert_eq!(a.usage.input_tokens, Some(0));
    for (u, cost) in [
        (usage(6, 5), Some(3)),
        (usage(1, 1), Some(9)),
        (usage(u64::MAX, 1), Some(3)),
    ] {
        let p = FixtureProvider::new(vec![turn(true, u, cost)]);
        let (outcome, a) = run(&p, (Some(20), Some(16)), None).await;
        assert!(matches!(
            outcome,
            RunOutcome::Failed {
                code: FailureCode::AccountingBoundViolated,
                ..
            }
        ));
        assert_eq!(a.tool_calls, 0);
        assert_eq!(
            (a.charged_tokens, a.charged_cost_microusd),
            (Some(10), Some(8))
        );
    }
}
#[tokio::test]
async fn cancellation_settles_without_dispatching_after_the_boundary() {
    for point in ["before", "after"] {
        let p = FixtureProvider::new(vec![turn(true, usage(3, 3), Some(3))]);
        let (outcome, a) = run(&p, (Some(20), Some(16)), Some(point)).await;
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert_eq!(a.tool_calls, 0);
        if point == "before" {
            assert_eq!(a.model_calls, 0);
            assert_eq!(a.charged_tokens, Some(0));
        } else {
            assert_eq!(a.model_calls, 1);
            assert_eq!(a.charged_tokens, Some(6));
        }
    }
}
#[tokio::test]
async fn earlier_known_usage_survives_a_later_definitely_unsent_failure() {
    let p = FixtureProvider::new(vec![
        turn(true, usage(3, 3), Some(3)),
        Err(ProviderError {
            code: FailureCode::ProviderTransport,
            delivery: DeliveryCertainty::NotSent,
        }),
    ]);
    let (_, a) = run(&p, (Some(20), Some(16)), None).await;
    assert_eq!(a.model_calls, 2);
    assert_eq!(a.usage.input_tokens, Some(3));
    assert_eq!(a.charged_tokens, Some(6));
    assert_eq!(a.cost_microusd, Some(3));
}

#[tokio::test]
async fn default_call_counts_allow_six_models_and_five_tools() {
    let turns = (0..6)
        .map(|i| {
            let mut events = turn(i < 5, usage(1, 1), Some(1)).unwrap();
            for event in &mut events {
                match event {
                    ProviderEvent::ToolCallStart { id, .. }
                    | ProviderEvent::ToolCallArgumentsDelta { id, .. } => *id = format!("read-{i}"),
                    _ => {}
                }
            }
            Ok(events)
        })
        .collect();
    let p = FixtureProvider::new(turns);
    let (outcome, a) = run(&p, (None, None), None).await;
    assert!(outcome.is_completed());
    assert_eq!((a.model_calls, a.tool_calls), (6, 5));
    assert_eq!(a.usage.input_tokens, Some(6));
    assert_eq!(a.cost_microusd, Some(6));
    assert_eq!(a.charged_tokens, None);
}

#[tokio::test]
async fn cancellation_after_possible_delivery_retains_unknown_liability() {
    let p = FixtureProvider::new(vec![turn(false, usage(1, 1), Some(1))]);
    let (outcome, a) = run(&p, (Some(10), Some(8)), Some("during")).await;
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert_eq!(a.model_calls, 1);
    assert_eq!(a.usage, Usage::default());
    assert_eq!(a.cost_microusd, None);
    assert_eq!(
        (a.charged_tokens, a.charged_cost_microusd),
        (Some(10), Some(8))
    );
}

#[tokio::test]
async fn failed_start_delivery_does_not_count_an_uninvoked_tool() {
    let p = FixtureProvider::new(vec![turn(true, usage(1, 1), Some(1))]);
    let (outcome, a) = run(&p, (Some(10), Some(8)), Some("sink_tool")).await;
    assert!(matches!(
        outcome,
        RunOutcome::Failed {
            code: FailureCode::EventSinkIo,
            ..
        }
    ));
    assert_eq!((a.model_calls, a.tool_calls), (1, 0));
    assert_eq!(a.charged_tokens, Some(2));
}
