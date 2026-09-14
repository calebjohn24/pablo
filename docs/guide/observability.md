---
title: Observability
description: Consume native events, write bounded JSONL traces, export OpenTelemetry and preserve privacy.
---

# Observability

Pablo emits native lifecycle events and OpenTelemetry spans from the same execution transitions. They share trace/span identities and timestamps but serve different audiences:

- Native events drive terminal/ACP UI, detailed local traces and host accounting.
- OTel exports operational spans without prompts, answers or tool content.

## Native lifecycle

An ordinary task emits:

```text
run.started
  model.started
  model.text_delta …
  model.finished
  tool.started
    shell.started       # shell only
  tool.finished
  model.started
  model.text_delta …
  model.finished
run.finished
```

Model/tool pairs repeat as needed. Events are delivered inline to the host sink before the provider stream advances. A sink must return promptly; blocking sink I/O cannot be interrupted by the async task deadline.

The terminal `run.finished` event carries the settled outcome and aggregate accounting. For supervised children, one root consumer assigns cross-tree ordering while native per-run sequence is preserved.

## Native JSONL trace

```sh
mkdir -p .pablo/traces
pablo run "Summarize README.md." \
  --no-shell \
  --trace .pablo/traces/task.jsonl
```

The target must be a new file. On Unix it is created with mode `0600`. A bounded writer reserves capacity for the root terminal record before accepting nonterminal events, so trace exhaustion can still report its terminal outcome.

Default traces replace content fields with `null` and preserve byte counts, identities, timings, policy decisions, finish reasons and truncation metadata. Input and host instructions are not written.

`--capture-content` explicitly includes model text, tool arguments and tool results in the native trace:

```sh
pablo run "Summarize README.md." \
  --trace .pablo/traces/content.jsonl \
  --capture-content
```

Captured model/tool content may contain sensitive workspace data. The runtime does not attempt to detect secrets in content you deliberately capture.

ACP can use `{session_id}` in a process-level trace path to create one exclusive file per session.

## OpenTelemetry

Pablo creates a root agent span, child model spans and child tool spans. It uses pinned OpenTelemetry libraries and an immutable GenAI semantic-conventions revision. `pablo --version` reports both the runtime and mapping revision.

OTel attributes include bounded identifiers, selected provider/model, operation, finish/outcome status, reported usage and safe deployment/policy metadata. OTel never receives prompts, answer text, tool arguments, filesystem paths or shell output—even when native content capture is on.

Network export is disabled by default. Legacy CLI/ACP can opt into OTLP/HTTP Protobuf using documented OTel options; deployments can configure it explicitly:

```toml
[credentials.collector]
consumer = "otel.headers"
sources = [{ kind = "environment", name = "PABLO_OTLP_HEADERS" }]

[options.otel]
exporter = "otlp"
endpoint = "http://localhost:4318/v1/traces"
headers = "collector"
service_name = "pablo"
propagators = ["tracecontext"]
resource_attributes = { "deployment.environment" = "development" }
```

Exporter work is bounded and owned by the CLI/ACP host. Shutdown cancels in-flight export/retry work, flushes within its allowance and joins the processing thread. Export loss is reported without changing the native task outcome.

## Incoming trace context

Continue W3C context on direct CLI tasks:

```sh
pablo run "Review README.md." \
  --traceparent 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01 \
  --no-shell
```

Add `--tracestate` when needed. Invalid or unsupported context rejects before the task starts. ACP peers use negotiated metadata for incoming context. Baggage has an empty allowlist.

## Failure semantics

OTel status is left unset for success and intentional cancellation. Failures use a closed `error.type`, such as `timeout`, `limit_exceeded`, `policy_denied`, `provider_transport`, `malformed_stream`, `tool_execution` or `tool_cleanup`.

A nonzero shell exit marks its tool span as error while returning the exit/output to the model. The root can still complete if the model handles it.

## Deployment identity

Configured runs attach normalized deployment fingerprints and safe provenance, never secret values or physical workspace paths. The effective fingerprint identifies resolved behavior. A separate host-only bindings fingerprint distinguishes the same portable deployment attached to different absolute roots.

Use `pablo config explain` to inspect this identity offline before execution.

## Collector smoke

Repository development includes a pinned real-Collector fixture:

```sh
npm run smoke:collector
```

It uses an offline model fixture and no provider credential. The check correlates native and exported span identities/timestamps and verifies privacy behavior.
