---
title: ACP integration
description: Host Pablo over stable ACP v1 with independent sessions, streaming updates, cancellation and typed outcomes.
---

# ACP integration

The Agent Client Protocol (ACP) is Pablo’s process boundary for editors and application hosts. The executable reads/writes newline-delimited protocol frames over stdio while all provider and tool work stays inside the same native runtime lifecycle.

## Start the agent

```sh
pablo acp --stdio --no-shell
```

The process performs ACP initialization, creates sessions, accepts one prompt per session, streams updates and returns a standard stop reason plus negotiated Pablo metadata.

Process-level options select provider, model, tools, limits, deployment and trace behavior:

```sh
pablo acp --stdio \
  --provider openrouter \
  --no-shell \
  --timeout 300 \
  --trace '.pablo/traces/{session_id}.jsonl'
```

`{session_id}` is expanded to keep per-session trace files exclusive.

## Session model

One process can serve successive independent sessions. Each session accepts one prompt. Provider HTTP clients, compiled built-in tool schemas and telemetry setup can remain warm, while each task receives fresh:

- run/session/trace identity
- workspace handle and policy checks
- model conversation context
- MCP sessions and credentials
- child supervisor and accounting ledger
- cancellation token and trace destination

The process does not provide durable chat history. A host that needs conversation persistence stores it in its own application and creates the next task deliberately.

## TypeScript reference client

The repository includes a client built on the pinned official stable ACP SDK:

```sh
nvm use
npm ci
cargo build --locked -p pablo --bin pablo
node examples/acp-client.ts \
  "Read README.md and summarize it." \
  "$PWD"
```

By default the example starts `target/debug/pablo`. It negotiates the Pablo extension, streams agent text/tool updates, prints the terminal outcome and sends cancellation when interrupted.

The module also exports helpers:

- `outcomeOf(response)` validates and returns the native outcome.
- `taskOf(response)` validates the complete task result and accounting.
- `structuredOf(response)` parses final JSON only after local output validation succeeded.

## Negotiation

Pablo uses stable ACP v1 and project-controlled extension metadata. Current C3 tasks negotiate `pablo/v2`; frozen C2 clients can remain on `pablo/v1` for compatible unconfigured tasks. A configured C3 request requires explicit support for the current metadata version.

Standard ACP stop reasons control the protocol turn. The Pablo task object preserves richer native truth such as cancellation, policy denial, limit exhaustion, delivery certainty and exact decimal-string accounting.

Do not infer a native outcome from the standard stop reason alone.

## Streaming and backpressure

Native model/tool events are projected to ACP session notifications in order. The stdio adapter:

- bounds incoming frames before JSON decoding
- uses a bounded event queue between the runtime worker and protocol writer
- waits for physical stdout acknowledgement before admitting more projected output
- allows cancellation processing while a producer is waiting for capacity
- coalesces only the events permitted by the negotiated contract
- joins the runtime worker and owned tools on disconnect

If the client stops reading, the transport deadline or disconnect path cancels owned work. The adapter never reports cleanup before it joins the worker.

## Cancellation

Send the standard ACP cancellation request for the active session, then continue reading until the prompt response or connection close. Pablo propagates cancellation to provider requests, local shell groups, MCP work and child tasks, waits for owned cleanup and preserves uncertain remote completion where applicable.

A late cancellation after the native terminal was settled does not rewrite trace history. Negotiated metadata remains the authoritative native result.

## Workspace and capabilities

ACP `session/new.cwd` establishes the workspace for legacy tasks. For configured deployments it must match the deployment’s bound workspace. A session cannot use its cwd to select a configuration file, inject credentials, add an MCP server or widen tool policy.

Client MCP definitions may select an exact subset of host-approved server definitions. Commands, arguments, endpoints and secret bindings must match the host configuration; client-supplied secrets reject.

## Machine outcome

Treat decimal-string counters as `BigInt`:

```ts
import { taskOf } from "./examples/acp-client.ts";

const task = taskOf(response);
const modelCalls = BigInt(task.accounting.model_calls);

switch (task.outcome.status) {
  case "completed":
    console.log(task.outcome.output);
    break;
  case "cancelled":
  case "timed_out":
    break;
  case "policy_denied":
  case "limit_exceeded":
  case "failed":
    console.error(task.outcome);
    break;
}
```

Validate the extension schema and fail on unknown revisions. The checked-in ACP v1/v2 schemas document the exact wire envelope.

## Host checklist

Before production use:

1. Pin the ACP SDK/protocol versions used by your host.
2. Pass an explicit workspace and process-level deployment.
3. Consume updates continuously and bound your own UI/storage queues.
4. On cancellation or shutdown, keep reading while Pablo joins cleanup.
5. Parse negotiated task metadata for native outcome and accounting.
6. Keep credentials out of session input, MCP client definitions and logs.
7. Restart the process when process-level provider/tool/exporter configuration changes.
