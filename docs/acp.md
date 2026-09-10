# ACP process boundary

C1.3 adds `pablo acp --stdio`, using stable ACP v1 over newline-delimited UTF-8 JSON. The official Rust SDK owns method dispatch, JSON-RPC IDs, notifications, and errors. The TypeScript reference client uses the official SDK's stable package entry point. [The lock manifest](acp-lock.json) records exact SDK releases, source revisions, and schema fingerprints; Cargo/npm lockfiles pin their dependency trees. Rust SDK **2.1.0** is a library version; the wire protocol is **1**. No unstable Rust features or experimental TypeScript entry points are enabled.

## Run the reference client

```sh
npm ci
cargo build --locked -p pablo
node examples/acp-client.ts "Read README.md and summarize it." /absolute/workspace
```

This command makes a live gateway request using the same privately loaded credential and model defaults as `pablo run`. Extra arguments after the workspace are passed to the executable, for example `--no-shell`, `--model google/gemini-3.8-flash`, `--max-tool-calls 10`, `--max-model-calls 20`, `--timeout 3600`, `--tool-timeout 900`, `--env-file /private/config.env`, or `--trace .pablo/traces/acp.jsonl`. Trace files must be new. For successive tasks in one process, use a per-session path such as `--trace ".pablo/traces/{session_id}.jsonl"`; the generated session ID replaces that placeholder. A fixed path remains exclusive and cannot be reused or overwritten. `--capture-content` requires `--trace`. Credentials are loaded from the invoking directory or the host's explicit file, independently of the session workspace. The reference client forwards Ctrl-C as `session/cancel` and waits for cleanup.

For another ACP host, spawn `target/debug/pablo acp --stdio` with stdin/stdout pipes. The host selects the working directory in `session/new`; `--workspace` is rejected in ACP mode. Diagnostics use stderr, and stdout carries only protocol messages. The current transport supports Unix pipes and sockets; C1.6 verifies macOS and Linux arm64.

## Session contract

A process admits successive independent in-memory sessions, with one prompt per session and one current session at a time. After the prompt response, `session/new` retires the previous session and creates a fresh task. C1.7 reuses one lazily created worker thread, Tokio executor, provider HTTP client/pool, compiled tool registry and telemetry SDK across those tasks. Each task has its own input, workspace, run/trace identities, cancellation token and outcome; no history is implicitly carried forward. Provider/exporter credentials and process configuration are resolved when their shared setup is first constructed and stay fixed until process exit. `initialize` selects protocol version 1. A client that cannot use the returned version must disconnect. `session/new` requires an existing absolute directory and an empty `mcpServers` list. It canonicalizes that directory and returns a generated session ID. Repeated initialization, replacement of an unprompted or active session, unknown/retired session IDs, and concurrent or repeated prompts are rejected. Session data disappears when the process exits; there is no saved conversation or session loading.

Prompts accept text and ACP's baseline resource links. Links become bounded textual references in the task; the adapter does not retrieve their URIs. Images, audio, and embedded context are rejected and their capabilities are false. Prompt text, including rendered links, is limited to 1020 KiB, reserving 4 KiB for the runtime's fixed instructions under its 1 MiB combined input limit. Provider, model, shell capability, deadlines, credential paths, and fixture endpoints come from host configuration, not prompt metadata.

No filesystem/terminal callbacks, permission requests, MCP servers, modes, authentication methods, session management, or other optional ACP behavior is advertised. Shell and [filesystem reads](filesystem.md) are local tools reported through standard ACP tool updates; `--no-shell` and `--no-filesystem` independently remove them from the provider catalog. Unsupported methods return the SDK's `-32601` error; invalid supported requests use `-32602`. Unknown future fields are tolerated by the pinned SDK.

## Events and terminal outcomes

| Native event | ACP projection |
| --- | --- |
| `assistant.text.delta` | `agent_message_chunk` with text content |
| `tool.started` | `tool_call`, kind `execute` for shell or `read`/`edit` for filesystem, raw input, default `pending` status |
| `shell.started` | `tool_call_update`, status `in_progress` |
| `tool.finished` | `tool_call_update`, `completed` or `failed`, typed raw output |
| `run.finished` | `session/prompt` response or error after all pending updates and cleanup |

