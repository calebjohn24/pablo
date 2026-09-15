---
title: Embed in Rust
description: Call pablo-core directly with an injected provider, tracer, event sink, tool registry and cancellation token.
---

# Embed in Rust

`pablo-core` contains the runtime contracts and execution lifecycle. The `pablo` crate owns executable concerns such as CLI parsing, credential files, process protocols and its standalone OTel SDK.

Choose this boundary when your Rust application needs direct, in-process ownership of providers, tools, events, telemetry and cancellation. For an independently upgradeable process boundary, use [ACP](./acp.md) instead.

## Add the source dependency

The workspace packages are marked `publish = false`. Pin a full reviewed Git revision:

```toml
[dependencies]
pablo-core = { git = "https://github.com/calebjohn24/pablo.git", rev = "REPLACE_WITH_FULL_COMMIT_SHA" }
opentelemetry_sdk = { version = "=0.32.1", default-features = false, features = ["trace"] }
tokio = { version = "=1.53.1", features = ["rt", "macros"] }
```

Do not track `main` implicitly in a production application. Review and update the pinned revision deliberately, and validate the constructors and event variants against that source revision.

## Smallest offline run

```rust
use opentelemetry_sdk::trace::SdkTracerProvider;
use pablo_core::{
    JsonlSink, RunSpec, Runtime, ScriptedProvider, telemetry,
};

// Inside an async function running on Tokio:
let sdk = SdkTracerProvider::builder().build();
let runtime = Runtime::new(telemetry::tracer(&sdk));

let spec = RunSpec::new(
    "A synthetic task",
    std::env::current_dir()?,
    "scripted/text-v1",
);
let provider = ScriptedProvider::text(["hello", " world"]);
let mut trace = JsonlSink::new(Vec::new(), &spec)?;

let outcome = runtime.run(&spec, &provider, &mut trace).await?;
```

`Runtime::run` grants no tools. The `ScriptedProvider` follows the asynchronous provider boundary without network I/O, which makes it useful for host tests.

## RunSpec

`RunSpec::new(input, workspace, model)` fills current defaults. A host can then set:

```rust
use std::time::Duration;
use pablo_core::{ReasoningConfig, ReasoningEffort, RunSpec};

let mut spec = RunSpec::new(task, workspace, model);
spec.instructions = "Use cited evidence only.".into();
spec.session_id = Some(application_session_id);
spec.limits.max_run_duration_ms = Duration::from_secs(300).as_millis() as u64;
spec.limits.max_model_calls = Some(4);
spec.limits.max_tool_calls = Some(8);
spec.reasoning = ReasoningConfig::Effort(ReasoningEffort::Low);
```

Validate current constructors against your pinned crate revision when integrating. The [runtime contract](https://github.com/calebjohn24/pablo/blob/main/docs/runtime.md) is authoritative for semantics and defaults.

## Inject a provider

A provider implements one async opening call and returns a bounded stream:

```rust
use pablo_core::provider::{ModelRequest, Provider, ProviderStream};

pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    fn stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
    ) -> futures::future::BoxFuture<
        'a,
        Result<ProviderStream<'a>, pablo_core::provider::ProviderError>,
    >;

    // Optional capability, identity, routing and accounting methods omitted.
}
```

The request carries the stable instructions, complete accepted message history, tool catalog, output allowance, absolute deadline, reasoning intent, cancellation token and active model span context. Credentials belong in the provider object, outside `RunSpec` and model-visible messages.

Provider streams yield normalized text, tool-call fragments and one finish/usage record. Bound untrusted frames before converting them to events. A future/stream must yield promptly and release owned network work when dropped.

Override `validate_model` and `validate_reasoning` to reject unsupported combinations before effects. Override `accounting_bounds` only when the adapter can attest and enforce a complete per-call upper bound; observed usage or a price estimate is insufficient.

## Consume events

`EventSink` is synchronous and ordered:

```rust
use pablo_core::{EventKind, RunEvent, SinkError};

let mut sink = |event: &RunEvent| -> Result<(), SinkError> {
    match &event.kind {
        EventKind::ModelTextDelta { text } => render(text),
        EventKind::RunFinished { outcome, accounting, .. } => {
            persist_terminal(outcome, accounting)
        }
        _ => {}
    }
    Ok(())
};
```

The sink executes inline before the provider stream advances. Return promptly. If your UI or network sink is asynchronous, bridge it through your own bounded queue and ensure shutdown joins its consumer. A sink error stops the run and closes OTel spans.

`JsonlSink<W>` is the provided bounded projection. Give each ordinary run its own sink. For a supervised tree, use `JsonlSink::for_tree` with the root run ID.

## Grant tools

Create a registry explicitly and use `run_with_tools`:

```rust
use pablo_core::{CancellationToken, ToolRegistry};

let tools = ToolRegistry::with_filesystem_reads()?;
let cancellation = CancellationToken::new();

let outcome = runtime
    .run_with_tools(&spec, &provider, &tools, &cancellation, &mut sink)
    .await?;
```

Exact builder signatures vary across the configured registry paths; use the source and examples for your pinned revision. Filesystem writes and shell execution require explicit constructors. An empty catalog grants nothing.

Configured MCP tools use the asynchronous `PreparedRun::tools_with_mcp` path and must be explicitly closed/joined by the owner when unused or after the task.

## Cancel correctly

```rust
let cancellation = CancellationToken::new();
let cancel_from_ui = cancellation.clone();

// Your signal/UI task calls:
cancel_from_ui.cancel();

// Keep awaiting the runtime future here.
```

Cancellation is a request to settle. Do not drop the runtime future after calling `cancel()`. Awaiting lets Pablo close provider work, kill/reap shell groups, join file workers, close MCP sessions and settle child accounting.

## Telemetry ownership

The core accepts a tracer and does not install a global provider or subscriber. Use `telemetry::tracer(&sdk_provider)` to construct the pinned instrumentation scope, or supply another valid OTel tracer implementation.

The standalone executable demonstrates bounded OTLP setup/shutdown. Embedders own the SDK, exporter, resources, incoming context and shutdown policy.

## Configured reference host

`crates/pablo/examples/preset_host.rs` shows an end-to-end configured host. It:

- parses the same deployment inputs as the CLI
- prepares and preflights the selected provider
- creates the root owner and supervised child tools
- attaches a bounded native trace
- constructs configured telemetry
- runs `Runtime` directly
- handles Ctrl-C by cancelling and still awaiting cleanup
- prints the root task result

Build it with:

```sh
cargo build --locked -p pablo --example preset_host
```

The example reuses executable-owned modules from source. Those modules are not promised library APIs; the typed `pablo-core` contracts are the integration boundary.
