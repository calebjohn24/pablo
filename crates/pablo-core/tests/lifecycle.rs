use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use futures_util::{future::BoxFuture, stream};
use opentelemetry::trace::{SpanKind, Status, TraceContextExt};
use opentelemetry_sdk::trace::{InMemorySpanExporter, Sampler, SdkTracerProvider};
use pablo_core::{
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    telemetry, *,
};
use tokio::sync::Notify;

fn spec() -> RunSpec {
    RunSpec::new(
        "synthetic-private-input-9ac",
        std::env::current_dir().unwrap(),
        "scripted/text-v1",
    )
}

fn sdk() -> (SdkTracerProvider, InMemorySpanExporter) {
    let exporter = InMemorySpanExporter::default();
    let sdk = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_simple_exporter(exporter.clone())
        .build();
    (sdk, exporter)
}

fn finish() -> ProviderEvent {
    ProviderEvent::Finished {
        reason: FinishReason::Stop,
        usage: Usage::default(),
    }
}

fn script(events: Vec<ProviderEvent>) -> ScriptedProvider {
    ScriptedProvider::new(
        events
            .into_iter()
            .map(|event| (Duration::ZERO, Ok(event)))
            .collect(),
    )
}

fn assert_terminal(events: &[RunEvent], outcome: &RunOutcome) {
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::RunFinished { .. }))
            .count(),
        1
    );
    assert_eq!(
        events.last().unwrap().kind,
        EventKind::RunFinished {
            outcome: outcome.clone()
        }
    );
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.seq, index as u64 + 1);
        assert_eq!(event.run_id, events[0].run_id);
        assert_eq!(event.session_id, events[0].session_id);
        assert_eq!(event.trace_id, events[0].trace_id);
        assert_ne!(event.trace_id, "0".repeat(32));
        assert_ne!(event.span_id, "0".repeat(16));
    }
}

struct GatedProvider {
    observed: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct DropSignal(Arc<AtomicBool>);
impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Provider for GatedProvider {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            assert_eq!(request.model, "scripted/text-v1");
            assert_eq!(request.input, "synthetic-private-input-9ac");
            assert_eq!(request.max_output_tokens, 65_536);
            assert!(request.deadline > tokio::time::Instant::now());
            assert!(request.context.span().span_context().is_valid());
            let guard = DropSignal(self.dropped.clone());
            let stream = stream::unfold((0, guard), move |(step, guard)| async move {
                let event = match step {
                    0 => ProviderEvent::TextDelta("synthetic-private-output-8bd ".into()),
                    1 => {
                        // Completion is impossible until the host observes the first delta.
                        self.observed.notified().await;
                        ProviderEvent::TextDelta("🦀".into())
                    }
                    2 => ProviderEvent::Finished {
                        reason: FinishReason::Stop,
                        usage: Usage {
                            input_tokens: Some(12),
                            output_tokens: Some(3),
                            cache_read_input_tokens: Some(4),
                            cache_write_input_tokens: None,
                        },
                    },
                    _ => return None,
                };
                Some((Ok(event), (step + 1, guard)))
            });
            Ok(Box::pin(stream) as ProviderStream<'a>)
        })
    }
}

