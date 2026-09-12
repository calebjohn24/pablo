# Telemetry export (C1.5)

Pablo creates native OTel spans at the same lifecycle transitions as JSONL events. The standalone CLI and ACP worker own the same SDK/exporter setup. The Rust core accepts an injected tracer and never installs global providers or a subscriber. Export is off by default, and content is always absent from OTLP, including when native `--capture-content` is enabled.

Enable traces to a local Collector:

```sh
OTEL_TRACES_EXPORTER=otlp \
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
pablo demo
```

`run`, `demo`, and `acp --stdio` share export settings. Exporter credentials come only from the process environment, independently of the privately parsed provider credential file. The `.env` provider loader does not install environment variables or load exporter settings.

## Incoming context

Embedded Rust hosts use `Runtime::new(tracer).with_parent_context(context)`. The host passes the OTel context explicitly; the runtime does not read thread-local context. A protocol adapter must first extract a remote context. Each runtime invocation starts its own run span under that supplied parent, with model and tool spans beneath the run. Native root records include the external parent ID; all records reuse SDK IDs and exact lifecycle timestamps.

CLI `run` and `demo` accept `--traceparent VALUE` and optional `--tracestate VALUE`. ACP clients negotiate `clientCapabilities._meta["pablo/v2"] = true`, then send:

```json
{
  "sessionId": "<session/new result>",
  "prompt": [{ "type": "text", "text": "Inspect the workspace" }],
  "_meta": {
    "pablo/v2": {
      "traceparent": "00-a123456789abcdef0123456789abcdef-b123456789abcdef-01",
      "tracestate": "pablofixture=parent"
    }
  }
}
```

The pinned stable ACP schemas have no dedicated trace-context fields, so C1.5 adds these fields inside the existing negotiated namespace. Unnegotiated incoming metadata is ignored. Extraction uses the pinned SDK's W3C propagator; each field is capped at 512 bytes. Invalid/oversized trace context starts a local trace with a fixed diagnostic. Invalid tracestate is discarded by the propagator. Context never enters model messages, tool arguments, or the shell environment. Baggage has an empty allowlist and is ignored. Fan-in links and broader propagation remain future work.

## Configuration

| Setting | Behavior |
| --- | --- |
| `OTEL_TRACES_EXPORTER` | Unset, empty, or `none`: no network. `otlp`: enable HTTP Protobuf. Other values disable export with a fixed diagnostic. |
| `OTEL_SDK_DISABLED=true` | Disable recording and export; native events retain valid unsampled SDK identities. |
| `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` | Service-name variable takes precedence over the resource attribute; fallback `pablo`. Other resource attributes use the SDK detector. `service.version` is the executable version. |
| `OTEL_PROPAGATORS` | Default `tracecontext`. `none`, or a list without `tracecontext`, disables incoming extraction. Other propagators are not installed; baggage remains ignored. |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | Pinned SDK's always-on/off, trace-ID ratio, and parent-based variants. Default parent-based always-on respects unsampled incoming parents. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | General base URL; the SDK appends `/v1/traces`. Default when explicitly enabled: `http://localhost:4318/v1/traces`. |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | Overrides the general endpoint and is used as-is. |
| `OTEL_EXPORTER_OTLP[_TRACES]_PROTOCOL` | Only `http/protobuf`; signal-specific value takes precedence. Unsupported protocols disable export with a fixed diagnostic. |
| `OTEL_EXPORTER_OTLP[_TRACES]_HEADERS` | SDK's comma-separated, percent-decoded header configuration. Signal-specific headers replace the general set. Values are never printed or copied to runtime contracts. |
| `OTEL_EXPORTER_OTLP[_TRACES]_COMPRESSION` | SDK settings: none or gzip. Signal-specific setting takes precedence. |
| `OTEL_EXPORTER_OTLP[_TRACES]_TIMEOUT` | Milliseconds, signal-specific first; default 10,000. Bounds HTTP attempts and the complete batch export, including retries. Invalid/nonpositive values use the default. |
| `OTEL_BSP_MAX_QUEUE_SIZE`, `OTEL_BSP_MAX_EXPORT_BATCH_SIZE`, `OTEL_BSP_SCHEDULE_DELAY` | SDK bounded batch queue, batch size and delay; defaults 2,048 spans, 512 spans, 5,000 ms. Queue overflow drops telemetry without blocking model/tool work. |
| `OTEL_BSP_EXPORT_TIMEOUT` | Whole-batch deadline in milliseconds; default 30,000. The earlier of this and the OTLP timeout applies. Exports are sequential. |

