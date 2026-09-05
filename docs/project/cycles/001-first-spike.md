# Cycle C1: a bounded agent, built in small checkpoints

## Objective

Implement the focused spike from [the design brief](../../context.md), section 37.1. A tiny TypeScript client starts `pablo acp --stdio`, sends a bounded task, observes streamed model and shell activity, receives a typed outcome, and can inspect correlated native/OTel traces. Prove cancellation and record release-build measurements.

Use Vercel AI Gateway first and complete one checkpoint per implementation session. This cycle precedes alpha.1. [State](../state.json) is the source of checkpoint progress; [the log](../log.jsonl) records verification and handoffs.

## Boundaries

Include project memory, a Rust core and executable, an offline provider fixture, one live gateway profile, bounded shell execution, ACP v1 stdio, a TypeScript reference client, typed outcomes, cancellation, JSONL events, native OTel spans, a real Collector proof, and measurements.

Defer filesystem tools, other providers, MCP, Skills, subagents, A2A, TUI, durable runtime sessions, general structured-output repair, full cost-ledger enforcement, retry/fallback orchestration, and release publishing. They remain in [the backlog](../backlog.md).

## C1.0: Project memory

Initialize local Git. Create the brain, structured state, append-only work log, cycle specification, backlog, working instructions, and dependency-free Node helper.

Acceptance:

- `status` reports progress, blockers, and the next eligible checkpoint.
- `context` assembles the brain, state summary, selected checkpoint specification, and three latest log entries without loading the full design brief or entire log into the output.
- `check` validates records, unique IDs, statuses, dependencies/cycles, active and next pointers, file references, completion evidence, context hash, and the 200-line brain limit.
- Tests reject representative corrupt records and exercise the real helper commands, including invocation from another directory and their read-only behavior.
- `.env` and generated runtime/build data are ignored; the context brief is preserved.
- Check results and the C1.1 handoff are recorded, leaving C1.1 ready without starting it.

## C1.1: Small runtime foundation

Create two Rust crates: `pablo-core` and the `pablo` executable. Define only `RunSpec`, `RunLimits`, `RunEvent`, `RunOutcome`, `Provider`, and `Tool` shapes needed by this spike. Introduce a scripted streamed provider, native OTel lifecycle spans, and correlated JSONL events. Pin toolchain and introduced dependencies in checked-in configuration/lockfiles.

Acceptance: a Rust integration test streams a text response through the core, produces ordered events and exactly one terminal outcome, and verifies native/OTel identity correlation. Record executable build/check commands. Network export is not required at this checkpoint.

## C1.2: Bounded shell loop

Add `shell.run`, Draft 2020-12 validation for its built-in argument schema, sequential tool dispatch, run/tool limits, cancellation, and process-group cleanup.

Acceptance: a fake provider requests a shell read of a temporary evidence file, validates the actual tool result on the second model request, and completes. Cover invalid arguments, nonzero exit, oversized output, timeout, cancellation races, and no surviving owned processes after cleanup.

## C1.2a: End-user run preview

Added at the user's request to test the agent as an end user immediately after C1.2. Bring forward the narrow live-provider and terminal entry point needed for a real task, using the existing Rust lifecycle. This explicitly permits a small credentialed smoke run before C1.4.

Implement `pablo run "TASK"` with an explicit workspace/model override, streamed answers and shell activity, Ctrl-C cleanup, private optional traces, and a Vercel HTTP/SSE adapter. Read the gateway key privately from the invoking directory's ignored `.env` or process environment, without installing file values in the environment. Keep each invocation a single bounded task.

Acceptance: the actual executable completes a local HTTP/SSE model/shell/model fixture, rejects malformed or oversized streams with safe errors, cancels owned shell work, and completes a small live read of synthetic evidence using the supplied gateway key. Document the exact end-user command and remaining limits. ACP, persistent chat, and formal live ACP acceptance remain C1.3–C1.4.

## C1.3: ACP process boundary

Use official Rust and TypeScript ACP SDKs and pin stable-v1 schema/SDK revisions. Implement `initialize`, `session/new`, `session/prompt`, `session/update`, and `session/cancel`. Add the tiny TypeScript reference client. Exercise the actual executable against local HTTP/SSE provider fixtures; use a trusted host-configured endpoint override for fixtures so the executable follows its ordinary provider path.

Acceptance: the client initializes, creates an in-memory session, streams ordered message/tool updates, obtains a typed outcome, and cancels a run after cleanup. Cover unsupported requests, slow consumers, client disconnect cleanup, and stdout containing protocol traffic only. Do not advertise unimplemented optional capabilities.

## C1.4: Live Vercel path