#[tokio::test(start_paused = true)]
async fn streams_before_completion_and_correlates_native_records_with_real_otel_spans() {
    let (sdk, exporter) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let observed = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let provider = GatedProvider {
        observed: observed.clone(),
        dropped: dropped.clone(),
    };
    let mut spec = spec();
    spec.session_id = Some("test-session".into());
    spec.trace.capture_content = true;
    let mut native = JsonlSink::new(Vec::new(), &spec).unwrap();
    let mut events = Vec::new();
    let outcome = runtime
        .run(&spec, &provider, &mut |event: &RunEvent| {
            if matches!(event.kind, EventKind::TextDelta { .. }) {
                assert!(
                    exporter.get_finished_spans().unwrap().is_empty(),
                    "delta arrives while both spans remain open"
                );
                observed.notify_one();
            }
            native.emit(event)?;
            events.push(event.clone());
            Ok(())
        })
        .await
        .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(events.len(), 6);
    assert_terminal(&events, &outcome);
    assert!(
        matches!(&outcome, RunOutcome::Completed { output, usage, .. }
        if output == "synthetic-private-output-8bd 🦀" && usage.input_tokens == Some(12) && usage.cache_write_input_tokens.is_none())
    );
    let native = String::from_utf8(native.into_inner()).unwrap();
    let decoded: Vec<RunEvent> = native
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(decoded, events);
    assert!(!native.contains(&spec.input));

    let spans = exporter.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 2);
    let root = spans
        .iter()
        .find(|s| s.name == "invoke_agent pablo")
        .unwrap();
    let model = spans
        .iter()
        .find(|s| s.name == "chat scripted/text-v1")
        .unwrap();
    assert_eq!(root.span_kind, SpanKind::Internal);
    assert_eq!(model.span_kind, SpanKind::Client);
    assert_eq!(model.parent_span_id, root.span_context.span_id());
    assert_eq!(root.parent_span_id, opentelemetry::trace::SpanId::INVALID);
    for (span, first, last) in [(root, 0, 5), (model, 1, 4)] {
        assert_eq!(
            span.span_context.trace_id().to_string(),
            events[first].trace_id
        );
        assert_eq!(
            span.span_context.span_id().to_string(),
            events[first].span_id
        );
        assert_eq!(events[first].span_id, events[last].span_id);
        assert_eq!(
            span.start_time
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros(),
            u128::from(events[first].timestamp_unix_micros)
        );
        assert_eq!(
            span.end_time
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros(),
            u128::from(events[last].timestamp_unix_micros)
        );
        assert_eq!(span.status, Status::Unset);
        assert_eq!(span.instrumentation_scope.name(), "pablo");
        assert_eq!(
            span.instrumentation_scope.schema_url(),
            Some(telemetry::SCHEMA_URL)
        );
        assert_eq!(
            span.instrumentation_scope.version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(
            span.attributes.iter().any(
                |kv| kv.key.as_str() == "pablo.run.id" && kv.value.as_str() == events[0].run_id
            )
        );
        assert!(
            span.events.is_empty(),
            "token deltas must not be copied into OTel"
        );
    }
    for event in &events[1..5] {
        assert_eq!(event.span_id, model.span_context.span_id().to_string());
        assert_eq!(
            event.parent_span_id.as_deref(),
            Some(events[0].span_id.as_str())
        );
        assert_eq!(event.trace_flags, "01");
    }
    assert!(model.attributes.iter().any(|kv| kv.key.as_str()
        == "gen_ai.usage.cache_read.input_tokens"
        && kv.value.as_str() == "4"));
    assert!(
        model
            .attributes
            .iter()
            .any(|kv| kv.key.as_str() == "pablo.model.output.bytes" && kv.value.as_str() == "33")
    );
    let exported = format!("{spans:?}");
    assert!(!exported.contains(&spec.input));
    assert!(!exported.contains("synthetic-private-output-8bd"));
}

#[tokio::test]
async fn scripted_provider_preserves_chunk_order_and_unknown_usage_without_an_exporter() {
    let sdk = SdkTracerProvider::builder().build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let provider = ScriptedProvider::text(["one", " ", "two"]);
    let mut events = Vec::new();
    let outcome = runtime
        .run(&spec(), &provider, &mut |e: &RunEvent| {
            events.push(e.clone());
            Ok(())
        })
        .await
        .unwrap();
    assert_terminal(&events, &outcome);
    assert_eq!(
        outcome,
        RunOutcome::Completed {
            output: "one two".into(),
            finish_reason: FinishReason::Stop,
            usage: Usage::default()
        }
    );
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let EventKind::TextDelta { text } = &e.kind {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(deltas, ["one", " ", "two"]);
}

#[tokio::test]
async fn native_content_redaction_does_not_redact_live_events() {
    let (sdk, _) = sdk();
    let spec = spec();
    let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(
            &spec,
            &ScriptedProvider::text(["secret-output"]),
            &mut trace,
        )
        .await
        .unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { output, .. } if output == "secret-output"));
    let trace = String::from_utf8(trace.into_inner()).unwrap();
    assert!(!trace.contains("secret-output"));
    assert!(!trace.contains(&spec.input));
    let records: Vec<serde_json::Value> = trace
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records[2]["text"], serde_json::Value::Null);
    assert_eq!(records[2]["text_bytes"], 13);
    assert_eq!(
        records.last().unwrap()["outcome"]["output"],
        serde_json::Value::Null
    );
    assert_eq!(records.last().unwrap()["content_redacted"], true);
}