Nonzero shell exits and unsuccessful tools have ACP status `failed`. The runtime still receives the full tool result and may continue according to its existing policy. Native run/model lifecycle events remain in the trace; the adapter does not invent additional ACP update variants for them.

Negotiate the checkpoint-local `pablo/v1` extension by sending `clientCapabilities._meta["pablo/v1"] = true` in `initialize`. The agent advertises the same capability in `agentCapabilities._meta`. [The extension schema](pablo-acp-v1.schema.json) describes its values. This short namespace is a spike contract; a project-controlled URI and release-stable extension naming remain a deliberate release decision.

For negotiated clients, each `session/update` carries `params._meta["pablo/v1"]` containing the native run/session IDs, inclusive `seq_start`/`seq_end`, timestamp, and OTel trace/span identities. Sequence numbers refer to native publication order; gaps are expected for native events without an ACP projection. Coalescing changes neither the native sequence nor its stored trace. Native contract revision `c2.4` includes filesystem results, configured policy and all-outcome accounting; the extension schema retains historical `c1.2` acceptance.

Terminal metadata adds the exact typed native `outcome`. Completed, cancelled, policy-denied, output-token-limit, and model/tool-call-limit outcomes map to `end_turn`, `cancelled`, `refusal`, `max_tokens`, and `max_turn_requests`. Timeouts, other limits, and failures use JSON-RPC `-32603` with the same terminal metadata at `error.data["pablo/v1"]`; they never pretend to be `end_turn`. Setup errors occur before run admission and have safe errors without invented run IDs. Generic clients receive standard updates, stop reasons and errors, with no Pablo update/terminal metadata. Usage stays unknown (`null`) when the provider omits it.

`session/cancel` affects only the matching active prompt. It cancels the core token, awaits owned process-group cleanup, drains pending updates, awaits that task's completion, then responds. The idle worker remains available to the next session; cancellation does not poison its token. Wrong-session and idle cancellations have no effect. A cancellation received while the final updates drain still returns ACP `cancelled`, as required by the protocol. If the core already emitted its terminal event, its immutable native outcome remains in metadata (possibly `completed`); use ACP `stopReason` for turn control and the typed outcome for the settled computation. This preserves trace truth without rewriting a completed run. The SDK's request-level `$/cancel_request` also propagates to this token while work is active.

## Transport bounds and ownership

The SDK has unbounded internal channels. The adapter bounds what can enter them:

- At most **32 MiB per input/output line**, **64 MiB total input**, and **128 input messages** over the process connection, including all successive tasks. Unterminated frames, oversized data, invalid UTF-8 and batch arrays close the connection. Ordinary malformed JSON gets the SDK's parse error. These are process-lifetime bounds, not rate limits or long-lived-session quotas.
- The runtime publishes through a bounded queue of **8 events**, each checked against **32 MiB serialized size**. Text uses a conservative escape-size bound; an exact counting writer handles other events and large text without allocating serialized JSON. A dedicated, joined runtime thread lets its existing synchronous event sink wait for capacity while the protocol thread continues reading cancellation. This does not add another model loop or change the Rust embedding contract.
- The forwarder keeps one current event and at most one lookahead. It forwards the first text delta of each model operation immediately, then coalesces adjacent text from the same model span at **4 KiB or 16 ms**, preserving its sequence range. Lifecycle/tool events are not dropped or merged. A single provider text delta may already exceed 4 KiB; it remains bounded by the core's output limit.
- Only **one projected notification** enters the SDK at a time. The forwarder waits for the physical stdout write/flush acknowledgement before sending another; SDK enqueue success is not treated as delivery. Bounded incoming traffic also bounds queued request responses. The write counter remains cumulative across tasks, so later sessions retain the same acknowledgement guarantees. Queue saturation produces one safe stderr diagnostic and blocks the worker until capacity returns.
- A stdout write/flush stalled for **30 seconds** closes the connection and cancels owned work. An output error, stdin EOF, SIGINT, or SIGTERM likewise cancels, closes the event queue to wake blocked producers, and awaits the runtime worker and telemetry shutdown. The shared SDK stays alive between tasks; its bounded batch processor exports in the background and its two-second shutdown allowance starts when the process closes. The existing shell cleanup allowance still applies. A disconnected peer cannot be promised a terminal protocol response; a writable native trace records the settled lifecycle. Closing stdout alone is detectable on the next write.

