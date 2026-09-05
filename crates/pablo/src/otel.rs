//! Standalone SDK ownership. The core continues to accept a host's tracer.
use opentelemetry::{
    Context, Key, KeyValue, propagation::TextMapPropagator, trace::TraceContextExt,
};
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    propagation::TraceContextPropagator,
    trace::{
        BatchSpanProcessor, Sampler, SdkTracerProvider, Span, SpanData, SpanExporter, SpanProcessor,
    },
};
use pablo_core::CancellationToken;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const FLUSH: Duration = Duration::from_secs(2);

pub struct Telemetry {
    pub sdk: SdkTracerProvider,
    cancel: CancellationToken,
    counts: Arc<Counts>,
}

#[derive(Debug, Default)]
struct Counts {
    ended: AtomicU64,
    exported: AtomicU64,
    rejected: AtomicU64,
}

impl Telemetry {
    pub fn new() -> Self {
        let cancel = CancellationToken::new();
        let counts = Arc::new(Counts::default());
        // The pinned SDK detector order otherwise lets resource attributes
        // overwrite OTEL_SERVICE_NAME. Apply its required precedence explicitly.
        let resource = Resource::builder().build();
        let explicit_name = std::env::var("OTEL_SERVICE_NAME")
            .ok()
            .filter(|v| !v.is_empty());
        let resource_name = opentelemetry_sdk::resource::EnvResourceDetector::new();
        use opentelemetry_sdk::resource::ResourceDetector;
        let service = explicit_name
            .or_else(|| {
                resource_name
                    .detect()
                    .get(&Key::new("service.name"))
                    .map(|v| v.to_string())
            })
            .unwrap_or_else(|| "pablo".into());
        let resource = Resource::builder_empty()
            .with_attributes(
                resource
                    .iter()
                    .map(|(k, v)| KeyValue::new(k.clone(), v.clone())),
            )
            .with_service_name(service)
            .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
            .build();
        let mut builder = SdkTracerProvider::builder().with_resource(resource);
        if std::env::var("OTEL_SDK_DISABLED").is_ok_and(|v| v.eq_ignore_ascii_case("true")) {
            // Preserve native correlation IDs even when OTel recording is disabled.
            builder = builder.with_sampler(Sampler::AlwaysOff);
        } else {
            for name in [
                "OTEL_METRICS_EXPORTER",
                "OTEL_LOGS_EXPORTER",
                "OTEL_CONFIG_FILE",
            ] {
                if std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "none") {
                    eprintln!("pablo: unsupported {name}; traces are the only export signal");
                }
            }
            match std::env::var("OTEL_TRACES_EXPORTER").as_deref() {
                Ok("otlp") => match exporter(cancel.clone(), counts.clone()) {
                    Ok(exporter) => {
                        builder = builder.with_span_processor(CountedProcessor {
                            inner: BatchSpanProcessor::builder(exporter).build(),
                            counts: counts.clone(),
                        })
                    }
                    Err(()) => {
                        eprintln!("pablo: invalid OTLP configuration; network telemetry disabled")
                    }
                },
                Ok("none" | "") | Err(_) => {}
                Ok(_) => {
                    eprintln!("pablo: unsupported OTEL_TRACES_EXPORTER; network telemetry disabled")
                }
            }
        }
        Self {
            sdk: builder.build(),
            cancel,
            counts,
        }
    }

    pub async fn shutdown(self) {
        let Self {
            sdk,
            cancel,
            counts,
        } = self;
        // The SDK batch worker owns a separate current-thread executor. Wake its
        // HTTP/retry future before the SDK's deadline, then join the worker.
        let mut shutdown = tokio::task::spawn_blocking(move || sdk.shutdown_with_timeout(FLUSH));
        let result = tokio::select! {
            result = &mut shutdown => result,
            _ = tokio::time::sleep(FLUSH - Duration::from_millis(100)) => {
                cancel.cancel();
                shutdown.await
            }
        };
        if !matches!(result, Ok(Ok(()))) {
            eprintln!("pablo: telemetry shutdown deadline reached");
        }
        let dropped = counts
            .ended
            .load(Ordering::Relaxed)
            .saturating_sub(counts.exported.load(Ordering::Relaxed))
            .saturating_add(counts.rejected.load(Ordering::Relaxed));
        if dropped > 0 {
            eprintln!(
                "pablo: telemetry dropped {dropped} spans (export failure, queue capacity or shutdown deadline)"
            );
        }
    }
}

fn exporter(cancel: CancellationToken, counts: Arc<Counts>) -> Result<OwnedExporter, ()> {
    let protocol = std::env::var("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL")
        .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL"));
    if protocol.is_ok_and(|v| v != "http/protobuf") {
        return Err(());
    }
    // These TLS options are not implemented by the pinned Rust HTTP exporter.
    // Never silently send without a requested certificate configuration.
    for suffix in [
        "CERTIFICATE",
        "CLIENT_CERTIFICATE",
        "CLIENT_KEY",
        "INSECURE",
    ] {
        if std::env::var_os(format!("OTEL_EXPORTER_OTLP_{suffix}")).is_some()
            || std::env::var_os(format!("OTEL_EXPORTER_OTLP_TRACES_{suffix}")).is_some()
        {
            return Err(());
        }
    }
    let timeout = std::env::var("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT")
        .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_TIMEOUT"))
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(10_000);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(timeout))
        .build()
        .map_err(|_| ())?;
    let inner = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .with_http_client(BoundedHttp(client, counts.clone()))
        .build()
        .map_err(|_| ())?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ())?;
    Ok(OwnedExporter {
        inner,
        executor: Mutex::new(Some(executor)),
        cancel,
        counts,
        export_timeout: Duration::from_millis(
            timeout.min(
                std::env::var("OTEL_BSP_EXPORT_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v > 0)
                    .unwrap_or(30_000),
            ),
        ),
    })
}