#[tokio::test]
async fn malformed_streams_have_one_typed_failure_and_error_spans() {
    for events in [
        vec![],
        vec![ProviderEvent::TextDelta("partial".into())],
        vec![finish(), finish()],
        vec![finish(), ProviderEvent::TextDelta("late".into())],
    ] {
        let (sdk, exporter) = sdk();
        let mut received = Vec::new();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run(&spec(), &script(events), &mut |e: &RunEvent| {
                received.push(e.clone());
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RunOutcome::Failed {
                code: FailureCode::MalformedStream,
                delivery: DeliveryCertainty::ResponseReceived
            }
        );
        assert_terminal(&received, &outcome);
        assert!(
            exporter
                .get_finished_spans()
                .unwrap()
                .iter()
                .all(|s| matches!(s.status, Status::Error { .. }))
        );
    }
}

#[tokio::test]
async fn byte_model_event_and_token_limits_settle_without_false_completion() {
    let (sdk, _) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    for limit in [
        LimitKind::InputBytes,
        LimitKind::ModelCalls,
        LimitKind::OutputBytes,
        LimitKind::Events,
        LimitKind::OutputTokens,
    ] {
        let mut spec = spec();
        match limit {
            LimitKind::InputBytes => spec.limits.max_input_bytes = 0,
            LimitKind::ModelCalls => spec.limits.max_model_calls = Some(0),
            LimitKind::OutputBytes => spec.limits.max_output_bytes = 3,
            LimitKind::Events => spec.limits.max_events = 4,
            LimitKind::OutputTokens => spec.limits.max_output_tokens = 1,
            _ => unreachable!(),
        }
        let provider = script(vec![
            ProviderEvent::TextDelta("éé".into()),
            ProviderEvent::Finished {
                reason: FinishReason::Stop,
                usage: Usage {
                    output_tokens: Some(2),
                    ..Usage::default()
                },
            },
        ]);
        let mut events = Vec::new();
        let outcome = runtime
            .run(&spec, &provider, &mut |e: &RunEvent| {
                events.push(e.clone());
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(outcome, RunOutcome::LimitExceeded { limit });
        assert_terminal(&events, &outcome);
        assert!(events.len() as u64 <= spec.limits.max_events);
        if matches!(limit, LimitKind::InputBytes | LimitKind::ModelCalls) {
            assert_eq!(
                events.len(),
                2,
                "do not start a model call after preflight exhaustion"
            );
        }
    }
}

#[tokio::test]
async fn provider_length_finish_is_a_limit_even_without_usage() {
    let (sdk, _) = sdk();
    let provider = script(vec![ProviderEvent::Finished {
        reason: FinishReason::Length,
        usage: Usage::default(),
    }]);
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(&spec(), &provider, &mut |_: &RunEvent| Ok(()))
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::LimitExceeded {
            limit: LimitKind::OutputTokens
        }
    );
}

#[tokio::test(start_paused = true)]
async fn timeout_drops_pending_stream_and_ends_both_spans() {
    let (sdk, exporter) = sdk();
    let dropped = Arc::new(AtomicBool::new(false));
    let provider = GatedProvider {
        observed: Arc::new(Notify::new()),
        dropped: dropped.clone(),
    };
    let mut spec = spec();
    spec.limits.max_run_duration_ms = 10;
    let mut events = Vec::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(&spec, &provider, &mut |e: &RunEvent| {
            events.push(e.clone());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(outcome, RunOutcome::TimedOut);
    assert!(dropped.load(Ordering::SeqCst));
    assert_terminal(&events, &outcome);
    assert_eq!(exporter.get_finished_spans().unwrap().len(), 2);
}

struct OpeningProvider {
    dropped: Arc<AtomicBool>,
    fail: bool,
}
impl Provider for OpeningProvider {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn stream<'a>(
        &'a self,
        _: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let _guard = DropSignal(self.dropped.clone());
            if self.fail {
                return Err(ProviderError {
                    code: FailureCode::ProviderRejected,
                    delivery: DeliveryCertainty::NotSent,
                });
            }
            std::future::pending().await
        })
    }
}

#[tokio::test(start_paused = true)]
async fn opening_failure_retains_delivery_certainty_and_opening_timeout_drops_work() {
    for fail in [true, false] {
        let (sdk, _) = sdk();
        let dropped = Arc::new(AtomicBool::new(false));
        let provider = OpeningProvider {
            dropped: dropped.clone(),
            fail,
        };
        let mut events = Vec::new();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run(&spec(), &provider, &mut |e: &RunEvent| {
                events.push(e.clone());
                Ok(())
            })
            .await
            .unwrap();
        assert_terminal(&events, &outcome);
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(
            outcome,
            if fail {
                RunOutcome::Failed {
                    code: FailureCode::ProviderRejected,
                    delivery: DeliveryCertainty::NotSent,
                }
            } else {
                RunOutcome::TimedOut
            }
        );
    }
}

#[tokio::test]
async fn native_trace_capacity_reserves_a_complete_terminal_record() {
    for capture in [false, true] {
        let (sdk, _) = sdk();
        let mut spec = spec();
        spec.limits.max_output_bytes = 2048;
        spec.trace.capture_content = capture;
        spec.trace.max_bytes = if capture { 2048 * 6 + 8192 } else { 8192 };
        let provider = ScriptedProvider::text(std::iter::repeat_n("x", 1000));
        let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
        let outcome = Runtime::new(telemetry::tracer(&sdk))
            .run(&spec, &provider, &mut trace)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            RunOutcome::LimitExceeded {
                limit: LimitKind::TraceBytes
            }
        );
        assert!(trace.bytes_written() <= spec.trace.max_bytes);
        let records: Vec<serde_json::Value> = String::from_utf8(trace.into_inner())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(records.last().unwrap()["type"], "run.finished");
        assert_eq!(records.last().unwrap()["outcome"]["limit"], "trace_bytes");
        assert_eq!(
            records
                .iter()
                .filter(|e| e["type"] == "run.finished")
                .count(),
            1
        );
        for (index, record) in records.iter().enumerate() {
            assert_eq!(record["seq"], index + 1);
        }
    }
}