Native JSONL and OTel spans originate in the existing runtime. Optional traces retain the CLI's exclusive file creation, private permissions, byte bound and default redaction. Output to the ACP client includes task results and tool arguments by design. OTel content stays off; C1.5 adds opt-in network export and incoming W3C context inside negotiated `session/prompt` metadata `_meta["pablo/v1"]`. Neither pinned stable-v1 schema defines dedicated trace-context fields. See [telemetry](telemetry.md#incoming-context) for the carrier and Collector proof.

## Offline verification

```sh
npm run typecheck
npm run test:acp
cargo test --locked -p pablo --test acp
```

The TypeScript suite starts the actual executable against local fragmented HTTP/SSE fixtures through `PABLO_FIXTURE_ENDPOINT`. It verifies model/shell/model evidence, standard schemas, negotiated metadata, streaming before completion, cancellation, stdout purity, native trace correlation/redaction, slow output, input bounds, disconnects, host errors and signals. The Rust test additionally uses the official typed client over actual Unix pipes. Fixture mode uses its synthetic credential and never loads the root `.env`.

## Live acceptance

Run the explicit paid C1.4 fixture from the project directory:

```sh
npm run smoke:live:acp
```

This builds the executable and runs the official TypeScript client through initialize/session/prompt against Vercel. The fixture removes inherited gateway keys and `PABLO_FIXTURE_ENDPOINT` from the child environment and passes the absolute root `.env` path to Pablo. Only the executable reads that file, with the usual canonical key/alias support. The harness never reads or sources credentials.

The default profile is `google/gemini-3.8-flash`. To select a previously built binary and optionally override the model:

```sh
node scripts/smoke-live-acp.ts target/release/pablo google/gemini-3.8-flash
```

The fixture creates a random evidence file in a temporary workspace, then requires one successful shell read followed by a second model call whose answer contains that evidence. It verifies streamed text against the typed outcome, reported usage totals against both model events, ordered ACP/native identities, one terminal response, protocol-only stdout, private redacted trace permissions, shell process/group disappearance and clean ACP exit. It deletes the workspace and writes only a redacted summary plus the native trace under ignored `.pablo/traces`. Assertion and SDK errors are never dumped to the terminal.

This small fixture explicitly caps tools/models at 1/2, the run at 90 seconds and the shell at 10 seconds; normal runtime defaults remain unchanged. Its outer process watchdog sends SIGTERM at 100 seconds and SIGKILL at 110 seconds if needed; a forced exit fails acceptance. Missing credentials, absent usage, a failed assertion or an unrun live check cannot pass this gate. There are no automatic live retries.

[C1.4 evidence](project/evidence/c1.4.md) records the observed live result. Deterministic offline tests cover missing/invalid keys, HTTP 401/429/500, streamed errors, incomplete SSE, cancellation and disconnect cleanup; they do not claim those failures were induced at Vercel. C1.5 owns incoming context and Collector export; C1.6 owns both-platform acceptance and performance measurements.

## Upstream references