Resource attributes, model/session names, and tracestate are operator-supplied metadata and must contain no credentials or task content. Exporter headers are sent only to the configured endpoint. HTTP redirects are not followed. HTTPS uses the existing rustls/system-root client. Explicit custom CA, client-certificate/key and insecure-mode OTLP variables are unsupported by this slice and disable export rather than silently changing transport security. Metrics/log exporters and declarative `OTEL_CONFIG_FILE` are unsupported and reported with fixed diagnostics; they create no pipelines.

## Failure and ownership

The SDK batch processor owns one joined thread and its own current-thread Tokio executor. Runtime span completion only enqueues bounded metadata. The HTTP adapter caps response bodies at 64 KiB, retains private headers outside span data, and suppresses arbitrary Collector response messages. SDK internal logs are disabled so endpoints, credentials and response bodies cannot reach stderr through dependency diagnostics.

The SDK's experimental HTTP retry implementation supplies bounded exponential backoff and jitter (three retries). A small adapter corrects its overly broad status classification: only 429, 502, 503 and 504 responses are retryable, alongside connection failures. Retry-After on 429/503 is honored within the batch/shutdown deadlines. Other errors, redirects, malformed successful responses, and partial success do not retry. Partial rejection counts and warnings produce sanitized diagnostics. These rules follow the [OTLP HTTP specification](https://opentelemetry.io/docs/specs/otlp/#otlphttp-response).

After model/tool work settles, shutdown allows up to two seconds for export. At 1.9 seconds it cancels pending HTTP/retry futures, drains/discards queued exports and lets the batch worker join before the SDK deadline. This also runs on ACP cancellation/disconnect. Lost spans are reported by count to stderr; they do not change native terminal outcomes, enter model context, or corrupt ACP stdout. The native JSONL stream remains the run lifecycle, without adding post-terminal exporter events.

## Reproducible proof

```sh
npm run test:telemetry
npm run smoke:collector
```

Ordinary tests use local HTTP/SSE fixtures and synthetic credentials. The explicit smoke downloads the official `otelcol` **0.160.0** archive into ignored `.cache/`, verifies the checked-in SHA-256, re-extracts the executable, and starts the [pinned configuration](../tests/fixtures/collector/config.yaml) on a loopback ephemeral port. The [lock](../tests/fixtures/collector/lock.json) includes macOS/Linux amd64/arm64 assets. It requires Node 24, the existing Rust toolchain, and `tar`; no Docker or paid provider is needed.

The real Collector accepts Pablo's HTTP Protobuf and forwards decoded OTLP JSON to a temporary loopback assertion server. The fixture checks exactly one run, two model calls and one real shell call; remote W3C parent and tracestate; native/span IDs and exact timestamps; resource/scope attributes; metadata-only content; and credential absence. Both Collector and Pablo exit, and temporary workspaces are removed. Raw traces and payloads are not committed. Platform execution evidence is recorded separately; asset availability does not claim Linux acceptance.

For ACP, C1.7 keeps the SDK and its bounded batch processor alive across successive independent tasks in the process. Task completion does not force a network flush; process close awaits owned work and the existing bounded SDK shutdown. Each task still supplies its own incoming parent context. CLI SDK ownership is unchanged.
