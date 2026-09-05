# Rust runtime foundation

The C1.2 implementation runs a bounded, sequential model/tool loop through two crates. `pablo-core` owns contracts, provider normalization, lifecycle transitions, event delivery, and OTel instrumentation. `pablo` owns the standalone SDK and terminal commands. C1.2a adds the direct HTTP gateway adapter and live `run` command through this same lifecycle; `demo` remains offline. See [the gateway contract](gateway.md). Network telemetry export remains deferred.

The contract revision is `c1.2`, not a frozen 0.1 API. [The cycle plan](project/cycles/001-first-spike.md) defines the next extensions.

## Embedding

Create a `RunSpec`, a provider, and an event sink, then await `Runtime::run`. Inject an OTel tracer using `telemetry::tracer(&sdk_provider)` so its scope includes the pinned mapping. A host can supply another OTel tracer implementation if it produces valid span identities. The core never installs a global provider or subscriber.

```rust
use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{JsonlSink, RunSpec, Runtime, ScriptedProvider, telemetry};

// Inside an async function running on Tokio:
let sdk = SdkTracerProvider::builder().build(); // valid IDs, no exporter
let runtime = Runtime::new(telemetry::tracer(&sdk));
let spec = RunSpec::new("A synthetic task", std::env::current_dir()?, "scripted/text-v1");
let provider = ScriptedProvider::text(["hello", " world"]);
let mut trace = JsonlSink::new(Vec::new(), &spec)?;
let outcome = runtime.run(&spec, &provider, &mut trace).await?;
```

An `EventSink` can also be a `Send` closure taking `&RunEvent` and returning `Result<(), SinkError>`. It sees each event inline before the provider stream advances. Hosts can forward the event to `JsonlSink` and a live consumer from that closure. The live consumer always receives text; the JSONL capture setting only affects its stored projection.

Sinks must return promptly. There is no internal event queue, and a synchronous sink's blocking I/O cannot be interrupted by Tokio's deadline. The [ACP adapter](acp.md) hosts its synchronous sink on a dedicated joined thread, with a bounded queue, acknowledged writes and disconnect cleanup. A sink should be dedicated to one run; initialize `JsonlSink` with an empty writer and the same spec used for execution.

## Lifecycle and limits

An admitted run emits `run.started`, one or more streamed model operations, optional sequential tool operations, and exactly one `run.finished` when its future completes and its sink remains writable. Each model call emits `model.started`, text deltas, and `model.finished`. Shell calls emit `tool.started`, `shell.started` with the owned process ID, and `tool.finished` after cleanup. Preflight budget exhaustion emits only the run lifecycle. Invalid configuration is rejected before admission.

`RunOutcome` distinguishes completion, cancellation, timeout, policy denial, limit exhaustion, and failure with delivery certainty (`not_sent`, `may_have_been_sent`, or `response_received`). Completion contains the final model response, finish reason, and aggregate reported usage across all model calls. Intermediate assistant text remains in the event stream and conversation history. A usage field becomes unknown if any contributing model call omits it or its sum overflows. Missing usage and cache counters remain `None`/JSON `null`. Provider `length` finishes indicate token-limit exhaustion; they do not silently become complete answers.

The provider receives the original input, stable instructions and tool catalog, evolving typed message history, model, an `allow_tool_calls` hint, output-token bound, absolute deadline, cancellation token, and active model span context. It owns credentials separately. Adapters should use the instructions, messages, and tools as the request context. Its asynchronous stream yields text, tool-call starts and argument fragments, then one final finish/usage item followed by EOF. Tool-call IDs must be unique within a run. Arguments are assembled within a byte bound and parsed before dispatch. Missing or duplicate finishes, inconsistent finish reasons, unknown fragment IDs, duplicate call IDs, and events after a finish fail explicitly. Opening and streaming failures use closed error codes rather than raw transport messages. There are no retries or fallbacks.

The defaults are 1 MiB of combined input/instructions and tool arguments; 4 MiB per model response; 8 MiB per serialized tool result; 32 MiB of serialized request context; 65,536 requested output tokens per model call; one hour per run; 15 minutes per shell call; and unlimited model/tool call counts. `RunLimits.max_model_calls` and `max_tool_calls` are `Option<u32>`: `None` (JSON null or an omitted field) means unlimited, while `Some(0)` disables those calls. Explicit positive values impose hard caps. Tools execute sequentially, so each run owns at most one shell process group. The 1,000,000-event bound applies to native events and separately to normalized provider items per call. A tool is not executed when no model-call budget remains to consume its result. The core enforces bytes and reported token counts; it does not estimate unknown token usage. Network adapters must bound their own frames before allocating normalized deltas. The scripted fixture stores its caller-supplied script in memory.

The provider future and stream are owned by the run future and dropped on failure, timeout, or cancellation. To cancel a run, cancel its supplied `CancellationToken` and await the run future. Executing shell tools observe cancellation and perform bounded cleanup before returning. Cancellation wins when it and completion are observed together at a control check; a cancellation after terminal delivery does not rewrite the outcome. Dropping the run future itself has process-kill fallbacks but cannot await cleanup or promise a terminal event.

Event count limits reserve two closing slots. JSONL defaults to 256 MiB, reserving a terminal record large enough for the configured output bound, including worst-case JSON escaping when content is enabled. Capacity exhaustion stops the run and writes its typed terminal outcome into the reserve. A later trace error does not replace an earlier provider failure. An I/O failure can leave a partial record; failed terminal delivery returns `RunError::EventDelivery` with the settled outcome and still closes the OTel spans.