/// Return status codes intact for the SDK's OTLP retry classification, bound
/// response memory, and never follow redirects with exporter credentials.
struct BoundedHttp(reqwest::Client, Arc<Counts>);
impl std::fmt::Debug for BoundedHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BoundedHttp")
    }
}
#[async_trait::async_trait]
impl HttpClient for BoundedHttp {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        let mut response = self.0.execute(request.try_into()?).await?;
        // The pinned SDK retries every 5xx and every redirect, and only honors
        // Retry-After for 429. Adapt classification to OTLP's four retry codes.
        let status = response.status().as_u16();
        if status != 200 {
            let classified = match status {
                503 if response.headers().contains_key("retry-after") => 429,
                429 | 502 | 503 | 504 => status,
                _ => 400,
            };
            let mut builder = Response::builder().status(classified);
            *builder.headers_mut().ok_or("invalid response")? = response.headers().clone();
            return Ok(builder.body(Bytes::new())?);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > 64 * 1024 {
                return Err("OTLP response exceeds 64 KiB".into());
            }
            body.extend_from_slice(&chunk);
        }
        use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
        use prost::Message;
        let Ok(decoded) = ExportTraceServiceResponse::decode(body.as_slice()) else {
            return Ok(Response::builder().status(400).body(Bytes::new())?);
        };
        if let Some(partial) = decoded.partial_success {
            self.1
                .rejected
                .fetch_add(partial.rejected_spans.max(0) as u64, Ordering::Relaxed);
            if partial.rejected_spans > 0 || !partial.error_message.is_empty() {
                eprintln!(
                    "pablo: Collector reported partial success or a warning (message withheld)"
                );
            }
        }
        Ok(Response::builder().status(200).body(body.into())?)
    }
}

struct OwnedExporter {
    inner: opentelemetry_otlp::SpanExporter,
    executor: Mutex<Option<tokio::runtime::Runtime>>,
    export_timeout: Duration,
    cancel: CancellationToken,
    counts: Arc<Counts>,
}
impl std::fmt::Debug for OwnedExporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnedExporter")
    }
}
impl SpanExporter for OwnedExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let count = batch.len() as u64;
        let executor = self.executor.lock().unwrap();
        let result = executor
            .as_ref()
            .ok_or(OTelSdkError::AlreadyShutdown)?
            .block_on(async {
                tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => Err(OTelSdkError::Timeout(FLUSH)),
                    result = tokio::time::timeout(self.export_timeout, self.inner.export(batch)) => result.unwrap_or(Err(OTelSdkError::Timeout(self.export_timeout))),
                }
            });
        if result.is_ok() {
            self.counts.exported.fetch_add(count, Ordering::Relaxed);
        }
        // SDK internal logging is disabled; also sanitize the processor result.
        result.map_err(|_| OTelSdkError::InternalFailure("OTLP export failed".into()))
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let result = self.inner.shutdown_with_timeout(timeout);
        // Called on the batch thread, outside any async execution context.
        self.executor.lock().unwrap().take();
        result
    }
}

#[derive(Debug)]
struct CountedProcessor {
    inner: BatchSpanProcessor,
    counts: Arc<Counts>,
}
impl SpanProcessor for CountedProcessor {
    fn on_start(&self, span: &mut Span, cx: &Context) {
        self.inner.on_start(span, cx);
    }
    fn on_end(&self, span: SpanData) {
        if span.span_context.is_sampled() {
            self.counts.ended.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.on_end(span);
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

/// Only W3C trace context crosses this boundary; baggage has an empty allowlist.
/// Invalid/oversized fields are ignored with a fixed diagnostic, never echoed.
pub fn parent(traceparent: Option<&str>, tracestate: Option<&str>) -> Context {
    let propagators = std::env::var("OTEL_PROPAGATORS").unwrap_or_else(|_| "tracecontext".into());
    if !propagators.split(',').any(|p| p.trim() == "tracecontext") {
        return Context::new();
    }
    let Some(traceparent) = traceparent else {
        return Context::new();
    };
    if traceparent.len() > 512 || tracestate.is_some_and(|v| v.len() > 512) {
        eprintln!("pablo: invalid incoming trace context; starting a local trace");
        return Context::new();
    }
    let mut carrier = HashMap::from([("traceparent".to_owned(), traceparent.to_owned())]);
    if let Some(state) = tracestate {
        carrier.insert("tracestate".into(), state.into());
    }
    let context = TraceContextPropagator::new().extract_with_context(&Context::new(), &carrier);
    if !context.span().span_context().is_valid() {
        eprintln!("pablo: invalid incoming trace context; starting a local trace");
    }
    context
}
