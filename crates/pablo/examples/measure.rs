//! C1.6 measurement host. Not part of the shipped executable or runtime API.
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/deployment.rs"]
mod deployment;

use futures::{StreamExt, future::BoxFuture, stream};
use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{
    CancellationToken, EventKind, EventSink, FinishReason, JsonlSink, Provider, RunEvent, RunSpec,
    Runtime, SinkError, Usage,
    gateway::GatewayProvider,
    provider::{ModelRequest, ProviderError, ProviderEvent, ProviderStream},
    telemetry,
};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const CHUNKS: usize = 1000;
const CHUNK: &str = "0123456789abcdef0123456789abcdef";
struct TimedProvider(Arc<Mutex<Option<Instant>>>);
impl Provider for TimedProvider {
    fn name(&self) -> &'static str {
        "measurement"
    }
    fn stream<'a>(
        &'a self,
        _: ModelRequest<'a>,
    ) -> BoxFuture<'a, Result<ProviderStream<'a>, ProviderError>> {
        Box::pin(async move {
            let events = (0..CHUNKS)
                .map(|_| ProviderEvent::TextDelta(CHUNK.into()))
                .chain(std::iter::once(ProviderEvent::Finished {
                    reason: FinishReason::Stop,
                    usage: Usage::default(),
                }));
            Ok(Box::pin(stream::iter(events).map(|event| {
                if matches!(event, ProviderEvent::TextDelta(_)) {
                    *self.0.lock().unwrap() = Some(Instant::now());
                }
                Ok(event)
            })) as ProviderStream<'a>)
        })
    }
}
fn stats(values: &[f64]) -> Value {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |p: f64| sorted[((p * sorted.len() as f64).ceil() as usize).saturating_sub(1)];
    json!({ "n": sorted.len(), "min": sorted[0], "p50": at(0.5), "p95": at(0.95), "p99": at(0.99), "max": sorted[sorted.len()-1] })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("mode required")?;
    let sdk = SdkTracerProvider::builder().build();
    let runtime = Runtime::new(telemetry::tracer(&sdk));
    if mode == "events" {
        let directory = args.next().ok_or("temporary directory required")?;
        let samples: usize = args.next().ok_or("sample count required")?.parse()?;
        let spec = RunSpec::new(
            "measure",
            Path::new(&directory).canonicalize()?,
            "fixture/measure",
        );
        let provider = TimedProvider(Arc::new(Mutex::new(None)));
        let mut durations = [Vec::new(), Vec::new()];
        let mut latencies = Vec::new();
        let mut bytes = 0;
        // Alternate pairs to reduce ordering bias; five warm-up pairs excluded.
        for pair in 0..samples + 5 {
            for index in 0..2 {
                let captured = (pair + index) % 2;
                let mut trace = if captured == 1 {
                    Some(JsonlSink::new(
                        BufWriter::new(File::create(Path::new(&directory).join("measure.jsonl"))?),
                        &spec,
                    )?)
                } else {
                    None
                };
                let mut observed = Vec::with_capacity(CHUNKS);
                let mut count = 0;
                let start = Instant::now();
                let result = runtime
                    .run(
                        &spec,
                        &provider,
                        &mut |event: &RunEvent| -> Result<(), SinkError> {
                            if matches!(event.kind, EventKind::TextDelta { .. }) {
                                observed.push(
                                    provider
                                        .0
                                        .lock()
                                        .unwrap()
                                        .take()
                                        .unwrap()
                                        .elapsed()
                                        .as_nanos() as f64,
                                );
                                count += 1;
                            }
                            if let Some(trace) = trace.as_mut() {
                                trace.emit(event)?;
                            }
                            Ok(())
                        },
                    )
                    .await?;
                if let Some(trace) = trace {
                    bytes = trace.bytes_written();
                    trace.into_inner().flush()?;
                }
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                assert!(result.is_completed());
                assert_eq!(count, CHUNKS);
                if pair >= 5 {
                    durations[captured].push(elapsed);
                    if captured == 0 {
                        latencies.extend(observed);
                    }
                }
            }
        }
        println!(
            "{}",
            json!({"mode":"events", "chunks_per_run":CHUNKS, "bytes_per_chunk":CHUNK.len(),
            "warmup_pairs":5, "samples":samples, "event_latency_ns":stats(&latencies),
            "no_trace_ms":stats(&durations[0]), "trace_ms":stats(&durations[1]),
            "no_trace_samples_ms":durations[0], "trace_samples_ms":durations[1], "trace_bytes_per_run":bytes})
        );
    } else if mode == "http" {
        let endpoint = args.next().ok_or("fixture endpoint required")?;
        // Reuse the actual CLI configuration, including instructions and caps.
        let mut options = config::Options::parse("run".into(), args.map(Into::into))?;
        if let Some(bootstrap) = &mut options.deployment {
            if bootstrap
                .fixture_endpoint
                .as_ref()
                .is_some_and(|value| value != &endpoint)
            {
                return Err("conflicting measurement fixture endpoints".into());
            }
            bootstrap.fixture_endpoint = Some(endpoint.clone());
        }
        let prepared = options.prepare_run(None, None, Some(uuid::Uuid::new_v4().to_string()))?;
        let spec = match &prepared {
            Some(prepared) => prepared.spec().clone(),
            None => options.spec()?,
        };
        let provider: Box<dyn Provider> = match &prepared {
            Some(prepared) => {
                let bootstrap = options.deployment.as_ref().unwrap();
                // The HTTP measurement always uses an explicit loopback fixture.
                if bootstrap.fixture_endpoint.as_deref() != Some(endpoint.as_str()) {
                    return Err(
                        "measurement requires the matching explicit fixture endpoint".into(),
                    );
                }
                deployment::Secrets::read(prepared, bootstrap)?.provider(bootstrap)?
            }
            None => Box::new(GatewayProvider::local_fixture_for(
                options.provider.unwrap_or_default(),
                &endpoint,
            )?),
        };
        let tools = match &prepared {
            Some(prepared) => prepared.tools()?,
            None => options.tools()?,
        };
        let runtime = match &prepared {
            Some(prepared) => runtime.with_deployment(prepared.deployment()),
            None => runtime,
        };
        pablo_core::runtime::validate_run(&spec, provider.as_ref(), &tools)?;
        let mut trace = prepared
            .as_ref()
            .map(|p| p.create_trace_file())
            .transpose()?
            .flatten()
            .map(|file| JsonlSink::new(BufWriter::new(file), &spec))
            .transpose()?;
        let mut count = 0;
        let mut first_ms = None;
        let start = Instant::now();
        let outcome = runtime
            .run_with_tools(
                &spec,
                provider.as_ref(),
                &tools,
                &CancellationToken::new(),
                &mut |event: &RunEvent| -> Result<(), SinkError> {
                    if let Some(trace) = &mut trace {
                        trace.emit(event)?;
                    }
                    count += 1;
                    if first_ms.is_none() && matches!(event.kind, EventKind::TextDelta { .. }) {
                        first_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
                    }
                    Ok(())
                },
            )
            .await?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        assert!(outcome.is_completed());
        println!(
            "{}",
            json!({"mode":"http", "run_ms":elapsed, "first_text_ms":first_ms, "events":count, "outcome":outcome,
                "deployment":prepared.as_ref().map(|p| p.deployment().identity())})
        );
    } else {
        return Err("unknown measurement mode".into());
    }
    sdk.shutdown_with_timeout(Duration::from_secs(2))?;
    Ok(())
}