Complete direct Vercel HTTP/SSE transport through the same provider boundary. Explicitly load the user's `AI_GATEWAY_API_KEY` from the ignored root `.env` for the live fixture. Use the user-selected `google/gemini-3.8-flash` default profile, with an explicit override available. D014 replaces the original GPT-4.1 mini smoke profile. Keep credentials outside serialized `RunSpec`, native trace content, tool environment, and project records.

Acceptance: a live model uses the shell to read temporary evidence and returns an answer containing that evidence. Record the model/profile, usage, safe errors, and a redacted evidence summary referencing the local runtime trace. A missing key or unrun live fixture leaves the gate pending.

## C1.5: Collector proof

Add OTLP/HTTP Protobuf export and a reproducible pinned local Collector fixture. Use the lifecycle instrumentation introduced in C1.1, without translating a second event stream into spans after the fact.

Acceptance: a real Collector receives correctly parented run/model/tool spans matching native JSONL IDs. Incoming W3C context propagates, OTel content is absent, credentials are absent from all serialized surfaces, and Collector failure does not alter the model/tool outcome or block shutdown indefinitely.

## C1.6: Cycle acceptance

Run all focused fixtures on macOS and Linux. Produce a local cycle report with acceptance evidence, measurements, limitations, and the next cycle proposal. Run the credentialed fixture explicitly.

Acceptance: live Vercel proof, real Collector proof, platform checks, and complete project records all exist. Record stripped binary size, startup time, idle RSS, event latency, trace overhead, and ACP stdio overhead relative to the in-process fixture. Include platform, tool versions, sample counts, and method. Establish measured baselines before imposing performance gates. Do not label the focused spike a complete alpha.1 or 0.1 release.

## Runtime contracts and defaults

- Core spec: input, workspace, provider/model configuration, limits, and trace settings. Supply credentials separately. Terminal outcomes distinguish completion, cancellation, timeout, limit exhaustion, policy denial, and failure with provider-delivery certainty. Events carry sequence, timestamp, run/session identity, and OTel correlation.
- Provider: streamed text, tool-call assembly, results, finish reasons, reported usage/cache counters, and a stable instruction/tool prefix across steps. Sequential tools; no automatic retries or fallback. Missing usage is unknown, not zero.
- Shell: command, explicit working directory, environment additions, timeout, and output bound. Return separate stdout/stderr, exit status, and truncation metadata. Noninteractive `/bin/sh`, a workspace-contained working directory, and a minimal environment excluding provider/exporter secrets. Static policy is not containment; hosts own isolation.
- Tool schemas: Draft 2020-12 with remote-reference retrieval disabled. Reject unsupported behavior explicitly. Map the internal `shell.run` identity to a provider-valid function name in the adapter rather than weakening the internal identity.
- ACP: standard lifecycle, message/tool updates, and stop reasons. Negotiate `pablo/v1` metadata for richer outcomes and correlation; follow the pinned ACP trace-context fields. Complete cancellation cleanup and pending updates before the prompt response. Diagnostics go to stderr.
- Current defaults (D015/D016): unlimited model/tool call counts with optional explicit caps, one active shell process, one hour per run, 15 minutes per shell call, 65,536 output tokens per model request, 1 MiB input/tool arguments, 8 MiB tool results, 4 MiB model output, and 32 MiB context. Optional native traces allow 256 MiB. Bound protocol frames, stream buffers, and event queues. These are configurable spike defaults; full cost-ledger enforcement is later release work.
- Telemetry: create OTel spans and native events at the same lifecycle transitions. Disable network export by default and keep OTel content disabled. Explicit native content capture is independent; fixtures enable it for synthetic evidence. Bound JSONL size, reserve terminal-record space, and stop cleanly on trace-capacity exhaustion. Exporter errors remain diagnostic with a two-second shutdown flush deadline.

## Verification strategy

Run relevant checks at each checkpoint; run the full suite at C1.6. Use offline fixtures in ordinary CI. Cover fragmented SSE/tool arguments, malformed responses, provider disconnects, missing usage, stable prompt prefixes, exactly one outcome, shell/process behavior, ACP ordering and backpressure, context propagation, trace/redaction bounds, and exporter outage.

Project-memory checks validate record consistency, not whether runtime acceptance was substantively met. Record actual results with evidence and keep incomplete gates visible.

## References

- [ACP prompt lifecycle](https://agentclientprotocol.com/protocol/v1/prompt-turn)
- [ACP extension rules](https://agentclientprotocol.com/protocol/v1/extensibility)
- [Official ACP Rust SDK](https://github.com/agentclientprotocol/rust-sdk)
- [Vercel REST API](https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-chat-completions/rest-api)
- [Current Vercel model identifier](https://vercel.com/ai-gateway/models/gemini-3.8-flash)

These references were inspected during planning. Pin exact protocol and SDK revisions when introduced; record any necessary compatibility decision in the brain.