`Runtime::run` grants no tools. `Runtime::run_with_tools` accepts an explicit `ToolRegistry` and cancellation token. `ToolRegistry::with_shell()` opts in to the built-in `shell.run` capability. The fixed catalog is advertised unchanged at each step; general host-tool registration is deferred. See [shell execution](shell.md) for its schema, policy, output, and cleanup contract.

## Native trace format

Each JSONL line contains the `c1.2` schema revision, sequence beginning at 1, UTC Unix microsecond timestamp, run/session IDs, lowercase hexadecimal OTel trace/span/parent IDs and flags, and a dotted `type` discriminator. Sequence order is authoritative; wall-clock timestamps may move with the system clock. Each run receives a new run ID and trace. A caller-supplied session ID can correlate independent turns.

Content capture defaults off. Text and final output are replaced before serialization, projected as `null`, and accompanied by byte counts and `content_redacted: true`. Original run input and instructions are never written. Tool arguments, including commands, cwd, and environment additions, are redacted by default; shell stdout/stderr are replaced by null and byte counts while exit/truncation metadata remains. Opted-in tool arguments can contain workspace paths. With content capture enabled, the native events round-trip as `RunEvent`; the redacted JSONL projection intentionally differs at those content fields. Capture does not automatically scrub secrets from opted-in response text.

This first format repeats the small correlation envelope per record. Static-header compression, replay tooling, and measured trace overhead are later work. No replay or durable-session API is claimed.

## OTel mapping

OTel API 0.32.0 and SDK 0.32.1 are pinned. The GenAI mapping uses revision [`fee465db333bdd6a7d2faa320edab5cf3101a4f4`](https://github.com/open-telemetry/semantic-conventions-genai/tree/fee465db333bdd6a7d2faa320edab5cf3101a4f4) from the GenAI conventions repository. Its [registry manifest](https://github.com/open-telemetry/semantic-conventions-genai/blob/fee465db333bdd6a7d2faa320edab5cf3101a4f4/model/manifest.yaml) declares the Development schema `https://opentelemetry.io/schemas/gen-ai-dev/1.42.0-dev` and core conventions 1.44.0. These are the baseline already recorded in the design brief. `pablo --version` exposes the mapping version and immutable revision.

The scope is `pablo`, versioned with the Cargo package and carrying that schema URL. The [pinned span definitions](https://github.com/open-telemetry/semantic-conventions-genai/blob/fee465db333bdd6a7d2faa320edab5cf3101a4f4/model/gen-ai/spans.yaml) inform this implemented subset:

| Operation | Span | Kind | Parent |
| --- | --- | --- | --- |
| Run | `invoke_agent pablo` | INTERNAL | New root |
| Streamed model call | `chat {model}` | CLIENT | Run |
| Tool call | `execute_tool shell.run` | INTERNAL | Run |

Spans start and end at the same transitions and exact microsecond timestamps as native lifecycle events. Native records copy SDK-generated identities directly. The provider receives an explicit model context; no context guard is held across an await. Sampling off preserves valid native identities and does not export spans. A no-op tracer with invalid IDs is rejected.

Standard metadata covers operation, agent name, requested model, provider identity, conversation ID, requested token maximum, streaming flag, finish reasons, and reported input/output/cache usage. OTel receives no prompts, output text, tool content, or per-token events, even when native content capture is enabled. Successful and intentionally cancelled span status stays unset. Failures set ERROR and a closed `error.type`: `timeout`, `limit_exceeded`, `policy_denied`, `provider_rejected`, `provider_transport`, `malformed_stream`, `event_sink_io`, `invalid_tool_arguments`, `tool_execution`, or `tool_cleanup`. A nonzero shell exit marks only its tool span with `shell_exit_nonzero`; the exit and output are returned to the next model call, which can still complete the run. Tool name, type, and call ID use standard `gen_ai.tool.*` attributes.

These provisional custom attributes are scoped to mapping `c1.2`:

| Attribute | Type and values | Cardinality / sensitivity |
| --- | --- | --- |
| `pablo.run.id` | String; generated UUID | Per run; operational identity |
| `pablo.run.outcome` | String; completed, cancelled, timed_out, policy_denied, limit_exceeded, failed | Fixed set; no content |
| `pablo.delivery.certainty` | String; not_sent, may_have_been_sent, response_received | Fixed set; failure metadata |
| `pablo.trace.format.version` | String; c1.2 | Fixed; format revision |
| `pablo.otel.mapping.version` | String; c1.2 | Fixed; convention mapping |
| `pablo.model.output.bytes` | Integer; accepted UTF-8 byte count | Numeric measurement; no content |
| `pablo.model.output.chunks` | Integer; accepted delta count | Numeric measurement; no content |
| `pablo.event.delivery_failed` | Boolean; true if terminal sink delivery fails | Fixed; diagnostic on run span |

Model/provider/session names are bounded metadata supplied by the host and must not contain credentials or task content. The CLI sets `service.name=pablo` and `service.version`. It owns an SDK without exporters and requests shutdown with a two-second timeout. Incoming W3C context, full standard OTel environment configuration, OTLP/HTTP export, and real Collector acceptance remain C1.3–C1.5 work.