#[tokio::test]
async fn terminal_reservation_covers_json_escaping_at_the_output_limit() {
    let (sdk, _) = sdk();
    let mut spec = spec();
    spec.limits.max_output_bytes = 512;
    spec.trace.capture_content = true;
    spec.trace.max_bytes = 512 * 6 * 2 + 8192;
    let text = "\u{0000}".repeat(512);
    let mut trace = JsonlSink::new(Vec::new(), &spec).unwrap();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(&spec, &ScriptedProvider::text([text.clone()]), &mut trace)
        .await
        .unwrap();
    assert!(outcome.is_completed());
    let events: Vec<RunEvent> = String::from_utf8(trace.into_inner())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_terminal(&events, &outcome);
}

#[tokio::test]
async fn broken_sink_reports_undelivered_terminal_and_still_ends_spans() {
    let (sdk, exporter) = sdk();
    let mut terminal_attempts = 0;
    let result = Runtime::new(telemetry::tracer(&sdk))
        .run(
            &spec(),
            &ScriptedProvider::text(["x"]),
            &mut |e: &RunEvent| {
                if matches!(e.kind, EventKind::RunFinished { .. }) {
                    terminal_attempts += 1;
                }
                if matches!(
                    e.kind,
                    EventKind::RunStarted | EventKind::ModelStarted { .. }
                ) {
                    return Ok(());
                }
                Err(SinkError::Io(io::Error::other("synthetic I/O error")))
            },
        )
        .await;
    assert!(
        matches!(result, Err(RunError::EventDelivery { outcome, .. }) if matches!(*outcome, RunOutcome::Failed { code: FailureCode::EventSinkIo, .. }))
    );
    assert_eq!(terminal_attempts, 1);
    assert_eq!(exporter.get_finished_spans().unwrap().len(), 2);
}