- [Official Rust SDK and stable-v1 entry points](https://github.com/agentclientprotocol/rust-sdk/tree/726c5030bfaa88cfdac2fb1f71a63abb331ce586).
- [Official TypeScript SDK](https://github.com/agentclientprotocol/typescript-sdk/tree/e6463f444093ed7c5f1cc937c3f32afb5853e906).
- [Reviewed stable-v1 schema](https://github.com/agentclientprotocol/agent-client-protocol/blob/f12f6b39d1a09af407c4f076b073c894f0da43f1/schema/v1/schema.json). The released TypeScript SDK bundles its own pinned snapshot; executable fixtures validate against that installed snapshot and its recorded SHA-256.
- [Prompt lifecycle and cancellation](https://agentclientprotocol.com/protocol/v1/prompt-turn), [extension negotiation](https://agentclientprotocol.com/protocol/v1/extensibility).

The reference client shows each shell command/cwd and terminal status with exit code and output byte counts. Final diagnostics include the exact limit, policy rule, or provider failure code, including outcomes carried in JSON-RPC errors. Tool/model call counts are unlimited by default, shared with the Rust CLI; explicit caps use the forwarded options above. Timeouts and byte/event/transport bounds still apply.

Default run/shell deadlines are one hour/15 minutes. ACP accepts up to 1020 KiB of combined prompt text/resource references, reserving 4 KiB of the core’s 1 MiB input limit for instructions. Larger tool results and terminal answers fit the 32 MiB wire frame, including worst-case JSON escaping of the default 4 MiB model output. Trace capacity is 256 MiB. The short process cleanup and SDK shutdown allowances are independent of task execution deadlines.

C2.3 adds optional `pablo/task-v1: true` alongside `pablo/v1: true` in capability metadata. When both are negotiated, terminal `pablo/v1` metadata contains `task` ([schema](pablo-task.schema.json)) in place of `outcome`; its outcome comes from the same native terminal event. This avoids duplicating potentially 24 MiB of escaped output in the 32 MiB transport frame. Peers negotiating only `pablo/v1` retain `outcome`; generic peers retain standard stop reasons and safe errors. The reference client's `taskOf` validates canonical decimal accounting through u64 and exposes strings for exact `BigInt` conversion. Legacy numeric usage remains for compatibility; the task accounting strings are the exact representation above JavaScript's safe integer range. The envelope version is `c2.3`, independently of native event revision.


## C2 extension compatibility review

| Name / capability | Schema and version | Bound and visibility | Generic fallback |
| --- | --- | --- | --- |
| `pablo/v1` | Local Draft 2020-12 metadata schema; native `c2.4`, historical revisions accepted | Correlation contains IDs/timestamps; native outcome contains task output. All share the 32 MiB frame limit. Incoming W3C fields are limited to 512 characters each and exclude baggage. | Standard ACP updates, stop reasons and safe errors; no project metadata and no remote parent extraction. |
| `pablo/task-v1` (requires `pablo/v1`) | Shared task schema `c2.3`; exact u64 accounting strings | Terminal `task` replaces `outcome`, avoiding output duplication; six-times-output plus 8 KiB task bound within the transport frame. Output is client-visible content; counters/IDs are metadata. Native content remains opt-in; OTel stays content-free. | Existing `pablo/v1` peers retain the legacy `outcome`; generic peers retain standard ACP. |

There are no custom methods or alternate lifecycle. The pinned ACP SDK/schema remains authoritative; both legacy and task-negotiated shapes, generic fallback and maximum escaping-heavy output have executable tests. Decimal schemas enforce canonical syntax; the core and reference client additionally enforce the u64 numerical maximum. JSON Schema `format` is annotation-only in the fixture validator, and no network schema resolution is enabled.

The existing short project-prefixed names are retained for development compatibility. No project-controlled DNS URI is claimed. Final namespace ownership/release stability and `pablo doctor` remain explicit release-hardening decisions; C2 acceptance does not silently ratify them or publish a release.


### Negotiated route attempts

C3.12 adds `pablo/model-route-v1` (requiring `pablo/v1`). Opted-in clients receive
`_pablo/model_attempt` extension notifications for routed model start/finish events,
with `sessionId`, native `type` and `pablo/v1` correlation containing `model_route`.
The terminal correlation carries the latest selection, including a selected entry
blocked before dispatch. Standard updates and task schema `c2.3` are unchanged.
See [the attempt contract](project/contracts/c3-model-routes.md#f03-attempt-observability--model-route-v1)
and the `model_route` definition in the extension schema. Notifications share the
existing bounded queue and acknowledged physical-write path. Non-opted clients
receive no new notification method or route metadata.

### Negotiated compaction

C3.12a adds `pablo/compaction-v1`, requiring `pablo/v1`. Opted-in peers receive
`_pablo/compaction` notifications for `context.compaction.started` and
`context.compaction.finished`, with session/type/correlation and a bounded
`compaction-v1` record. The terminal correlation includes the latest record.
Notifications share physical-write acknowledgment with ordinary updates.

Summary text appears only on successful completion with explicit content capture;
otherwise `summary` is null and `summary_bytes` describes its size. Capture requires
a trace destination, such as `{base="workspace",path="{session_id}.jsonl"}`.
Private provider continuation is never exposed. Summary deltas do not become agent
answer chunks. Peers without the capability keep existing shapes; each new session
has fresh history and compaction allowance. The task schema remains `c2.3`.
See [the contract](project/contracts/c3-compaction.md) and the reference client's
`onCompaction` callback.