#[tokio::test]
async fn sampling_off_keeps_valid_native_identities_and_emits_no_exported_spans() {
    let exporter = InMemorySpanExporter::default();
    let sdk = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOff)
        .with_simple_exporter(exporter.clone())
        .build();
    let mut events = Vec::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(
            &spec(),
            &ScriptedProvider::text(["x"]),
            &mut |e: &RunEvent| {
                events.push(e.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_terminal(&events, &outcome);
    assert!(events.iter().all(|e| e.trace_flags == "00"));
    assert!(exporter.get_finished_spans().unwrap().is_empty());
}

#[tokio::test]
async fn separate_turns_share_only_the_supplied_session_identity() {
    let (sdk, exporter) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    let mut spec = spec();
    spec.session_id = Some("shared-session".into());
    let mut first = Vec::new();
    let mut second = Vec::new();
    let provider = ScriptedProvider::new(vec![(Duration::from_millis(1), Ok(finish()))]);
    let mut sink1 = |e: &RunEvent| {
        first.push(e.clone());
        Ok(())
    };
    let mut sink2 = |e: &RunEvent| {
        second.push(e.clone());
        Ok(())
    };
    let (one, two) = tokio::join!(
        runtime.run(&spec, &provider, &mut sink1),
        runtime.run(&spec, &provider, &mut sink2)
    );
    assert_terminal(&first, &one.unwrap());
    assert_terminal(&second, &two.unwrap());
    assert_eq!(first[0].session_id, second[0].session_id);
    assert_ne!(first[0].run_id, second[0].run_id);
    assert_ne!(first[0].trace_id, second[0].trace_id);
    assert_eq!(exporter.get_finished_spans().unwrap().len(), 4);
}

#[tokio::test]
async fn invalid_configuration_is_rejected_before_run_admission() {
    let (sdk, exporter) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    for field in 0..4 {
        let mut spec = spec();
        match field {
            0 => spec.model = "x".repeat(257),
            1 => spec.workspace = "relative".into(),
            2 => spec.limits.max_events = 3,
            _ => spec.session_id = Some("x\n".into()),
        }
        let result = runtime
            .run(
                &spec,
                &ScriptedProvider::text(["x"]),
                &mut |_: &RunEvent| panic!("no event before admission"),
            )
            .await;
        assert!(matches!(result, Err(RunError::InvalidSpec(_))));
    }
    assert!(exporter.get_finished_spans().unwrap().is_empty());
    let mut spec = spec();
    spec.trace.max_bytes = 1;
    assert!(matches!(
        JsonlSink::new(Vec::<u8>::new(), &spec),
        Err(SinkError::Capacity)
    ));
}

#[tokio::test]
async fn stream_failure_is_not_retried_or_hidden_by_a_later_trace_capacity_error() {
    let (sdk, exporter) = sdk();
    let provider = ScriptedProvider::new(vec![
        (
            Duration::ZERO,
            Ok(ProviderEvent::TextDelta("partial".into())),
        ),
        (
            Duration::ZERO,
            Err(ProviderError {
                code: FailureCode::ProviderTransport,
                delivery: DeliveryCertainty::ResponseReceived,
            }),
        ),
        (
            Duration::ZERO,
            Ok(ProviderEvent::TextDelta("must not appear".into())),
        ),
    ]);
    let mut events = Vec::new();
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(&spec(), &provider, &mut |event: &RunEvent| {
            if matches!(event.kind, EventKind::ModelFinished { .. }) {
                return Err(SinkError::Capacity);
            }
            events.push(event.clone());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Failed {
            code: FailureCode::ProviderTransport,
            delivery: DeliveryCertainty::ResponseReceived
        }
    );
    assert_terminal(&events, &outcome);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::ModelStarted { .. }))
            .count(),
        1
    );
    assert!(!format!("{events:?}").contains("must not appear"));
    assert_eq!(exporter.get_finished_spans().unwrap().len(), 2);
}

#[tokio::test]
async fn sink_failure_before_the_model_call_records_that_nothing_was_sent() {
    let (sdk, exporter) = sdk();
    let mut terminal = None;
    let outcome = Runtime::new(telemetry::tracer(&sdk))
        .run(
            &spec(),
            &ScriptedProvider::text(["x"]),
            &mut |event: &RunEvent| {
                if let EventKind::RunFinished { outcome } = &event.kind {
                    terminal = Some(outcome.clone());
                    Ok(())
                } else {
                    Err(SinkError::Io(io::Error::other("unavailable")))
                }
            },
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Failed {
            code: FailureCode::EventSinkIo,
            delivery: DeliveryCertainty::NotSent
        }
    );
    assert_eq!(terminal, Some(outcome));
    assert_eq!(exporter.get_finished_spans().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn default_deadline_allows_long_work_and_still_settles_at_one_hour() {
    let (sdk, _) = sdk();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    for (seconds, completes) in [(901, true), (3601, false)] {
        let provider = ScriptedProvider::new(vec![
            (
                Duration::from_secs(seconds),
                Ok(ProviderEvent::TextDelta("done".into())),
            ),
            (Duration::ZERO, Ok(finish())),
        ]);
        let started = tokio::time::Instant::now();
        let mut events = Vec::new();
        let outcome = runtime
            .run(&spec(), &provider, &mut |event: &RunEvent| {
                events.push(event.clone());
                Ok(())
            })
            .await
            .unwrap();
        if completes {
            assert!(outcome.is_completed());
            assert!(started.elapsed() >= Duration::from_secs(901));
        } else {
            assert_eq!(outcome, RunOutcome::TimedOut);
            assert_eq!(started.elapsed(), Duration::from_secs(3600));
        }
        assert_terminal(&events, &outcome);
    }
}
