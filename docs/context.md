# `pablo`: exhaustive project context and architecture brief

Status: pre-implementation design context  
Prepared: 2026-09-03  
Revised: 2026-09-04 (tool named `pablo`; focused version 0.1 cut line and staged 0.x architecture preserved)  
Proposed license: Apache-2.0  
Proposed implementation language: Rust  
Proposed executable: `pablo`

This document is the complete starting context for a new open-source agent runtime. It records the product intent, settled decisions, evidence from existing applications, proposed architecture, version 0.1 scope, developer experience, operating model, risks, and unresolved decisions. No implementation exists in this directory yet.

### How to read this document

Nobody needs all of it at once. Entry points by role:

- **Deciding or implementing version 0.1:** section 29.1 is the authoritative release contract. If another section describes more behavior, it is architecture or 0.x backlog unless section 29.1 includes it.
- **Evaluating or embedding `pablo`:** sections 1–3, 11, 12, 27, 28, and 31.
- **Implementing the core:** sections 9, 10, 12–25, 28, and 32.
- **Protocol, telemetry, and add-on work:** sections 11.4–11.8, 21, 28, and 32.4.
- **Deciding what ships when:** sections 2, 29–31, and 35–37.

The document intentionally preserves the full design. Scope is compressed by staging capabilities, not by deleting their rationale. Section 29 separates the first releasable vertical slice from the preserved product target.

## 1. Executive summary

`pablo` should be a minimal, powerful, extensible agent runtime for applications that need agentic workflows outside software development. It takes inspiration from the small native design of [`fx`](https://github.com/vercel-labs/fx), but it will be implemented in Rust and designed primarily as infrastructure that other products embed.

The core thesis is:

> Build a tiny headless agent runtime whose reference interfaces happen to be an excellent CLI and TUI.

The runtime should work well immediately after installation, while giving application developers complete control over models, prompts, tools, shell execution, MCP servers, skills, context, budgets, event presentation, persistence, and observability.

In this document, MCP is the Model Context Protocol, ACP is the Agent Client Protocol, A2A is the Agent2Agent Protocol, and OTel is OpenTelemetry.

The architecture deliberately uses a small number of powerful primitives:

1. **Shell** is the universal execution primitive.
2. **MCP** is the live capability and data integration boundary.
3. **Skills** are portable procedural knowledge, scripts, references, and assets.
4. **Host tools** are the typed boundary for application-specific reads and consequential actions.
5. **Subagents** are independently addressable child runs and sessions for parallel or specialized work.
6. **ACP** is the native client-to-agent and supervisor-to-local-child session protocol.
7. **A2A** is the native agent-to-remote-agent task and artifact protocol.
8. **Official add-ons** project that one runtime into additional provider, application, and API protocols without adding alternate model loops.

Minimal must mean few coherent primitives, not a weak agent.

## 2. Settled product decisions

These decisions were made in the originating discussion and are the baseline unless deliberately revisited. Each points to the section that carries the full design, and that section is the only place the design is stated.

**Product and packaging**

- An open-source agent runtime plus CLI/TUI library, built for developers to embed in their own products (section 3).
- Aimed at non-coding workflows while keeping the power of coding agents.
- Inspired by the minimal, native, embeddable approach of `fx`, implemented in Rust rather than Zig (sections 4 and 5).
- Lightweight, fast to start, easy to distribute, one native executable (section 28).
- Headless and language-neutral first; the TUI is a reference client and operator interface, not the only interface (section 26).
- The tool and its canonical executable are named `pablo` (section 11.9).
- One usable core plus official optional add-ons. The core includes JSON Schema Draft 2020-12, shared JSON-RPC 2.0 machinery, Open Responses, ACP v1, A2A 1.0, MCP, skills, shell and filesystem tools, native OTel, and the initial gateways. Projections a deployment does not use add no dependency, binary, attack surface, configuration, or startup cost (section 10).
- Optional means official but separable: add-ons are pinned, tested, documented, and released with the core, depend inward on public contracts, and never own a second loop, session store, tool dispatcher, scheduler, permission model, or trace source (section 11.4).
- Vercel AI Gateway and OpenRouter are the first gateways; Open Responses is a native third provider path (section 19).
- AG-UI over HTTP and SSE, the Vercel AI SDK `UIMessage` stream, and OpenAPI 3.1 import are official add-ons (section 11.7).
- OttoV3's bounded analysis is the first dogfood target; Graphline is the second proving ground for resumable sessions, streamed activity, artifacts, and application-owned approvals (sections 6 and 7).

**Execution and policy**

- Shell execution is enabled and first-class (section 14).
- Version 0.1 runs in `yolo` mode: no interactive approval, and every statically allowed operation executes immediately. Static denies still apply (section 14.3).
- Developers may narrow shell commands, MCP servers and tools, filesystem scope, and other capabilities in configuration (section 20.3).
- Command filters are policy and ergonomics, not a security boundary; containment belongs to sandboxes, containers, VMs, OS permissions, and network policy (section 14.4).
- MCP and Agent Skills are first-class features, not later plugins (sections 15 and 16).

**Agents and protocols**

- Subagents are first-class in version 0.1. Each has its own identity, context, model loop, lifecycle, events, usage, spans, and output. Durable child sessions are a preserved post-0.1 extension of the same contract (section 17).
- Children inherit the host and parent authority ceiling and may only narrow it (section 17.6).
- Bounded one-task children ship first; named persistent child sessions remain part of the product design but are not a version 0.1 gate (section 17.12).
- Chains compose the same child primitives. Ownership is a strict tree, execution may be an observable dependency graph, and handoffs pass typed results and references rather than transcripts or mutable workspaces (section 17.18).
- Stable ACP v1 is the native process and local-subagent protocol: the supervisor is an ACP client, every local agent is an ACP agent, in-process children use typed in-memory dispatch, and subprocess children use stdio (sections 11.5 and 11.6).
- A2A 1.0 is native for remote agents; a remote subagent is a supervised proxy whose internals stay outside local authority claims (section 11.6).
- `pablo` adds only capability-negotiated ACP and A2A extensions for invariants absent upstream (section 11.5). Draft ACP v2 is outside the 0.1 contract.
- JSON Schema Draft 2020-12 is the canonical schema dialect for tools, outputs, handoffs, and add-on boundaries (section 11.5).

**Performance and observability**

- Prompt-cache stability is a runtime responsibility from version 0.1. Advanced breakpoint allocation, TTL policy, affinity, and cache-ratio targets remain staged optimization work (section 28.4).
- Observability, visibility, and runtime tuning are central differentiators (section 21).
- OpenTelemetry is the native operational telemetry model, created where work occurs, propagated as W3C Trace Context, following the pinned GenAI and MCP conventions, and exported through OTLP. No proprietary telemetry abstraction is translated into OTel later (section 21.5).
- The native `pablo` trace is the lossless replay and audit record, emitted from the same transitions and sharing OTel identities and timestamps (section 21.12).
- Visibility means inspectable state, actions, evidence, costs, and decisions, never hidden chain-of-thought (section 21.3).

## 3. Product problem

Many products currently embed Claude Code, Codex, Cursor CLI, or another coding agent inside E2B, Daytona, Vercel Sandbox, or a similar environment to perform work that is not fundamentally software development. Examples include:

- Reading and synthesizing documents.
- Producing structured analysis from files.
- Drafting reports and artifacts.
- Operating business applications through APIs and CLIs.
- Running media or document-processing utilities.
- Researching a question with files, web access, and domain tools.
- Maintaining an evidence-backed conversation over a durable workspace.
- Proposing business actions for human approval.
- Delegating independent or specialized work to parallel child agents and combining their results.

Coding agents are used because they already provide the important systems behavior: a model loop, shell access, filesystem access, streaming, tool calling, context management, session resumption, and useful failure recovery. Their coding-specific prompting and product assumptions are incidental.

The opportunity is not merely to expose a model API. The product to replace is the complete harness around the model:

- Tool loop.
- Context assembly.
- Streaming and cancellation.
- Filesystem and process behavior.
- Model and gateway normalization.
- Retry and recovery semantics.
- Session state.
- Child-agent supervision, messaging, concurrency, and budget accounting.
- Extension discovery.
- Output validation.
- Tracing and debugging.
- A usable interactive interface.

`pablo` should provide that harness without assuming the task is code editing, without forcing a large application framework, and without taking ownership of the embedding product’s durable business state.

## 4. Why Rust

Rust is a strong fit for this project because it offers:

- A native executable with no language runtime required in the target sandbox.
- Straightforward cross-compilation and release artifact distribution.
- Strong control over memory, concurrency, process lifetime, and streaming I/O.
- Mature async HTTP, terminal, serialization, and tracing ecosystems.
- More approachable contributor ergonomics and library availability than Zig for this particular protocol-heavy project.
- A viable official MCP SDK.
- The ability to expose both a Rust library and a standalone process.

Rust will not create meaningful application-level latency relative to Zig in these use cases. Model calls, tool execution, remote sandbox creation, document processing, and network I/O operate on millisecond-to-minute timescales. Differences in local instruction execution are immaterial. Startup time and binary size should still be measured and protected, but they are not reasons to avoid Rust.

The major interoperability limitation is that OttoV3 is TypeScript and Graphline is Python. A Rust crate or FFI library alone would be inconvenient for both. Therefore, stable ACP v1 plus narrowly negotiated `pablo` extensions should be the cross-language process boundary, with a Rust crate available as an additional integration option rather than the only one.

## 5. Inspiration and evidence from `fx`

### 5.1 Inspected baseline

- Repository: `https://github.com/vercel-labs/fx.git`
- Local checkout: `/Users/caleb/Projects/fx`
- Branch: `main`
- Commit: `4351daf29ccf510563bcc63001ffc65b0c7bbe0f`
- Working tree at inspection: clean and tracking `origin/main`

Important source locations:

- `/Users/caleb/Projects/fx/README.md`
- `/Users/caleb/Projects/fx/src/main.zig`
- `/Users/caleb/Projects/fx/src/core/agent/`
- `/Users/caleb/Projects/fx/src/core/tooling/`
- `/Users/caleb/Projects/fx/src/core/execution/`
- `/Users/caleb/Projects/fx/src/core/session/`
- `/Users/caleb/Projects/fx/src/core/mcp/`
- `/Users/caleb/Projects/fx/src/core/skills/`
- `/Users/caleb/Projects/fx/src/core/permissions/`
- `/Users/caleb/Projects/fx/src/gateway/`
- `/Users/caleb/Projects/fx/src/ui/`
- `/Users/caleb/Projects/fx/src/builtins/tools.zig`

### 5.2 What `fx` demonstrates

`fx` is a small native coding-agent harness written in Zig. Its public positioning emphasizes a tiny, open, embeddable, model-agnostic agent with a Unix-shell-like interface rather than a heavy IDE-style TUI. At the inspected baseline it targets a 7.8 MiB production binary, supports native and experimental WebAssembly embedding, and exposes an ACP server.

Architecturally, `fx` separates:

- Composition in `main.zig`.
- Agent contracts and orchestration under `src/core/agent`.
- Generic tool schemas, projection, admission, and dispatch under `src/core/tooling`.
- Built-in tool implementations under `src/tools`.
- Provider transport under `src/gateway`.
- Terminal rendering and input under `src/ui`.
- Sessions and persistence under `src/core/session`.
- MCP protocol and lifecycle under `src/core/mcp`.
- Skill discovery and invocation under `src/core/skills`.
- Permissions under `src/core/permissions`.

Its provider boundary is typed and provider-neutral. Requests carry messages, tool selection, response format, model options, cancellation, deadlines, stream sinks, delivery certainty, retry ownership, and usage evidence. Concrete providers own request serialization, authentication, HTTP, and stream reduction.

Its built-in tool surface includes filesystem operations, a durable shell tool, web access, skill discovery and loading, MCP capability retrieval, result retrieval, subagents, questions, and vision. The durable shell supports run, interact, and stop operations; TTY and non-TTY execution; long-running session handles; bounded output; and explicit current directories.

Its subagent design is especially relevant: a model-facing tool can run one temporary child or create and continue a named persistent child. Children retain the trusted base prompt, receive only a child-specific instruction overlay, own ordinary session state, and remain associated with the parent rather than becoming hidden global state.

Its MCP implementation demonstrates the real complexity hidden behind “support MCP”:

- Protocol negotiation.
- Stdio and HTTP transports.
- Legacy compatibility.
- Startup and operation timeouts.
- Authentication.
- Workspace trust.
- Dynamic tool selection.
- Tools, resources, prompts, completions, progress, elicitation, and notifications.
- Result validation, bounding, truncation, and secret masking.
- Server restarts and health.

Its skill implementation demonstrates progressive disclosure and defensive discovery:

- Scan allowed roots for skill directories.
- Parse bounded `SKILL.md` metadata.
- Advertise only names and descriptions initially.
- Load complete instructions for an explicit or relevant match.
- Read referenced resources on demand.
- Keep resource paths within the authorized skill root.
- Record diagnostics for invalid, missing, linked, unreadable, and oversized candidates.
- Support skills from multiple compatible clients and scopes.

### 5.3 What `pablo` should borrow from `fx`

- A native single-binary experience.
- A small composition root with feature logic in owned modules.
- Typed contracts at provider, tool, session, and output boundaries.
- Provider-neutral orchestration with provider-specific wire adapters.
- A durable shell abstraction rather than one blocking subprocess call.
- Bounded tool results and retained overflow handles.
- Cancellation as a real runtime contract.
- Persistent sessions and explicit compaction.
- First-class child agents built from the same run/session primitive as the root agent.
- Bounded parent-child messaging rather than automatically copying entire transcripts between agents.
- Skills and MCP as discovery layers over the same internal capability registry.
- A useful noninteractive JSON mode alongside the TUI.
- Diagnostic traces that users can inspect and share deliberately.
- A headless embedding surface.
- Strict separation between product state and terminal presentation.
- Startup and binary-size budgets.
- Real PTY end-to-end testing rather than relying only on unit tests.

### 5.4 What `pablo` should not copy directly

- Coding-specific prompts, terminology, Git assumptions, or development-command classification.
- A large interactive permission reviewer in version 0.1.
- Every legacy transport or compatibility path from the beginning.
- Product behavior tied to one terminal rendering model.
- A requirement that all hosts link the native library directly.
- A broad collection of bespoke integrations that shell and MCP already cover.

`fx` is evidence that the minimal design can remain powerful. It is also evidence that MCP, permissions, terminal processes, and session lifecycle can expand quickly. `pablo` should preserve the clean contracts while choosing a narrower first release.

## 6. Evidence from OttoV3

### 6.1 Inspected baseline

- Repository: `https://github.com/calebjohn24/ottov3.git`
- Local checkout: `/Users/caleb/Projects/otto/ottoV3`
- Branch: `feature/omit-coaching-outcome-labels`
- Commit: `ff40ad18327a6b3d7ee0b748f0a5e37f27329df9`
- The checkout contained unrelated untracked user files. They were not modified.

Important source locations:

- `/Users/caleb/Projects/otto/ottoV3/src/be/analysis/runner.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/analysis/settings.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/analysis/runner.test.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/recordings/markdown-coaching.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/recordings/processor.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/recordings/repository.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/practice/workflows.ts`
- `/Users/caleb/Projects/otto/ottoV3/src/be/practice/simulation-workflows.ts`
- `/Users/caleb/Projects/otto/ottoV3/scripts/analysis/smoke-e2b.ts`

### 6.2 Current Otto agent shape

Otto has a reusable analysis runner built around Cursor CLI inside an E2B sandbox. The runner is already close to a generic bounded-agent contract.

For each attempt it:

1. Creates a secure E2B sandbox with a bounded lease.
2. Verifies the expected knowledge checksum, skill checksum, Cursor CLI version, and build.
3. Authenticates before writing task content.
4. Verifies privacy mode.
5. Writes task instructions and an `AGENTS.md` overlay that treats evidence as untrusted and constrains tool behavior.
6. Mounts input files and immutable knowledge/skill/schema directories.
7. Executes Cursor in print mode using streamed JSON.
8. Parses streamed messages and extracts the terminal result.
9. Validates Markdown or JSON output.
10. If validation fails, resumes the same session with structured repair feedback for a bounded number of cycles.
11. Records attempts, fallback state, model, reasoning level, usage, duration, session ID, request ID, sandbox ID, engine version, prompt/template versions, and knowledge fingerprints.
12. Emits safe content-free progress events to the product.
13. Emits a richer private debug artifact for investigation.
14. Always kills the sandbox.

The runner has explicit safe failure categories such as credential, privacy, template, schema, provider, sandbox, and stalled failures. Retryability is classified deliberately. New sandboxes are used for outer attempts. Requested models and default primary/fallback routes have bounded attempt policies and backoff.

Product workflows sit above the runner:

- Coaching supplies domain-specific instructions, files, a report marker, and a validator that checks the generated Markdown against the underlying transcript.
- Practice workflows use durable step storage, resume completed work, renew leases on agent events, persist typed output, and map safe failures.
- Recording processing maps agent events into progress and lease renewal, persists private artifacts per attempt, and transactionally writes final reports with full provenance.

### 6.3 What Otto requires from `pablo`

Otto’s ideal integration should look conceptually like this:

1. Otto provisions E2B and owns its lease.
2. Otto copies or installs the `pablo` binary into the sandbox image.
3. Otto mounts task inputs, knowledge, skills, and schema files.
4. Otto starts `pablo` in headless protocol mode.
5. `pablo` owns the model/tool loop, shell, file tools, context, stream normalization, and local session state.
6. Otto consumes versioned events and renews its workflow lease.
7. `pablo` performs syntax and JSON Schema validation internally.
8. Otto may perform domain-specific semantic validation.
9. A failed semantic validation can be returned to the same `pablo` session as repair feedback.
10. Otto persists the final result and selected trace material.
11. Otto destroys the sandbox.

This use case requires:

- One-shot runs.
- A resumable session token even for bounded jobs.
- File-oriented inputs as first-class values.
- Structured output and Markdown output contracts.
- Same-session repair.
- Typed terminal outcomes distinct from streamed narration.
- Clear retry ownership so the gateway, agent, and workflow do not multiply retries.
- Requested and resolved provider/model identity.
- Content-free public events and content-bearing private traces.
- Host-selectable trace retention.
- Secret scrubbing.
- Deterministic version and content fingerprints.
- An end-to-end smoke path using a real sandbox and real model.
- Optional bounded delegation for independent evidence review, extraction, validation, or comparison passes without changing Otto’s durable workflow ownership.

### 6.4 Why Otto is the first dogfood target

Otto is the lower-risk first integration because its unit of work is bounded and its product boundary is already clean. It can validate the native model loop, shell behavior, skills, files, structured output, repair, tracing, and gateway support without first solving durable multi-user conversation infrastructure.

The target is not merely “the binary can answer a prompt.” The target is that one representative coaching task can replace Cursor CLI while preserving or improving:

- Output quality.
- Schema and domain validity.
- Repair success.
- Usage and cost observability.
- Privacy behavior.
- Debuggability.
- Total runtime reliability.

## 7. Evidence from Graphline

### 7.1 Inspected baseline

Graphline’s repository instructions declare GitHub remote state authoritative. The local checkout was 403 commits behind `origin/main`, so inspection used remote Git objects rather than treating local files as current.

- Repository: `https://github.com/Deirfgeiz/Graphline.git`
- Authoritative baseline: `origin/main`
- Commit: `d5942127cd38a59b306db59708e7e6ee0632c6e9`
- Canonical roadmap inspected: `origin/main:docs/ROADMAP.md`
- Agent guide inspected: `origin/main:AGENTS.md`
- Relevant recent documentation branches inspected:
  - `origin/docs/design-partner-first-run` at `5ac34feec2c5ee7b502f575b2a8445e4bb7d3ef6`
  - `origin/docs/continual-learning-playbook` at `0599aee215699daac94fe625b19061dbddc94df1`
  - `origin/docs/voc-heidi-griffen-2026-08-24` at `bdcc00c82fe45c1356b09a7427745149ea8ec0be`

Important authoritative source locations at the baseline:

- `apps/api/app/services/sandbox_session.py`
- `apps/api/app/services/claude_code_stream.py`
- `apps/api/app/integrations/e2b.py`
- `apps/api/app/routers/chat.py`
- `apps/web/components/chat/useLoanChat.ts`
- `apps/web/components/chat/tool-progress.ts`
- `apps/web/components/chat/ToolCallCard.tsx`
- `apps/web/lib/types.ts`
- `docs/decisions/2026-08-05-conversational-action-layer.md`
- `docs/decisions/2026-08-05-one-needs-you-model.md`
- `docs/research/continual-learning-karanam-trajectory.md` on its documentation branch

### 7.2 Current Graphline agent shape

Graphline uses one warm E2B sandbox per chat thread. Each user turn starts a fresh `claude -p` subprocess and resumes the persisted Claude session. The sandbox filesystem, installed packages, and session state survive between turns.

The sandbox session manager owns:

- In-process per-thread locks.
- Database advisory locks across application replicas.
- Per-tenant warm-sandbox capacity.
- Reclaiming the oldest idle sandbox.
- Creating, reconnecting, sleeping, waking, reaping, and closing sandboxes.
- Fetching the current loan and exact version snapshot.
- Installing Claude Code and required utilities.
- Mounting the system prompt, loan snapshot, skills, user uploads, and OCR material.
- Starting and resuming Claude turns.
- Streaming stdout through a pure translator.
- Discovering rotated provider session IDs.
- Persisting user and assistant messages.
- Tracking tokens, cost, and activity timestamps.
- Collecting newly created artifacts and uploading them to R2.
- Writing artifact and audit records.

The Claude stream translator is intentionally pure. It converts provider-specific stream JSON into a Graphline event model containing text deltas, tool calls, tool results, finishes, artifacts, and errors. It preserves multiple assistant/tool rounds and tolerates arbitrary byte chunks and malformed lines.

The web application does not use a stock AI SDK chat hook because the backend emits its own SSE protocol. It manually parses and reduces events, handles optimistic messages, supports aborting the client request, and replays messages from a sequence cursor after disconnection. Tool-progress presentation heuristically translates raw Claude tool names and arguments into outcome-oriented language and hides technical detail behind an expandable control.

### 7.3 Graphline’s action boundary

Graphline’s product is not meant to be an unconstrained chatbot. It is a cited loan-definition and human-approved action layer.

The durable conversational action model is:

1. A work item exists in canonical application state.
2. The assistant proposes one or more typed actions.
3. The application persists an immutable action request with its kind, version, inputs, consequences, preconditions, conflict keys, and context.
4. A human reviews and approves the exact action.
5. A typed existing application writer revalidates preconditions immediately before mutation.
6. The action and its audit record are committed transactionally.

The LLM does not directly write canonical loan state, compute authoritative dollar amounts, decide legal status, or autonomously send consequential external communications. The model extracts, classifies, summarizes, explains, drafts, and orchestrates. Deterministic code computes and commits authoritative state.

This distinction must remain possible in `pablo`. Generic tool calling is insufficient if all tools are modeled as equivalent immediate side effects.

### 7.4 What Graphline requires from `pablo`

Graphline should retain ownership of Postgres state, tenant authorization, database locks, warm-sandbox limits, artifacts, audit, and action approvals. `pablo` should replace the Claude subprocess and bespoke stream translator, not the whole Graphline session manager.

The use case requires:

- Durable resumable sessions.
- Multiple turns over a stable workspace.
- A serializable session reference independent of one provider’s session ID.
- Exact binding to the host’s current context version.
- Streaming text, tools, results, progress, artifacts, usage, and errors.
- Stable sequence IDs and replay after disconnect.
- Clear distinction between client disconnect, turn cancellation, process cancellation, and session closure.
- Host-defined typed tools.
- Host-controlled action proposals and approval pauses.
- Explicit artifact publication.
- User-facing semantic tool labels supplied by capability metadata rather than inferred from raw shell arguments.
- Trace linkage from user-visible output back through model and tool calls.
- File inputs, skills, and shell access within the sandbox.
- Bounded outputs and cost controls.
- Specialized child agents for document extraction, covenant/status analysis, servicing-data checks, artifact drafting, or review, with every child remaining inside the parent’s tenant, context-version, tool, and budget ceiling.

### 7.5 Continual-learning implications

Graphline’s recent continual-learning research branch contributes several principles that are directly relevant to `pablo`:

- Capture the full execution tree: every model call, subagent, tool call, tool result, prompt version, and tool version.
- Link every user-visible output backward to the exact trace that produced it.
- Treat corrections, retries, edits, undos, abandonments, and the final accepted diff as higher-value feedback than a generic thumbs-up or thumbs-down.
- Separate “something went wrong” from “corrected to right.” Only the latter contains a positive target.
- Snapshot volatile task inputs at trace time if future replay matters.
- Run eval cases through the real production prompt, tools, and loop.
- Expose primitives rather than rigid workflows, while keeping deterministic invariants outside the model.
- Return resulting state from tools, not merely “done.”
- Preserve model portability and evaluate model changes through real workloads.

These ideas align with the proposed observability and replay design and should influence the trace schema from the beginning.

## 8. Product boundary

### 8.1 What the runtime owns

`pablo` should own:

- Provider-neutral conversation and tool-loop orchestration.
- Model request construction and streamed response reduction.
- Provider capability resolution.
- Tool selection and dispatch.
- Root-agent and subagent supervision using the same run/session engine.
- Parent-child messaging, waiting, cancellation, and lifecycle propagation.
- Shell and local filesystem tools.
- MCP client lifecycle and capability projection.
- Skill discovery, activation, and resource loading.
- Session history and provider-state persistence.
- Context budgeting and compaction.
- Syntax and JSON Schema output validation.
- Same-session repair prompts.
- Cancellation, deadlines, and budgets.
- Native ACP v1 client/agent roles, stdio and in-memory transports, `pablo` extension schemas, and mapping between ACP sessions/updates and runtime state.
- Native A2A 1.0 client/server roles for remote-agent tasks, contexts, artifacts, discovery, streaming, and cancellation.
- Public run, control, event, tool, schema, observer, and provider contracts that official and community add-ons can use without private-core access.
- Trace creation, content hashing, and redaction hooks.
- Native OpenTelemetry context extraction/injection, instrumentation, semantic conventions, metrics, correlated logs, SDK integration, and OTLP export.
- Local artifact declaration and publication events.
- The reference CLI and TUI.

### 8.2 What the embedding application owns

The host should own:

- E2B, Daytona, Vercel Sandbox, container, or VM provisioning.
- Durable workflow orchestration and scheduling.
- Database state and transactions.
- Tenant isolation and authorization.
- Business-level idempotency.
- Human approval rules.
- Canonical domain records.
- Domain-specific semantic validation.
- External artifact storage and retention.
- Product-specific event presentation.
- Long-term trace retention and privacy policy.
- OpenTelemetry Collector/backend deployment, fleet-wide sampling, backend credentials, and service/resource naming policy.
- Business-level retries around complete runs.
- Which context snapshot is authoritative for a turn.
- Cross-tenant or fleet-wide concurrency quotas above the runtime’s per-run child limits.
- Exposure of any network-facing surface (AG-UI, Open Responses, networked ACP, A2A): endpoint lifecycle, TLS termination, request authentication, rate limiting, denial-of-service protection, and which agent profile, tools, data, and tenant it may reach.

### 8.3 Why sandbox provisioning stays outside the core

Otto creates a fresh E2B sandbox for each outer attempt. Graphline keeps a warm E2B sandbox per thread and coordinates capacity through Postgres locks and tenant policy. These are product lifecycle decisions, not generic agent-loop behavior.

`pablo` should run inside whichever execution environment the host supplies. Optional language-specific helpers can later provision popular sandbox products, but the Rust core should not take a dependency on E2B, Daytona, or Vercel Sandbox in version 0.1.

This design keeps the binary portable and allows the same runtime to operate:

- Directly on a developer’s machine.
- In a prebuilt sandbox image.
- As a child process in an application container.
- In a long-lived workspace.
- In a fresh isolated job.

## 9. Conceptual architecture

```text
Applications and clients
  direct Rust | native ACP stdio | TypeScript/Python SDK | CLI/TUI
  optional projections: AG-UI | Vercel AI SDK UI stream
                              |
                              v
+----------------------------------------------------------------+
|                      public a contracts                         |
| RunSpec | RunHandle | RunEvent | RunOutcome | Tool | Observer   |
+----------------------------------------------------------------+
                              |
                              v
+----------------------------------------------------------------+
|                         usable a core                           |
| Run/session/agent supervisor   Context and compaction           |
| Model and tool loop            Output and JSON Schema contracts |
| Event and control plane        Native trace and replay          |
| Budgets/cancellation           Artifacts and host callbacks     |
| ACP local-agent sessions       A2A remote-agent tasks           |
| Agent ownership/proxy tree     Chain graph and typed handoffs   |
| Unified capability registry    Shell/files/MCP/skills           |
+----------------------------------------------------------------+
              |                                |
              v                                v
 Provider boundary                     Agent/capability boundary
 Vercel | OpenRouter | Open Responses  A2A remote agents | MCP | optional OpenAPI
              |                                |
              +----------------+---------------+
                               |
                               v
       Local machine / E2B / Daytona / Vercel Sandbox / container

Core lifecycle operations create native OTel telemetry and the lossless
event record together. Events feed application-facing adapters; no adapter
reconstructs telemetry after the fact or reaches around public contracts.
```

The TUI consumes the same event protocol as an embedding application. It must not have a private execution path or own hidden product state.

## 10. Suggested repository shape

The exact Cargo workspace can change, but an initial structure should preserve clear dependency direction and make the supported core useful without every compatibility package:

```text
pablo/
  Cargo.toml
  crates/
    pablo-core/              usable provider-neutral runtime and public API
    pablo-agents/            ACP/A2A supervisor, hierarchy, messaging, budgets
    pablo-protocol/          JSON-RPC machinery, public types, pablo extensions
    pablo-acp/               native stable ACP v1 client/agent and transports
    pablo-a2a/               native A2A 1.0 client/server and HTTP binding
    pablo-providers/         Vercel, OpenRouter, Open Responses provider boundary
    pablo-open-responses/    native Open Responses mapping and conformance
    pablo-tools/             shell, filesystem, artifacts, host-tool bridge
    pablo-mcp/               MCP client and capability projection
    pablo-skills/            Agent Skills discovery and activation
    pablo-telemetry/         native OTel instrumentation, SDK/OTLP, conventions
    pablo-trace/             lossless event trace, redaction, and replay
    pablo-cli/               pablo executable, command parsing, reference TUI
    pablo-ag-ui/             official AG-UI server/projection add-on
    pablo-openapi/           official OpenAPI 3.1 tool-import add-on
  sdk/
    typescript/
      packages/ai-sdk/   Vercel AI SDK UIMessage stream adapter
    python/
  docs/
  examples/
```

This is an ownership map, not a requirement that every directory become a separately published crate immediately. Open Responses, ACP, and A2A modules may remain internal workspace crates, but they are linked, tested, documented, and supported as native parts of the `pablo` package. The optional dependency rule applies to AG-UI, Vercel AI SDK, and OpenAPI: they depend on public runtime/ACP contracts, the core never depends on them, and they do not depend on one another.

Three packaging layers avoid confusing “minimal” with “unusable”:

1. **Core package:** the supported runtime, provider/tool traits, model loop, JSON Schema and JSON-RPC foundations, Open Responses, root and child agents, ACP v1, A2A 1.0, sessions/tasks, events, control handles, budgets, native OTel, and extension registry.
2. **Default `pablo` distribution:** the core plus direct Vercel AI Gateway and OpenRouter support, shell/filesystem, MCP, skills, native trace/replay, and the CLI/TUI. It works immediately with one gateway key and can call an Open Responses endpoint. Version 0.1 serves ACP and delegates through the A2A client; enabling an A2A server endpoint is preserved 0.x work.
3. **Official add-ons:** separately selectable crates, companion binaries, or SDK packages for AG-UI, Vercel AI SDK UI streams, and OpenAPI. A larger convenience distribution may bundle them, but the ordinary core dependency and executable do not need to.

OTel is not in the third layer. The core uses the OTel API and data model directly, and the default `pablo` distribution includes an OTel SDK plus OTLP/HTTP export. An embedded host may supply its own compatible providers and processors, and alternate exporters can remain optional, but disabling export does not select a different telemetry model.

Rust hosts compose add-ons in process. Non-Rust hosts and users of prebuilt binaries can run an add-on as an ACP client of `pablo acp --stdio`. This supplies post-install extensibility without promising an unstable dynamic Rust ABI or loading arbitrary native libraries into the agent process.

## 11. Public integration surfaces

### 11.1 Native executable

The easiest cross-language integration is a prebuilt static or minimally dynamic executable placed inside the target environment.

Proposed full command direction. Section 29.1, not this catalog, defines which commands gate version 0.1; commands marked post-0.1 are intentionally preserved without implementation pressure:

```text
pablo                                      Open the interactive TUI
pablo "summarize these files"              Run a task interactively
pablo init                                 Scaffold .pablo/config.toml and .agents/skills/ here (post-0.1)
pablo run --spec run.json                  Run from a declarative specification (post-0.1)
pablo run --json "summarize these files"   Emit one machine-readable outcome document
pablo run --stream "..."                   Emit JSON Lines events, ending with the outcome
pablo run --dry-run "..."                  Show the assembled request without calling a model (post-0.1)
pablo run --timings "..."                  Print startup and per-step timing on stderr (post-0.1)
pablo resume SESSION_ID                    Resume a session (post-0.1)
pablo sessions list                        List resumable sessions in this workspace (post-0.1)
pablo sessions show SESSION_ID             Inspect one session, its children, and budgets (post-0.1)
pablo acp --stdio                          Serve the native ACP agent protocol
pablo a2a serve                            Serve configured profiles over A2A (post-0.1)
pablo a2a agents                           Inspect configured remote A2A agents
pablo agents tree                          Inspect the current parent/child tree
pablo agents graph [CHAIN_ID]              Inspect dependencies and handoffs (post-0.1)
pablo agents inspect AGENT_ID              Inspect one child agent
pablo agents stop AGENT_ID                 Stop one child agent
pablo mcp list                             Inspect MCP servers and capabilities
pablo mcp add NAME --command CMD | --url   Add a server to workspace or user config (post-0.1)
pablo mcp test SERVER                      Test one MCP connection
pablo skills list                          Inspect discovered skills
pablo skills show NAME                     Inspect one skill
pablo skills check PATH                    Validate a skill package
pablo trace show TRACE                     Inspect a trace
pablo trace diff TRACE_A TRACE_B           Compare prefix, tools, config, cost, and outcome (post-0.1)
pablo trace export --redacted TRACE        Write a shareable trace without content or secrets (post-0.1)
pablo trace replay TRACE                   Replay a captured task (post-0.1)
pablo config explain                       Show every resolved value and its source (post-0.1)
pablo doctor                               Diagnose provider, MCP, skill, shell, and startup timing
```

Exact names may change, but the concepts should remain available.

### 11.2 Rust library

Rust applications should be able to embed the core directly without spawning a child process. This surface may initially be less stable than the process protocol while the architecture settles.

### 11.3 TypeScript and Python SDKs

Version 0.1 ships a tiny TypeScript ACP reference client for Otto, not a new stable language SDK. The design below is preserved for later ergonomic TypeScript and Python packages after the ACP extensions and event contracts survive real integration.

The first SDKs should be intentionally thin. They should:

- Download or locate the correct native binary.
- Spawn and supervise it.
- Use ACP initialization and capability negotiation, preferably through an official ACP SDK where one exists.
- Expose typed run/session methods.
- Expose typed spawn, message, wait, list, inspect, and cancel operations for child agents.
- Expose typed handoffs, dependency joins, and execution-graph snapshots without reimplementing their semantics.
- Extract and inject W3C Trace Context and allowlisted Baggage through the process protocol.
- Decode ACP `session/update` notifications and `session/prompt` outcomes into ergonomic SDK event/result types without inventing another wire protocol.
- Answer host-tool callbacks.
- Propagate cancellation.
- Surface stderr and process failures as typed SDK errors.
- Optionally adapt events to SSE or framework-specific streams.

The SDKs should not reimplement the model loop, ACP/A2A state machines, MCP client, skill loader, or retry policy. TypeScript should use the official ACP SDK where its stable surface fits. Python may use generated stable-v1 schema types plus a small JSON-RPC transport until an appropriate official SDK exists.

Three SDK details decide whether the first ten minutes feel good:

- **Binary delivery without postinstall scripts.** Ship the native binary in per-platform optional dependencies for npm (the pattern esbuild and swc use) and in platform wheels for Python (the pattern ruff uses), so `npm install` and `pip install` work behind proxies and in CI without a download step. Honor an `PABLO_BINARY` environment variable and an explicit constructor option for a custom path. The SDK checks that the binary's protocol version matches its own and fails with one clear message when it does not.
- **Generated types from one source of truth.** The Rust contracts produce JSON Schema, and CI generates the TypeScript and Python event, spec, outcome, tool, and error types from that schema. Hand-written SDK types drift; generated ones cannot. The SDKs add only ergonomic wrappers over the generated types.
- **Idiomatic cancellation and iteration.** TypeScript accepts an `AbortSignal` and exposes events as an async iterable; Python accepts `asyncio` cancellation and exposes an async iterator. Both expose discriminated unions for events and outcomes, a helper for declaring host tools from a JSON Schema (with optional Zod and Pydantic adapters), and a blocking `run()` convenience that returns the typed outcome for callers who do not need streaming.

### 11.4 Official add-on contract

An add-on is an adapter over public runtime contracts, not a plugin with privileged access. Every in-process add-on should be constructible from explicit handles such as:

- A runtime client or `RunHandle` factory.
- The versioned `RunSpec`, `RunEvent`, `RunOutcome`, `Tool`, schema, artifact, error, and trace-context types.
- A bounded event subscription with documented backpressure behavior.
- Control operations for cancellation, resume, host input, and host-tool results.
- Explicit credential, HTTP transport, clock, storage, and observer dependencies where applicable.

An add-on must not depend on private scheduler state, mutate session files directly, install a process-global runtime or telemetry subscriber, or infer policy from presentation events. It uses the same public operations available to another embedding host.

Each official add-on must publish:

- Its supported upstream protocol and package versions.
- Its mapping from external identities and lifecycle states to `pablo` identities and outcomes.
- Capability negotiation and unsupported-feature behavior.
- Whether it is lossless, lossy, or uses extension fields for each event/content type.
- Cancellation, reconnect, resume, error, and backpressure behavior.
- Authentication and trust-boundary responsibilities.
- Trace-context extraction and injection behavior.
- Golden protocol fixtures plus upstream conformance or acceptance-test results where available.
- A compatibility matrix against supported `pablo-protocol` versions.

Official add-ons release independently when useful, but CI must test them against the oldest and newest supported core protocol versions. In-process crates follow Rust semver compatibility. Companion processes negotiate the wire protocol and may evolve independently. Unknown optional fields should survive round trips where the external protocol permits it; unknown required semantics fail explicitly rather than being silently discarded.

No network-facing surface, including native A2A serving, listens merely because the package or an add-on is installed. The host explicitly starts it, supplies bind addresses and authentication, and selects the named agent profile it may expose. Default binds should be loopback-only.

### 11.5 Core schema and process interoperability

#### JSON Schema Draft 2020-12

Draft 2020-12 is the canonical schema dialect for:

- Tool input and structured result contracts.
- Free-standing structured output contracts.
- Typed inter-agent handoffs and host callbacks.
- Add-on configuration and protocol payload definitions where JSON Schema applies.
- Checked examples and generated TypeScript/Python types.

Schemas should declare `$schema`. The validator must support the documented subset consistently, including local `$ref` and `$defs`, and report unsupported vocabularies or keywords rather than pretending validation succeeded. Network retrieval of external references is disabled by default; an explicitly configured resolver must bound schemes, hosts, recursion, cycles, document count, and bytes. `format` behavior must state whether each format is annotation-only or asserted. Schema compilation is cached by canonical digest, and validation errors are bounded, path-aware, and safe to return to the model.

Rust developers may use optional Serde and schema-derive helpers, but raw JSON Schema remains the language-neutral contract. A derive library’s output is validated against the same fixtures as schemas supplied from TypeScript, Python, MCP, OpenAPI, or files.

#### Shared JSON-RPC 2.0 machinery

JSON-RPC 2.0 parsing, request-ID correlation, bidirectional request/response routing, notification handling, error representation, cancellation plumbing, size limits, and test fixtures are native protocol infrastructure. ACP uses this machinery directly. MCP and A2A bindings should reuse it only where their official SDKs and selected bindings expose the same behavior; do not fork or bypass an official protocol state machine merely to share code.

There is no separate proprietary “`pablo` JSON-RPC” lifecycle beside ACP. Product-specific operations are capability-negotiated ACP extensions using ACP’s standard extensibility rules. This preserves one client/agent handshake, session model, update stream, and cancellation path.

#### Native ACP v1 process protocol

The native process protocol is stable ACP v1, including its JSON-RPC 2.0 envelope, initialization and capability exchange, authentication declarations, session creation/load, prompt turns, `session/update` notifications, client filesystem/terminal/elicitation capabilities, permission requests, cancellation, content blocks, tool-call updates, plans, usage updates, stop reasons, errors, `_meta`, and extension-method rules.

`pablo` must follow ACP’s published schema and transport specification rather than define lookalike envelopes or alternate method names. Exact ACP schema and SDK releases are pinned and surfaced through `pablo doctor`. The implementation treats the versioned schema as authoritative when prose examples are incomplete.

The high-level `RunSpec`, `RunEvent`, and `RunOutcome` remain ergonomic Rust/TypeScript/Python types, but their wire representation is an ACP composition:

- `initialize` negotiates ACP plus versioned `pablo` extension capabilities.
- `session/new` or `session/load` establishes the runtime session, working directory, and MCP configuration.
- `session/prompt` carries the turn’s content and any negotiated `pablo` run metadata.
- Standard `session/update` variants carry messages, thought summaries when explicitly available, tool calls, progress, plans, configuration, session information, and usage.
- Standard prompt responses and JSON-RPC errors terminate a turn; negotiated metadata carries richer typed `pablo` outcomes when needed.
- `session/cancel` cancels the actual run rather than only stopping delivery to the client.

Only missing product semantics use extensions. Prefer namespaced `_meta` values over new methods when data belongs to an existing ACP interaction. Use `_`-prefixed custom methods only for operations with no ACP lifecycle equivalent, such as optional tree inspection or a direct typed host-tool callback that cannot be expressed through MCP or an ACP client capability. Before release, every extension receives a stable project-controlled URI/name, JSON Schema, version, capability bit, size bound, visibility class, and fallback behavior for generic ACP peers.

The native ACP transport must still bound frame size, content references, pending reverse requests, event queues, and slow consumers. It must handle bidirectional request-ID ownership, cancellation races, late responses, clean shutdown, and unknown future fields/variants. Large binary and artifact content moves by bounded ACP resource/content references or host artifact references instead of unbounded base64 in updates.

### 11.6 Native local and remote subagent communication

#### ACP for local agents

The supervisor is an ACP client. Every local root or child agent is an ACP agent. This applies even when both live in the same process:

- **In-process:** use the official ACP Rust types and handlers over a typed in-memory transport, avoiding JSON serialization while preserving protocol state and behavior.
- **Subprocess:** use ACP’s standard stdio transport.
- **Externally supplied local agent:** allow a configured third-party ACP agent to occupy a child slot under the same supervisor-side limits and observability.

Creating a local child creates an ACP connection/session, records the returned `sessionId`, and sends its task through `session/prompt`. Child progress and tool activity arrive through `session/update`; the prompt response or error becomes the child outcome; follow-up messages are later prompts on the same session; persistent children retain the ACP session binding; stop uses `session/cancel` plus bounded process termination when applicable.

The supervisor retains deterministic state that ACP does not own: parent/root identity, authority intersection, concurrency slots, budget reservations, wait graph, chain dependencies, workspace lease, checkpoint identity, and native OTel topology. These values travel through negotiated `_meta` only when the peer needs them. An unextended third-party ACP child can still run, but receives conservative supervisor-side defaults and cannot claim capabilities or authority merely by returning metadata.

ACP client filesystem, terminal, elicitation, and permission methods are capabilities exposed by the supervisor/host, not ambient access. Calls enter the same policy, budget, tool, trace, and cancellation paths as native operations. The child’s internal tool calls are reported through standard ACP tool updates even when execution stays inside its own runtime.

#### A2A for remote agents

The package architecture natively supplies both A2A 1.0 roles. Version 0.1 gates the client role only; the server role remains native 0.x work rather than an external add-on:

- **A2A client:** a parent/supervisor delegates to an explicitly configured remote agent using its Agent Card, messages, streaming-message operation, task/status lifecycle, context continuity, artifacts, follow-up input, and cancellation.
- **A2A server:** `pablo` exposes explicitly selected agent profiles and their public skills/capabilities through bounded Agent Cards and maps incoming A2A work onto ordinary supervised runs.

A remote delegation creates a local proxy `AgentRef` with kind `remote_a2a`. Its A2A `contextId`, optional `taskId`, endpoint identity, Agent Card digest, protocol version, and remote artifact IDs are recorded without becoming canonical local session IDs. Standard A2A text, file, and structured data Parts carry inputs and typed handoffs; Artifacts carry deliverables. Streaming task/status and artifact updates become native events and OTel activity under the proxy node.

A2A is native for remote subagents, not for an in-process child where ACP already provides a smaller session/control path. Conversely, ACP is not stretched into a remote agent discovery and task protocol. Both produce the same local `AgentRef`, `RunOutcome`, graph, budget, artifact, and observability interfaces at the supervisor boundary.

| Concern | Local child | Remote child |
| --- | --- | --- |
| Native protocol | ACP v1 | A2A 1.0 |
| Initial transport | Typed in-memory or ACP stdio | One documented A2A HTTP binding |
| Continuity | ACP `sessionId` | A2A `contextId` and optional `taskId` |
| Progress | ACP `session/update` | A2A task/status/artifact stream |
| Completion | ACP prompt response or error | A2A Message or terminal Task |
| Cancellation | ACP `session/cancel` and local process control | A2A task cancellation with delivery uncertainty |
| Local policy guarantee | Full inherited authority and budget enforcement | Only local endpoint, input, credential, time, and output controls; remote internals are opaque |
| Visibility | Full native lifecycle | Protocol-visible remote lifecycle under a local proxy node |

The project should define the smallest possible versioned A2A extension for W3C trace context and optional `pablo` correlation/schema metadata only where standard service parameters, metadata, and Parts are insufficient. Remote peers need not implement it. Base A2A interoperability must continue with reduced correlation rather than failure.

Agent Cards expose only configured public capabilities, never the raw shell/tool inventory, filesystem paths, internal prompts, credentials, private children, or local policy rules. Remote messages, cards, status text, metadata, and artifacts are untrusted data. Authentication, TLS, tenant routing, endpoint allowlists, rate limits, and service discovery remain host responsibilities.

Remote authority and cost claims have a different strength from local enforcement. `pablo` can bound what it sends, how long it waits, what local credentials/tools it exposes, and how much returned data it accepts; it cannot prove that another service honored an internal model, token, tool, or cost ceiling. Traces and UI must label remote reported usage separately. Cancellation, retry, and side effects may have uncertain delivery and must never be presented as local exactly-once behavior.

### 11.7 Native Open Responses and optional compatibility add-ons

#### Open Responses

Open Responses client/provider support is native to the `pablo` package. It translates Open Responses input items, multimodal content, tool definitions, tool calls/results, streamed events, usage, finish state, and provider extensions into the provider-neutral model boundary. The implementation may live in its own internal crate for ownership and testing, but it is linked and available in the supported core/default distribution without installing an add-on.

The internal content model should preserve at least the information Open Responses can express but may remain a richer superset needed for gateway routing, provider continuation state, delivery certainty, retry ownership, and exact provenance. Unsupported or lossy mappings produce explicit diagnostics. Provider-specific fields remain namespaced and never silently become portable core behavior.

Open Responses does not define `pablo` sessions, local subagents, authority, shell execution, chain scheduling, or native replay. Version 0.1 requires native client/provider compatibility, not an Open Responses server that presents an entire multi-agent run as if it were one model response. Because the specification is young, pin an immutable specification or acceptance-suite revision and expose it through diagnostics.

AG-UI, the Vercel AI SDK stream projection, and OpenAPI import remain optional official add-ons because they are application/API conveniences rather than foundations of agent execution.

#### AG-UI

`pablo-ag-ui` is the primary application-facing web add-on. Its initial server surface accepts an AG-UI run input over HTTP and emits AG-UI events over Server-Sent Events. It maps lifecycle, messages, streamed text, tool calls, tool results, activity/progress, shared-state snapshots/deltas, interrupts, cancellation, and custom events to the native runtime.

The native `RunEvent` remains authoritative and richer. Agent hierarchy, typed handoffs, artifacts, policy decisions, detailed usage, and replay cursors use standard AG-UI fields when available and a versioned `pablo` extension namespace otherwise. The mapping must state which extensions a generic AG-UI client can safely ignore. AG-UI state is an application projection, not permission or canonical business state, and it cannot widen the runtime’s host policy.

The first implementation should support reconnect or replay from the native sequence cursor where the transport permits it, propagate cancellation to the real `RunHandle`, and route client-side tools through the host-tool contract. It must not turn an HTTP disconnect into implicit run cancellation unless the host selected that policy.

#### Vercel AI SDK `UIMessage` streams

The TypeScript package supplies a small adapter from `RunEvent` to the Vercel AI SDK `UIMessage` stream protocol and helpers usable by `useChat`. It should project text, reasoning summaries when explicitly available, tool inputs/results, typed data parts, sources, files/artifacts, errors, finish state, and transient activity without re-running or reinterpreting the agent.

The adapter belongs in the TypeScript SDK and does not introduce Node.js into the Rust executable. It supports the current pinned AI SDK stream contract, emits the required headers and Server-Sent Event framing, preserves stable message/part IDs across reconnects, and exposes raw `pablo` extensions for hierarchy and advanced inspection. A host can choose AG-UI or AI SDK projection from the same event stream without changing execution.

#### OpenAPI 3.1

`pablo-openapi` imports explicitly selected OpenAPI 3.1.x operations into the unified `Tool` descriptor. It resolves parameters, request bodies, response schemas, servers, content types, and security-scheme metadata while preserving the source document, operation, and schema digests.

Import is not authorization. The host must select operations, bind server URLs, supply authentication outside model-visible data, classify effects, set time and size bounds, and decide whether calls execute inside or outside the sandbox. An imported operation defaults to `dynamic` effect unless explicit trusted policy narrows it. Missing or duplicate `operationId` values receive deterministic qualified identities or fail according to strict mode.

External references, recursive schemas, redirects, generated URLs, response decompression, and arbitrary content types are bounded. Remote reference fetching is off by default. OpenAPI descriptions and examples are untrusted input and cannot grant tools, alter instructions, or supply credentials. Start with the 3.1 feature set, accepting compatible 3.1 patch releases; add 3.2 only after its mapping and ecosystem support are tested.

### 11.8 Add-on ergonomics

The preferred experience should be recognizable in each ecosystem:

- Rust: ACP, A2A, Open Responses, JSON Schema, JSON-RPC, and OTel are already present; add `pablo-ag-ui` or `pablo-openapi` only when that projection is needed.
- TypeScript: install the base SDK plus a narrow package such as the AI SDK stream adapter.
- Python: consume the base async event iterator directly or add a protocol-specific web adapter.
- Prebuilt-process users: install or download a companion binary that connects as an ACP client to `pablo acp --stdio`.

Add-ons should use ordinary async futures and stream traits rather than expose concrete internal channels. Optional Tower-style service/layer adapters can support Rust middleware, but the stable tool and provider contracts must not require an application to adopt one web framework. An embedded library never creates its own Tokio runtime when already running inside a host runtime, and it never installs a global tracing subscriber.

There is no runtime marketplace, arbitrary dynamic library loading, or implicit code download in version 0.1. Discoverability comes from documentation, coordinated package naming, examples, and a machine-readable compatibility manifest shipped with each official add-on. Installation and execution remain explicit.

### 11.9 The executable name

`pablo` is the project name and canonical executable name. Use it consistently for the CLI, package names, configuration directory (`.pablo/`), and custom telemetry namespace (`pablo.*`).

The installer should install the `pablo` executable and check for an existing command before installation. It must never silently overwrite an existing command, alias, function, or executable.

### 11.10 Illustrative host code

The contracts in section 12 are easier to judge against the code a host should be able to write. These examples are the target shape, not final API or package names. Each performs the same work: run one bounded task with a structured output contract, expose one host tool, observe safe progress, and receive a typed outcome.

The custom TypeScript/Python SDK and direct host-tool APIs shown here are post-0.1 ergonomics. The version 0.1 Otto proof uses an ACP client directly and exposes application tools through MCP; the Rust embedding shape can evolve alongside the core.

<details>
<summary>Show target TypeScript, Python, Rust, and CLI examples</summary>

TypeScript:

```ts
import { createRuntime, defineTool } from "pablo-sdk";

const pablo = await createRuntime({ workspace: "./job-1234" }); // spawns `pablo acp --stdio`, negotiates ACP and `pablo` extensions

const getLoanSnapshot = defineTool({
  name: "get_loan_snapshot",
  description: "Return the canonical loan snapshot for a loan ID.",
  input: { type: "object", properties: { loanId: { type: "string" } }, required: ["loanId"] },
  effect: "read",
  presentation: { active: "Reading loan record", done: "Read loan record" },
  handler: async ({ loanId }) => db.loans.snapshot(loanId),
});

const run = pablo.run({
  input: "Extract every covenant from the documents in ./inputs and cite the source page.",
  instructions: { developer: COVENANT_INSTRUCTIONS },
  skills: ["covenant-extraction"],
  tools: [getLoanSnapshot],
  output: { schema: covenantReportSchema },
  budget: { maxSteps: 60, maxCostUsd: 2, maxDurationSeconds: 900 },
  subagents: { maxActivePerParent: 3 },
  signal: controller.signal,
});

for await (const event of run.events()) {
  switch (event.type) {
    case "activity":          ui.progress(event.label); break;          // user-safe, content-free
    case "tool.call.started": log.debug(event.tool, event.agentId); break;
    case "usage":             meter.record(event.costUsd); break;
  }
}

const outcome = await run.outcome();
if (outcome.kind === "completed" && outcome.output) {
  await persist(outcome.output, outcome.traceId);
} else if (outcome.kind === "validation_failed") {
  await run.repair({ feedback: semanticValidator(outcome) });           // same session, bounded
}
```

Python:

```python
from pablo_sdk import Runtime, tool

@tool(effect="read", presentation={"active": "Reading loan record", "done": "Read loan record"})
async def get_loan_snapshot(loan_id: str) -> dict:
    """Return the canonical loan snapshot for a loan ID."""
    return await db.loans.snapshot(loan_id)

async with Runtime(workspace="./job-1234") as pablo:
    run = pablo.run(
        input="Extract every covenant from the documents in ./inputs and cite the source page.",
        instructions={"developer": COVENANT_INSTRUCTIONS},
        skills=["covenant-extraction"],
        tools=[get_loan_snapshot],
        output={"schema": covenant_report_schema},
        budget={"max_steps": 60, "max_cost_usd": 2, "max_duration_seconds": 900},
    )
    async for event in run.events():
        if event.type == "activity":
            ui.progress(event.label)
    outcome = await run.outcome()
    if outcome.kind == "completed" and outcome.output:
        await persist(outcome.output, outcome.trace_id)
```

Rust, embedded in process:

```rust
let runtime = pablo_core::Runtime::builder()
    .provider(pablo_providers::vercel::from_env()?)
    .tool(GetLoanSnapshot::new(db.clone()))
    .tracer_provider(host_tracer_provider)        // never installs a global subscriber
    .build()?;

let mut run = runtime.start(
    RunSpec::task("Extract every covenant from the documents in ./inputs and cite the source page.")
        .workspace("./job-1234")
        .skill("covenant-extraction")
        .output(OutputContract::json_schema(covenant_report_schema))
        .budget(Budget::default().max_steps(60).max_cost_usd(2.0)),
)?;

while let Some(event) = run.events().next().await {
    if let RunEvent::Activity { label, .. } = &event { ui.progress(label); }
}
match run.outcome().await? {
    RunOutcome::Completed { output: Some(output), trace_id, .. } => persist(output, trace_id).await?,
    RunOutcome::ValidationFailed { diagnostics, .. } => { run.repair(diagnostics).await?; }
    other => map_failure(other),
}
```

Headless CLI, same task:

```bash
pablo run --json --skill covenant-extraction --output-schema covenant.schema.json \
  "Extract every covenant from the documents in ./inputs and cite the source page." > result.json
```

`result.json` is one JSON document carrying `schema_version`, the typed `outcome`, `usage`, `trace_id`, and `session_id`. `--stream` instead writes JSON Lines events to stdout and ends with the outcome record, so a host that only needs a pipe never has to speak ACP.

These examples deliberately show what a host does not write: no model loop, no stream parser, no retry policy, no tool dispatcher, no trace format. If an integration needs any of those, the runtime contract is missing something.

</details>

## 12. Core typed contracts

The implementation should define contracts before building UI details.

The shapes below are the compatibility direction, not a demand to implement every field in version 0.1. The focused release freezes only fields exercised by section 31.1; reserved future concepts should remain optional or internal until their behavior exists.

### 12.1 `RunSpec`

A run specification should include at least:

- Protocol/schema version.
- Run ID and optional parent trace ID.
- Optional incoming W3C `traceparent`, `tracestate`, allowlisted `baggage`, and additional span-link contexts.
- Optional existing session reference.
- User input.
- System and developer instruction layers.
- Workspace root.
- Input mount descriptions and trust labels.
- Activated and available skills.
- MCP server configuration or references.
- ACP client capabilities and negotiated `pablo` extension values when invoked through ACP.
- Allowed A2A remote-agent references and server-profile identity when applicable.
- Built-in and host capability configuration.
- Subagent enablement, concurrency, depth, child-count, inheritance, and budget policy.
- Provider, model, reasoning, and provider options.
- Output contract.
- Step, token, time, cost, and tool-result budgets.
- Retry and fallback policy.
- Trace capture policy.
- OTel instrumentation, sampling, export, and content-capture policy without embedding exporter credentials.
- Artifact policy.
- Environment metadata supplied by the host.

### 12.2 `AgentRef` and `SessionRef`

Every root or child agent should have a serializable `AgentRef` containing:

- Stable agent ID.
- Optional human-readable name.
- Kind: root, temporary local ACP child, persistent local ACP child, or remote A2A proxy child.
- Root run/session ID.
- Optional parent agent ID.
- Immutable owner/root-session identity.
- Depth in the agent tree.
- Current lifecycle state.
- Optional session reference.
- Native protocol binding: ACP session or A2A remote proxy.
- Protocol peer identity and negotiated version/capabilities.

A session reference should be serializable and include:

- Runtime session ID.
- ACP `sessionId` for a local/root agent or A2A `contextId` plus optional active `taskId` for a remote proxy.
- Owning agent ID and optional parent agent ID.
- Session format version.
- Provider-specific opaque continuation state when present.
- History or history-store reference.
- Workspace identity.
- Host context identity/version.
- Active instruction, tool, schema, and skill fingerprints.
- Creation and last-use timestamps.

Provider session IDs must not be the canonical session identity. Providers can rotate IDs or expose no durable server-side session at all.

### 12.3 `SubagentSpec`

A child-agent request should include:

- Stable call/request ID.
- Operation: run, create/message, wait, inspect, or stop.
- Child transport: local in-process ACP, local/subprocess ACP, configured external ACP process, or remote A2A agent.
- For A2A, a configured remote-agent identity or discovered Agent Card reference rather than an arbitrary model-generated URL.
- Optional stable child name for persistent children.
- Complete bounded task or message.
- Optional child-specific instruction overlay.
- Explicit context attachments from the parent.
- Workspace mode and optional child working directory.
- Optional model/provider/effort override within the host’s allowed set.
- Tool, MCP, skill, ACP client-capability, and remote-agent restrictions that can only narrow inherited local authority.
- Optional logical chain and node identity.
- Explicit upstream outcome, evidence, and artifact references.
- A dependency condition such as all valid, all settled, any valid, or quorum.
- A bounded result projection describing what downstream agents may receive.
- Output contract.
- Child step/token/time/cost budget or requested reservation.
- Lifetime policy for parent completion or cancellation.

The runtime returns an `AgentRef` immediately for asynchronous work and eventually a structured child outcome. A synchronous convenience operation may wait internally, but the protocol must preserve the independent child identity and event stream.

A local ACP child receives full inherited enforcement; a remote A2A child receives only the local-side controls described in section 11.6.

### 12.4 `Provider`

A provider adapter should accept a provider-neutral request containing:

- Model ID.
- Messages and multimodal parts.
- Advertised tool schemas.
- Tool choice.
- Structured response format.
- Reasoning options.
- Cache plan: the breakpoints and TTL to apply, or automatic mode, resolved from the adapter's capability record (section 28.4).
- Output-token bound.
- Deadline and cancellation handle.
- Active OTel context and native trace identifiers.
- Credential lease supplied by the configured credential authority.

It should emit normalized incremental events and a final result containing:

- Content parts.
- Tool calls.
- Finish reason.
- Provider-specific opaque continuation state.
- Requested and resolved model/provider identity.
- Exact or best-available usage, including cache-read and cache-write token counts and the TTL applied when the provider reports them.
- Gateway generation/request identifiers.
- Delivery certainty and retry evidence.
- Provider diagnostics.

### 12.5 `Tool`

Every built-in, host, or MCP tool should normalize into one internal descriptor:

- Stable qualified identity.
- Model-facing name.
- Human-facing title and activity labels.
- Description.
- JSON input schema.
- Optional structured output schema.
- Origin: built-in, shell, host, or MCP server.
- Effect metadata when known.
- Sensitivity and trace policy.
- Idempotency metadata.
- Timeout and result-size limits.
- Execution function or transport binding.

Suggested effect vocabulary:

- `read`
- `local_write`
- `external_write`
- `irreversible`
- `dynamic`

Shell is generally `dynamic`; a shell command cannot be reliably understood from its top-level executable alone.

The built-in subagent tool is a supervisor interface, not a second implementation of the agent loop. Its operations create or address ACP sessions for local children or A2A tasks/contexts for remote children through the same supervisor, identity, graph, budget, policy, and observability surface used by the root.

### 12.6 `RunEvent`

The versioned event union should cover:

- Run/session lifecycle.
- Agent created, started, idle, resumed, completed, failed, stopped, and removed.
- ACP connection/session/prompt/update/cancel state and A2A discovery/context/task/status/artifact state when applicable.
- Parent-child message delivery, wait registration, and wait completion.
- Chain node blocked, ready, started, skipped, settled, and dependency-satisfied transitions.
- Typed handoff creation, validation, delivery, rejection, and acknowledgement.
- Child concurrency queueing and budget reservation/release.
- Model request start and finish.
- Assistant content delta.
- Reasoning summary or provider reasoning metadata when explicitly available.
- Tool selection.
- Tool input delta.
- Tool call start, progress, result, and failure.
- Shell stdout and stderr chunks.
- MCP connection, negotiation, progress, and failure.
- Skill discovery, selection, activation, resource load, and failure.
- Output validation and repair.
- Artifact declaration.
- Usage and cost updates.
- Context utilization and compaction.
- Retry and fallback.
- Cancellation and timeout.
- Human or host input required.
- Final outcome.

Every event should carry:

- Schema version.
- Run and session IDs.
- Agent ID, optional parent agent ID, and root agent ID.
- Monotonic sequence number.
- Timestamp.
- Trace/span identity and optional parent span.
- OTel trace flags and enough context to correlate JSON logs using lowercase hexadecimal `trace_id` and `span_id` fields.
- Visibility classification.
- Optional sensitivity/redaction metadata.
- Source protocol plus correlated ACP session or A2A context/task identity when the event crossed one of those boundaries.

An illustrative wire form, using snake_case fields and a dotted `type` discriminator:

```json
{
  "schema_version": "0.1",
  "type": "tool.call.started",
  "seq": 4182,
  "ts": "2026-09-03T21:14:07.412Z",
  "run_id": "run_01J9K3...",
  "session_id": "ses_01J9K3...",
  "agent_id": "agt_01J9K4...",
  "parent_agent_id": "agt_01J9K3...",
  "root_agent_id": "agt_01J9K3...",
  "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
  "span_id": "00f067aa0ba902b7",
  "parent_span_id": "53ce929d0e0e4736",
  "trace_flags": "01",
  "visibility": "operator",
  "tool": "mcp:slack/search_messages",
  "call_id": "call_8f3a",
  "effect": "read",
  "presentation": { "active": "Searching Slack", "done": "Searched Slack" },
  "policy": { "decision": "allow", "rule_id": "mcp.default" }
}
```

Internally the shared identity fields live once in a reference-counted run context and each event carries only what varies; the full envelope is materialized at serialization boundaries. The native trace writes the static context once as a header record so per-event overhead stays small (section 28). Text, tool-input, and shell-output deltas carry a `seq` range when a transport coalesces them.

### 12.7 `RunOutcome`

Terminal outcomes should distinguish:

- Completed with a valid final response.
- Completed with structured output.
- Waiting for host input.
- Waiting for host tool result.
- Interrupted.
- Cancelled.
- Timed out.
- Failed before provider delivery.
- Failed after possible provider delivery.
- Exhausted retry/fallback policy.
- Validation failure.
- Policy denial.
- Child completed, child failed, child stopped, or child wait timed out, including a bounded result suitable for returning to the parent.
- Remote A2A completed, input-required, auth-required, rejected, cancelled, failed, or delivery-uncertain outcomes without collapsing remote uncertainty into local success/failure claims.

Do not represent all non-success states as an empty assistant string.

### 12.8 `OutputContract`

Supported output modes should include:

- Freeform text or Markdown.
- JSON constrained by JSON Schema.
- An explicitly declared artifact.
- Host-semantic validation after syntactic/schema validation.

The runtime can own syntax and JSON Schema checks. Domain semantic validation may require application data and should be callable through the host protocol. A failed validation should return bounded, non-sensitive diagnostics and optionally resume the same session for repair.

## 13. Runtime lifecycle

Two workload shapes must use the same core rather than becoming separate products. Either shape may supervise child agents.

### 13.1 Bounded run

This is the Otto shape:

```text
prepare -> start -> model/tool loop -> validate -> repair? -> complete/fail -> close
```

The host usually creates and destroys the surrounding sandbox.

### 13.2 Resumable session

This is the Graphline shape:

```text
open/reconnect -> turn -> idle -> turn -> sleep/wake -> turn -> close
```

Each turn is still a bounded model/tool loop. The session persists history, provider state, and workspace identity between turns.

### 13.3 Suggested internal states

```text
created
preparing
ready
running
queued
waiting_for_tool
waiting_for_host
waiting_for_children
validating
repairing
completed
failed
cancelled
idle
sleeping
closed
```

State transitions should be explicit, observable, and tested. The UI should derive from events and snapshots rather than maintain an independent hidden state machine.

### 13.4 Child-agent lifecycle

A parent may start a bounded temporary child or create and message a named persistent child. Every child passes through the ordinary run/session state machine and emits normal model, tool, skill, MCP, usage, artifact, and completion events under its own `agent_id`. The supervisor's responsibilities for limits, budget reservation, queueing, message routing, bounded outcomes, cancellation propagation, orphan prevention, and persistent-child ownership are specified in section 17.

### 13.5 Cancellation semantics

The following are different and must not be conflated:

- Stop rendering or disconnect one client.
- Cancel the current model request.
- Cancel the current tool or shell process.
- Cancel the current turn.
- Stop one child agent without cancelling its siblings or parent.
- Cancel a parent and its descendant tree.
- Close the session.
- Destroy the surrounding sandbox.

Cancellation should propagate through model HTTP, shell process groups, MCP requests, host callbacks, and descendant agents where possible. Root cancellation should cancel temporary descendants by default. Persistent children follow an explicit lifetime policy and must never become unowned. A disconnected web client should not automatically imply that a durable server-side turn must be killed; that is a host policy decision.

## 14. Shell design

Shell is enabled in version 0.1 and should be treated as a primary capability, not an escape hatch.

### 14.1 Why shell matters for non-coding agents

Shell access enables:

- PDF and document conversion.
- OCR and media processing.
- Spreadsheets and data transformation.
- Existing vendor CLIs.
- Small generated Python, JavaScript, Bash, or other scripts.
- Archive inspection.
- Search and text processing.
- Browser automation CLIs.
- Local databases.
- Custom organization utilities.
- Installing task-specific packages inside an isolated environment.

This is a major reason products use coding agents for non-coding work today.

### 14.2 Proposed shell tool

One shell tool can expose an action union:

- `run`: start a command, optionally with a TTY.
- `interact`: write exact input and/or observe a running process.
- `stop`: terminate a running process gracefully or forcibly.

The request should support:

- Exact command string.
- Working directory.
- Shell executable/profile selection.
- Environment additions and removals.
- TTY choice.
- Initial yield interval.
- Optional absolute deadline.
- Output byte bound.

The result and events should include:

- Stable process/session handle.
- Separate stdout and stderr streams where possible.
- Terminal-safe preview.
- Exact retained-output handle when preview encoding or truncation occurs.
- Exit status or signal.
- Start/end/duration timestamps.
- Timeout/cancellation reason.
- Truncation metadata.

Long-running commands must not require shell detachment tricks such as `nohup` or `&`. The runtime should own their process lifetime.

### 14.3 Version 0.1 execution policy

Version 0.1 supports `yolo` execution. It does not pause for interactive approval.

The recommended semantic split is:

```text
approval behavior: none
static capability policy: configurable
environment isolation: owned by the host
```

This prevents “yolo” from disabling an embedding application’s explicit deny configuration.

Example configuration:

```toml
[execution]
mode = "yolo"

[shell]
enabled = true
default = "allow"
timeout_seconds = 300
max_model_output_bytes = 65536        # inline in the tool result; the rest stays behind a retained-output handle
max_retained_output_bytes = 16777216

deny = [
  "sudo *",
  "shutdown *",
]
```

Policy precedence should be deterministic:

1. An explicit deny wins.
2. If an allowlist is non-empty, a request must match it.
3. Otherwise the configured default applies.
4. An allowed operation executes immediately.

The exact matching language must be documented. Prefer a small, predictable matcher over an elaborate pseudo-shell parser. Record the rule ID that decided each call.

### 14.4 Security limitation

A command denylist cannot safely contain arbitrary shell behavior. For example, `python script.py`, `bash script.sh`, a package-manager hook, or a downloaded executable can perform actions not visible in the outer command string. Configuration is useful for application policy and accidental misuse but must not be marketed as secure isolation.

For untrusted work, run `pablo` inside a sandbox with explicit filesystem, process, credential, and network boundaries.

## 15. MCP design

### 15.1 Role of MCP

MCP is the standard boundary for discovering and invoking live external capabilities and accessing contextual resources. It should be a first-class input to the unified capability registry.

MCP is not the top-level client/agent or subagent protocol for `pablo`. Native ACP defines local sessions, prompts, updates, and cancellation; native A2A defines remote-agent tasks and artifacts. MCP describes capability exchange with tool/resource/prompt servers and does not replace either agent lifecycle. All three may use JSON-RPC machinery in selected transports without conflating their roles.

### 15.2 Implementation choice

Use the official Rust MCP SDK where it provides the necessary client support rather than manually rebuilding the protocol. At the time of this context dump:

- Official repository: `https://github.com/modelcontextprotocol/rust-sdk`
- Crate/documentation name: `rmcp`
- The SDK supports client and server roles and current MCP capabilities.

Pin an audited release and keep protocol version negotiation explicit. Avoid relying on draft-only features in the critical path unless they are isolated behind capability checks.

### 15.3 Version 0.1 MCP scope

The version 0.1 release surface is:

- MCP client lifecycle and capability negotiation.
- Stdio transport for local servers.
- Streamable HTTP transport for remote servers.
- Tool discovery and calls.
- Bounded tool results, progress, cancellation, request/operation timeouts, and safe errors.
- Environment and header-based credentials.
- Qualified names, collision handling, static server/tool allow/deny configuration, and trace spans without credential leakage.

The following MCP capabilities remain native design targets but are not version 0.1 gates:

- Resources and resource templates.
- Prompts.
- Roots where required.
- Catalog-change notifications and the long-tail capability dispatcher.
- Structured text, image, audio, and resource content without prematurely flattening everything to text.
- Server health and one bounded restart policy.
- Rich interactive OAuth UX.
- General MCP sampling initiated by servers.
- Durable MCP tasks.
- Complex form or URL elicitation.
- Legacy HTTP+SSE compatibility unless real target servers require it.

The internal event and waiting-state model should leave room for elicitation and long-running tasks so adding them does not require a protocol redesign.

### 15.4 Dynamic capability selection

An application may configure hundreds or thousands of MCP tools. Advertising every schema on every request wastes context, harms tool selection, and destroys prompt-cache stability: on several providers the tool block is the first part of the cached prefix, and any change to it invalidates everything after it.

Use progressive disclosure that never changes the advertised tool array within a turn:

1. Discover and cache server and tool metadata per server; invalidate only on catalog-change notifications or reconnection.
2. Advertise a bounded, host-chosen default set as native tools. This set is fixed for the session unless the host changes it.
3. Advertise three stable dispatcher tools for the long tail. `capability.search` returns matching qualified identities with one-line descriptions. `capability.describe` returns the full JSON Schema of named capabilities as a tool result. `capability.call` invokes a qualified capability with arguments that the runtime validates against the real schema before dispatch, returning bounded validation errors for repair.
4. Optionally promote frequently used catalog tools into the native array at the next turn boundary, when the host prefers native schema guidance for them and accepts one cache write.
5. Apply catalog-change notifications at a turn boundary, never mid-turn.

The dispatcher trades some provider-side argument guidance for a prefix that never changes. The runtime's schema validation and repair loop recovers most of the difference, and the trace records which path each call used. Section 28.4 holds the cache contract.

Tool identities should be stable and collision-resistant, for example:

```text
mcp:slack/search_messages
mcp:linear/create_issue
mcp:graphline/get_loan_snapshot
```

Provider-facing names may need a sanitized form such as `mcp__slack__search_messages`, while traces preserve the canonical identity.

### 15.5 MCP configuration

Illustrative configuration:

```toml
[mcp]
default = "allow"
deny_tools = ["production-db/delete_*" ]

[[mcp.servers]]
name = "filesystem"
transport = "stdio"
command = "mcp-server-filesystem"
args = ["/workspace"]
required = true
startup_timeout_ms = 30000
operation_timeout_ms = 60000

[[mcp.servers]]
name = "crm"
transport = "http"
url = "https://example.com/mcp"
bearer_token_env = "CRM_MCP_TOKEN"
required = false
```

Credentials must be resolved outside model-visible configuration. The model should see capability metadata, never bearer tokens, environment secrets, or unmasked authentication errors.

### 15.6 MCP trust and safety

- MCP tool descriptions, prompt results, resources, and tool results are external data. They do not override host policy or user authority.
- A skill or MCP prompt cannot grant itself additional tools.
- Workspace-provided MCP configuration can execute local commands and therefore has the same power as code. The host must decide whether project configuration is trusted.
- Static yolo execution does not remove this trust issue; it only removes approval prompts.
- MCP tool results should be bounded, typed, and redacted before entering model context or traces.
- Unknown MCP tools should default to a `dynamic` effect classification unless the host supplies stronger metadata.
- Server requests for sampling or human input must flow through the same budget, trace, and host-control layers as native requests.

### 15.7 MCP observability

Trace at least:

- Server identity, source, transport, and negotiated protocol version.
- Connection, initialization, health, restart, and shutdown.
- Catalog generation and change notifications.
- Tool/resource/prompt identity.
- Request and response IDs.
- Start/end/duration.
- Result content types and byte counts.
- Truncation and redaction.
- Progress.
- Authentication-required state without credentials.
- Protocol errors separately from tool-execution errors.
- Static policy decision and deciding rule.

MCP instrumentation must follow the applicable OpenTelemetry MCP semantic conventions. Propagate configured OTel context through MCP request and notification `params._meta` using `traceparent`, `tracestate`, and filtered `baggage`. Emit MCP client spans with `mcp.method.name`, `mcp.protocol.version`, `mcp.session.id`, JSON-RPC identifiers/status, network transport, and applicable `gen_ai.tool.*` or `gen_ai.prompt.*` attributes.

An MCP tool call should produce one logical tool span. When the MCP span already represents the GenAI tool execution, add `gen_ai.operation.name = "execute_tool"` and the applicable tool attributes to it rather than emitting a duplicate nested `execute_tool` span. Underlying HTTP transport spans may still exist and should be correlated normally.

## 16. Skills design

### 16.1 Adopt the Agent Skills format

Do not invent a proprietary skill package format. Implement the open Agent Skills specification:

- Specification repository: `https://github.com/agentskills/agentskills`
- Required file: `SKILL.md`
- YAML frontmatter containing at least a name and description.
- Markdown instruction body.
- Optional scripts, references, assets, templates, schemas, and other resources.

Canonical shape:

```text
my-skill/
  SKILL.md
  scripts/
  references/
  assets/
```

### 16.2 Progressive disclosure

Skills should use three disclosure levels:

1. **Catalog:** Load only name, description, source, and identity for every available skill.
2. **Activation:** Load the complete `SKILL.md` body when the user explicitly invokes the skill or the task clearly matches it.
3. **Resources:** Load referenced scripts, documents, and assets only when the activated instructions require them.

This allows many installed skills without paying the context cost of all instructions on every run.

An activated `SKILL.md` body enters the conversation as a message at the point of activation rather than being spliced into the system prefix, so the cached instruction prefix stays stable across steps (section 28.4). Host-pinned skills declared at session start may live in the prefix.

### 16.3 Activation

Support:

- Explicit CLI activation: `pablo --skill pdf-processing "..."`.
- Explicit interactive activation: `/pdf-processing` or an equivalent menu action.
- Host-pinned activation in `RunSpec`.
- Automatic matching from task to skill description. The default is model selection from the stable catalog of names and descriptions, which adds no extra model call or network round trip; a host may enable a lexical prefilter for very large catalogs or supply its own matcher (section 28.9).

Activation order must be deterministic. Name collisions should never silently choose one skill. Resolve with a qualified source identity or report ambiguity.

### 16.4 Skill roots and scopes

Likely roots include:

- Skills bundled with the `pablo` distribution.
- User skills, preferably under `~/.agents/skills/` or the platform config equivalent.
- Workspace skills under `.agents/skills/`.
- Explicit roots passed by the embedding host.
- Optional compatibility roots for established Claude, Codex, or other client layouts.

The Agent Skills format does not mandate one install location. The runtime should support the cross-client `.agents/skills/` convention and make additional roots explicit and inspectable.

### 16.5 Skills and shell

Bundled scripts execute through the ordinary shell capability. They receive no hidden elevated path. This gives one consistent process lifecycle, trace, output bound, cancellation, and policy mechanism.

Skills may document required programs, network access, MCP servers, or environment expectations. Optional namespaced metadata can improve diagnostics, but `pablo` should not make a proprietary metadata extension mandatory for basic compatibility.

### 16.6 Skills and MCP

A skill can explain how to use MCP tools and may declare expected server/tool identities in optional metadata. Activation can make those MCP capabilities strong candidates for dynamic selection.

The relationship is:

- Skill: how to perform a workflow.
- MCP: which live systems and data are available.
- Shell: which local programs and scripts can execute.
- Host policy: what is actually allowed.

A skill never grants permission merely by naming a tool.

### 16.7 Skill integrity and portability

Every activated skill should be identified by:

- Name.
- Source and path/URI.
- Optional author-provided version.
- Content digest of `SKILL.md`.
- Digests of every referenced resource actually loaded.

If remote installation is added, maintain a separate lock record mapping source to immutable revision/content digest. Do not burden the portable skill format with a required proprietary manifest.

Resource resolution must reject path traversal and unintended symlink escape. Installation and updates are meaningful filesystem mutations and should be explicit commands even though execution is yolo once configured.

### 16.8 Skill observability

Emit and trace:

- Catalog roots scanned.
- Discovery diagnostics.
- Candidate metadata.
- Activation source: explicit user, host-pinned, or automatic match.
- Match score/reason when automatic.
- Exact content digest.
- Instructions loaded and their context cost.
- Resources read.
- Scripts executed through shell.
- Required capabilities missing or denied.
- Completion or failure.

The TUI should make active skills visible and allow the developer to inspect the instructions contributing to the current turn.

## 17. Subagents as first-class citizens

### 17.1 Definition

A subagent is an independently addressable child agent supervised within a root run or session. It is not an opaque helper function and not merely one nested model request.

Every subagent has:

- A stable agent ID.
- A root and parent identity.
- Its own message history and context budget.
- Its own model loop.
- Its own active tools, MCP capabilities, and skills.
- Its own lifecycle and cancellation state.
- Its own events and trace spans.
- Its own usage and cost accounting.
- Its own output contract and terminal outcome.
- An optional durable session for future messages.
- A native protocol binding: ACP for a local child, A2A for a remote proxy (section 11.6).

The root agent is the root node of the same agent model; there are no separate root and child execution engines. "Subagent" is the supervisor's product concept, not a wire protocol.

### 17.2 Why subagents belong in version 0.1

Non-coding workflows frequently contain independent or specialist work: analyzing document groups in parallel, extraction challenged by a second agent, separating research from synthesis from artifact production, giving one child a specialist skill and a narrow toolset, keeping persistent domain specialists across a long conversation, or delegating a bounded task without flooding the parent with intermediate tool results.

If hierarchy, identity, and budgets are added after the public runtime types, ACP extensions, and trace schema stabilize, they will require invasive changes. First-class support does not require a swarm planner; it requires making agent identity recursive from the start.

### 17.3 Operations

Version 0.1 implements `spawn`, `wait` for any/all, `inspect`, and `stop` for temporary children. The full operation set remains:

- `spawn` or `run`: create one bounded temporary child for one complete task.
- `message`: create or continue a named persistent child and send its next message.
- `send`: send follow-up input or steering to an existing child without creating another identity.
- `wait`: wait for one, any, or all specified children, with a bounded timeout.
- `stop`: cancel the current child turn or close the child according to the request.
- `list`: enumerate children visible to the caller.
- `inspect`: obtain a child snapshot, status, budgets, and trace identity.

The model sees these through one built-in `subagent` tool with an action union; host SDKs expose the same operations directly so delegation need not originate with the root model. A synchronous `run` helper may combine create and wait, but the child's identity and event stream are published immediately either way. Section 11.6 defines how each operation maps onto ACP sessions and A2A tasks; `wait`, `list`, and `inspect` are supervisor-local views over recorded protocol state.

### 17.4 Identity and ownership

Use a tree, not an unowned global pool:

- Every child has exactly one immutable parent at creation and records its root owner.
- A child may create descendants only below the configured maximum depth.
- Names for persistent children are stable within the parent's namespace; IDs are globally unique even when names repeat under different parents.
- A child cannot be reparented silently.
- Host APIs authorize child access through the root/owner identity.
- A remote A2A child is a locally owned proxy node; the remote service's internal agents are not claimed as part of the local tree unless it reports interoperable child relationships.

Persistent children may be hidden from ordinary root-session discovery while remaining visible in the parent's tree. If direct child resume is later supported, it must preserve owner authorization rather than turn the child into an unrelated root session.

### 17.5 Context and instruction inheritance

Do not copy the parent transcript into a child by default. That is expensive, leaks irrelevant data, and destroys specialization. A child receives:

- Runtime invariants.
- The host's trusted system and developer instructions.
- The effective static capability ceiling.
- A bounded child-specific task.
- An optional child-specific instruction overlay, appended after the shared layers so siblings share one cached prefix (section 28.4).
- Only the parent context attachments explicitly selected for delegation.
- Relevant skill catalog and MCP capability metadata within its authority.

For local children these values accompany standard ACP session and prompt content through negotiated metadata. For remote children, transmit only what can be represented as bounded A2A Parts, referenced files, structured data, and negotiated extensions; never serialize a complete `RunSpec` or local authority policy to an untrusted remote.

The overlay may specialize behavior but cannot replace the trusted base prompt or widen authority. Parent-to-child context transfer is visible and fingerprinted. The parent receives only a bounded terminal result, artifact references, and status metadata; full child history stays available through trace and inspection APIs and is never injected into the parent context automatically.

### 17.6 Capability and authority inheritance

Effective local-child authority is the intersection of host/root static policy, parent effective authority, and child-specific restrictions. A child may narrow but never widen: shell access and command rules, filesystem roots and write access, MCP servers and capabilities, host tools, skills and skill roots, credential scopes, network boundary declarations, model and provider choices, and step, token, time, cost, and concurrency budgets.

Yolo semantics apply inside that effective set: an allowed child action executes without a prompt, and a parent cannot escape a deny by delegating the denied action. A remote A2A child is a separate authority domain with only the local-side controls described in section 11.6.

### 17.7 Models and specialization

By default a child inherits the parent's provider, model, reasoning level, and provider options. The host may allow a parent or `SubagentSpec` to choose from an approved set of alternatives, which enables a fast inexpensive model for broad extraction, a stronger model for synthesis or adversarial review, a vision-capable model for image-heavy evidence, or a specialist skill with a narrow catalog. Every override is explicit in the child trace, and fallback cannot escape the host-approved set.

### 17.8 Workspace semantics and shared mutation

The child workspace mode is explicit:

- `shared`: the parent's workspace.
- `subdirectory`: a named subtree enforced by static policy.
- `snapshot`: a read-only or copy-on-write snapshot supplied by the host.
- `external`: a separate host-supplied workspace.

Only `shared` may be necessary for the first implementation, but the contract must not assume all children share mutable files forever.

Concurrent agents should not casually edit the same files. Prefer read-only shared inputs, per-child subdirectories or copy-on-write snapshots, explicit artifact publication, one designated merger or deterministic host merge step, and typed host tools for canonical external writes. If shared mutation is enabled, the runtime exposes it as a risk and records overlapping paths; it does not claim transactional semantics it cannot provide. An execution dependency does not make external actions idempotent, so downstream nodes receive delivery-certainty and idempotency information for consequential upstream calls.

### 17.9 Concurrency and scheduling

Version 0.1 has one root, depth-one children, one bounded queue, a maximum of two active children by default, and no descendant waits, so it needs no general wait-graph or ancestor-capacity algorithm. The fuller supervisor design is:

The supervisor enforces maximum active children per parent, maximum active descendants per root, maximum total children per root turn or session, maximum tree depth, and a bounded pending queue, with fair scheduling so one child cannot starve siblings and cancellation-aware queue removal. It must prevent parents and children deadlocking while all slots are occupied and waiting on descendants; one practical rule is to reserve capacity for ancestors, or to disallow a child from synchronously waiting on a descendant when no runnable slot can become available. The wait graph is exposed separately from the ownership tree so deadlocks are diagnosable.

### 17.10 Hierarchical budgets

Version 0.1 uses one atomic root ledger plus per-child counters and hard child ceilings. Hierarchical reservations and release across recursive descendants begin when recursive delegation does. The end-state contract is:

Child work counts against root budgets: each child records independent usage while totals roll up through every ancestor. Budget types include model requests, agent steps, input, output, and cache tokens, monetary cost, wall-clock deadline, tool calls, shell process count and runtime, MCP requests, artifact and trace bytes, and child count and depth.

Reserve a bounded budget before starting a child so concurrent children cannot all observe the same remaining allowance and overspend it; release unused reservations when a child settles. A child-specific limit may be lower than its reservation. Budget exhaustion stops or fails only the appropriate subtree unless root policy says otherwise, and the root trace shows child-level and rolled-up consumption.

### 17.11 Messaging and results

Messages are explicit protocol records, not mutations of another agent's prompt buffer: ACP prompt, content, and update records locally; A2A Messages and Parts remotely. The supervisor may index them into one normalized ledger while retaining the original protocol identity and payload kind. Each message carries a message ID, sender and recipient agent IDs, root/session identity, creation and delivery sequence, content parts or references, an optional reply-to or correlation ID, visibility and sensitivity classification, delivery and acknowledgement state, and its ACP or A2A identity.

Child outcomes are structured: status, bounded final text or structured output, artifact references, usage and cost summary, trace/span reference, validation result, and safe failure information. The parent never scrapes prose to learn whether a child succeeded. Use ACP content blocks and A2A structured-data Parts for JSON Schema-constrained results, and ACP resource references or A2A Artifacts and File Parts for larger deliverables; `pablo` adds only a schema digest and validation outcome in negotiated metadata rather than a second handoff envelope.

### 17.12 Temporary and persistent children

Version 0.1 implements temporary children only. The persistent-child design below is preserved so the initial IDs and ownership model do not block it later, but it does not create storage or recovery work for the first release.

A **temporary child** receives one bounded ACP prompt or A2A task, exists for one run, returns one terminal outcome, is normally cancelled when its parent or root is cancelled or closes, and retains trace history according to root policy but no resumable conversation.

A **named persistent child** is created on the first message to a new name, has its own durable session and history, continues that session on later messages, may receive a replacement overlay while preserving trusted base instructions and owner identity, remains registered under its parent until explicitly removed or retention expires, and can never become an ownerless background process. It retains an ACP session locally or an A2A context binding remotely; an individual remote task may still settle between messages. The host chooses whether persistent children may outlive an individual parent turn; they do not outlive the owning root session unless the host explicitly supports that lifecycle.

### 17.13 Failure behavior

- One child failure does not automatically fail siblings or the parent; the parent receives a typed failure and decides whether to retry, replace, or continue.
- Parent process death is recoverable from persisted child and session state, or the supervisor marks in-flight children interrupted on restart.
- A child's possibly delivered external action retains the same delivery-certainty semantics as a root action; remote A2A cancellation, retries, and side effects are delivery-uncertain whenever the peer cannot prove the terminal state.
- Retries preserve or deliberately replace child identity according to policy and are visible in traces.
- A lost local ACP connection triggers bounded reconnection or session load when supported; otherwise the turn becomes interrupted rather than silently replayed.

Cancellation propagation across the tree follows section 13.5.

### 17.14 Events and observability

Child activity arrives in the ordinary `RunEvent` envelope (section 12.6) with agent, parent, root, source protocol, and original peer identities. The runtime provides both a multiplexed root stream containing the full tree and a filtered stream or snapshot for one agent. Trace views show the agent tree and lifetimes, delegation tasks and selected context, per-child model, tool, skill, and MCP activity, inter-agent messages and waits, budget reservation and usage, outcomes returned to parents, cancellation propagation, and per-protocol session, task, and remote-reported state. The TUI renders a live tree and lets the developer expand one child without flooding the root transcript.

### 17.15 Skills, MCP, and subagents

- A skill may recommend or structure delegation but cannot create more authority or budget than the caller has.
- A parent can activate a specialist skill for a child without loading it into the parent context.
- MCP connections may be pooled, but every request is attributed to the calling child and evaluated against its effective capability set.
- An MCP server's sampling request is not automatically a subagent; if supported later it remains a separately classified nested model operation unless explicitly promoted into an `pablo` agent session.
- Shell processes belong to the child that created them and follow its lifetime policy.
- A configured third-party ACP agent can serve as a local child without becoming an MCP server, and a configured A2A peer can serve as a remote child without being flattened into a tool call.

### 17.16 Configuration

The `[subagents]` and `[a2a]` tables in section 20.2 carry the illustrative keys. The runtime provides bounded defaults, and hosts may reduce limits to zero to disable delegation for a run. ACP is always the local protocol and is not configurable to a proprietary alternative. Remote A2A endpoints and Agent Cards are host-configured or resolved through a host-approved registry; model output alone cannot authorize a new remote endpoint.

### 17.17 What first-class does not mean

Version 0.1 does not need a proprietary role-planning language, a fixed researcher/writer/reviewer pipeline, a global autonomous swarm, cross-tenant child pools, hidden children created by provider behavior, or a second workflow engine. Provide reliable primitives and let the root model, host application, or skill decide when and how to compose them.

### 17.18 Agent chaining and efficient composition

Version 0.1 supports only the direct handoff and bounded fan-out/fan-in described in section 29.1. The general graph model is preserved below.

<details>
<summary>Show the preserved 0.x chaining design</summary>

#### Ownership tree and execution graph are different structures

Agent ownership should always remain a tree: every child has exactly one immutable parent and one root owner. Work dependencies may form a separate directed graph over those agents. For example, three sibling extraction agents can feed a fourth synthesis agent without any agent being reparented.

```text
Ownership tree                 Execution graph

root                           extract_a ───┐
├─ extract_a                   extract_b ───┼──> synthesize ──> review?
├─ extract_b                   extract_c ───┘
├─ extract_c
├─ synthesize
└─ review
```

An execution edge means "this node may consume that node's outcome" or "this node becomes runnable after that condition settles." It does not transfer ownership, credentials, authority, budget, or instruction priority. Every dependency and handoff is authorized through the common supervising root or host.

Bounded chains within one turn should be acyclic. Persistent specialists may exchange messages over a longer session, but the runtime must detect and reject synchronous wait cycles such as agent A waiting for B while B waits for A.

Useful identities include:

- `chain_id`: one logical composition attempt.
- `node_id`: one role in the composition, stable across a bounded retry when policy permits.
- `agent_id`: the concrete agent session executing that node.
- `edge_id`: one declared dependency or handoff.
- `attempt_id`: one execution attempt for a node.

Keeping these identities separate allows the runtime to replace a failed worker without pretending it is a different logical stage, and allows one persistent agent to execute multiple chain nodes over time without conflating its history with the chain definition.

#### Common composition patterns

The runtime should make the following patterns easy without defining proprietary researcher, writer, or reviewer roles:

- **Sequential pipeline:** one agent's validated result becomes the next agent's bounded input.
- **Fan-out/fan-in:** independent children process partitions concurrently, then one reducer or parent combines their results.
- **Router and specialist:** a cheap routing step selects only the specialist needed for the task.
- **Evaluator and repair:** a reviewer emits structured defects; the producing agent receives those defects for one bounded repair cycle.
- **Map/reduce:** one homogeneous task is mapped over many items under a concurrency window, then deterministically aggregated or summarized.
- **Conditional branch:** later work runs only when validation, confidence, disagreement, or policy indicates that it is necessary.
- **First valid or quorum:** several candidates run concurrently and the supervisor accepts the first valid result or waits for a configured agreement threshold.
- **Persistent specialist handoff:** a recurring domain agent retains useful history and receives small new work packets across root turns.

These are scheduling and data-flow patterns over ordinary agents. They must not require a second model loop, a privileged planner, or hidden provider behavior.

#### Use the least expensive primitive that preserves quality

Not every step should be an agent. The preferred decision order is:

1. Use deterministic code or a tool for parsing, arithmetic, validation, sorting, merging, and canonical writes.
2. Keep work in the current agent when it fits its context and does not benefit from isolation, a different model, or parallel execution.
3. Use a temporary child for bounded independent reasoning, context isolation, parallel work, or adversarial review.
4. Use a persistent child only when future tasks genuinely benefit from retained specialist history.
5. Add another layer of delegation only when its expected quality or latency benefit exceeds its model, context, scheduling, and failure overhead.

Shallow, bounded graphs should be the default. Deep chains compound latency, summarization loss, model error, and debugging difficulty. Wide fan-out can reduce wall-clock time but increases cost and provider-rate pressure. The runtime should make both consequences visible rather than presenting more agents as inherently better.

Known business workflows should normally be orchestrated by deterministic host code. A model should choose the graph only when decomposition or routing itself requires judgment. A skill may describe a reusable composition recipe, but durable workflow state, timers, business retries, and canonical side effects remain host responsibilities.

#### Typed handoffs

The supervisor may expose a normalized `Handoff` view containing:

- Chain, node, edge, source-agent, and source-trace identities.
- The downstream objective and explicit success criteria supplied by the supervisor.
- A validated structured result or bounded text projection.
- Content-addressed evidence and artifact references.
- Validation status, confidence only when meaningfully defined, unresolved questions, and safe failure information.
- Trust, sensitivity, and visibility labels.
- Output-contract and relevant input fingerprints.
- Byte and token estimates before the handoff enters another model context.

This is an in-memory/SDK convenience type, not a second wire envelope. A local handoff is encoded with ACP content blocks, resource references, and negotiated `_meta`; a remote handoff uses A2A structured-data/file Parts and Artifacts plus an optional negotiated extension. Reuse standard IDs and content forms wherever possible and add only missing chain/schema correlation fields. A downstream agent receives the smallest sufficient projection and may request another authorized reference if necessary; the projection rules in section 17.5 apply.

Large values live once in an artifact or host content store and move through the graph by immutable reference and digest (section 28.7). The runtime may materialize a referenced value into a child workspace when needed, and the trace preserves the source identity and verifies the digest, so there is no ambiguity about which evidence version was used.

A handoff is data, not a higher-priority instruction layer. Text produced by an upstream agent cannot grant authority or override the downstream agent's trusted host instructions. This remains true even when an upstream agent labels its output as a plan or system prompt.

#### Efficient scheduling

The supervisor should optimize the actual critical path rather than maximize simultaneous activity:

- Start nodes only when their declared dependencies are satisfied.
- Run only genuinely independent work concurrently.
- Use a bounded sliding window for large map operations rather than spawning every child at once.
- Route extraction and classification to an approved inexpensive model when evals support it; reserve stronger models for synthesis, ambiguity, or review.
- Cancel queued or speculative descendants as soon as their output can no longer affect the accepted result.
- Avoid automatic best-of-N or debate patterns. They multiply cost and often produce correlated answers unless the task and acceptance rule justify them.
- Persist a validated node outcome as a checkpoint so a downstream failure does not require replaying successful upstream model work.

Budget reservation (section 17.10), shared clients and cached prefixes (section 28.4 and 28.10), and event backpressure (section 28.6) apply to chains without restatement here.

Reuse must be deliberate. A persistent specialist saves repeated setup and domain context, but its growing history can make it slower, more expensive, and more prone to stale assumptions. The trace should expose retained-context size and compaction. A fresh temporary child is preferable when continuity provides no measurable benefit.

#### Dependency and join semantics

A downstream node should declare one explicit readiness rule:

- `all_valid`: run only when every required upstream result validates.
- `all_settled`: run with successes and typed failures after every dependency settles.
- `any_valid`: run after the first valid outcome and optionally cancel remaining producers.
- `quorum`: run when a configured number or proportion of acceptable outcomes exists.
- `always`: run for cleanup, reporting, or failure synthesis after dependencies settle.

The default should be `all_valid`. A failed dependency must not silently turn into missing context. The supervisor should either skip the blocked node with a typed reason, invoke an explicit fallback edge, or pass a typed partial-result envelope when the join rule permits it.

Transport retries, same-session output repair, replacement workers, and whole-chain business retries are separate policies. Only one layer should own each retry. A node repair may continue the same agent session with validator feedback; replacing a poisoned or interrupted worker creates a new `agent_id` and `attempt_id` under the same logical `node_id`.

#### Observability and tuning

The trace and TUI offer two complementary views: the ownership tree, meaning who created and controls each agent, and the execution graph, meaning which outputs, validations, and conditions enabled each node. For every chain, show the planned and actual graph, dynamic routing decisions, queue time, active time, critical path, model and tool configuration, input and output sizes, validation state, retries, cancellations, and cost per node, collapsing routine worker detail while keeping every event inspectable. Fan-in and cross-trace dependencies become OTel span links under the rules in section 21.6, with the full graph retained in the native trace.

Useful efficiency diagnostics include:

- Duplicate context bytes and tokens sent across children.
- Time and cost spent on results that were never consumed.
- Fan-out width, queue pressure, and provider throttling.
- Persistent-child context growth.
- Deterministic work performed by models that could become tools.
- Reviewer invocation rate and how often review changes the accepted result.
- Node cache/checkpoint reuse.
- Critical-path latency versus summed agent time.

These measurements let developers tune chains with evidence instead of intuition.

#### Concrete target shapes

An efficient Otto analysis might use bounded fan-out for independent document groups. Each worker returns schema-valid facts with evidence references. Deterministic code validates and merges the facts. A reviewer runs only on disagreements, missing evidence, or low-confidence fields, and the root synthesizes the final report. Raw extraction transcripts never enter the synthesis context.

An efficient Graphline turn might query several read-only specialists concurrently against one immutable loan-version snapshot, join their typed findings, and let the root explain or draft a proposal. A consequential action still crosses Graphline's typed proposal and approval boundary. Persistent specialists are used only when their retained domain history improves later turns; otherwise temporary children prevent stale context accumulation.

#### Version 0.1 boundary

Version 0.1 proves composition without implementing this general graph model. A root or host can start depth-one temporary children, wait for any or all, stop them, and pass one bounded validated outcome or artifact reference into a later child. That is enough for sequential work and bounded fan-out/fan-in in Otto. Chain, node, edge, checkpoint, conditional-scheduling, quorum, persistent-specialist, recursive-delegation, and cycle-analysis machinery remains the preserved 0.x design above. A declarative `ChainSpec` should be considered only after real integrations reveal a stable portable contract.

</details>

## 18. Host tools and consequential actions

Shell and MCP are powerful, but product applications also need typed tools whose implementation remains in TypeScript, Python, or another host language.

For out-of-process hosts, prefer exposing host tools through MCP configuration carried by the ACP session. This reuses MCP tool discovery, schemas, calls, results, progress, cancellation, and authentication instead of inventing a second reverse-tool protocol. An in-process Rust host may register the same normalized `Tool` directly.

If a direct host callback later proves necessary for TypeScript/Python latency or lifecycle reasons, define one capability-negotiated ACP reverse-request extension containing:

- Call ID.
- Qualified tool identity.
- Validated arguments.
- Run/session/trace context.
- Declared effect and presentation metadata.

Version 0.1 does not implement this extension: out-of-process hosts expose tools through MCP, and embedded Rust hosts register normalized tools directly. When added, an SDK invokes the registered host callback and returns a typed tool result. Unsupported generic ACP clients simply do not advertise this extension. Host tools enter the same model loop, policy, native OTel telemetry, and lossless trace as built-ins and MCP tools.

For applications like Graphline, distinguish:

- **Read tools:** retrieve canonical facts.
- **Proposal tools:** create a typed action proposal without executing its consequence.
- **Commit tools:** application-owned writers available only after the host’s own authorization and approval flow.

Version 0.1 does not need to implement a generic multi-user approval product. It must preserve the waiting/callback and effect contracts needed for the host to implement one.

Never require an application to expose a generic database or arbitrary “execute action” tool. Typed domain operations preserve validation, authorization, idempotency, audit, and useful tool responses.

## 19. Model-provider design

### 19.1 Initial gateways

Support:

- Vercel AI Gateway.
- OpenRouter.

Both provide HTTP APIs compatible with common OpenAI request shapes, but the runtime should not assume identical behavior. Model-level support for tools, structured outputs, reasoning, images, PDFs, usage, and provider options varies.

Relevant official documentation at the time of writing:

- Vercel AI Gateway SDKs and APIs: `https://vercel.com/docs/ai-gateway/sdks-and-apis`
- Vercel AI Gateway direct REST: `https://vercel.com/docs/ai-gateway/openai-compat/rest-api`
- OpenRouter quickstart: `https://openrouter.ai/docs/quickstart`
- OpenRouter model capabilities: `https://openrouter.ai/docs/guides/overview/models`

In Rust, call the gateway HTTP APIs directly. “Support Vercel AI” means Vercel AI Gateway at the provider boundary; the separate official TypeScript add-on provides Vercel AI SDK `UIMessage` stream compatibility at the application boundary.

### 19.2 Open Responses provider compatibility

Open Responses is the third native provider path in the version 0.1 package and default binary. It lets a developer point `pablo` at a conforming endpoint with the same provider-neutral model loop used by Vercel AI Gateway and OpenRouter. It may be implemented in a focused internal crate, but it is not feature-gated or separately installed.

The native implementation maps shared request, item, content, tool, streaming, usage, and finish semantics. It must still perform a capability handshake or configured capability resolution because accepting the wire format does not prove that every endpoint supports every modality, tool mode, reasoning option, schema feature, or extension. The runtime records both the requested Open Responses behavior and the endpoint’s resolved behavior.

Open Responses compatibility should reduce adapter duplication, not erase provider identity. Direct Vercel and OpenRouter adapters remain useful for gateway routing, catalogs, provider preferences, pricing, fallback, and fields that are not yet portable. The same normalized event must not be emitted twice when an Open Responses endpoint also has HTTP auto-instrumentation.

### 19.3 Shared transport, explicit capabilities

It is reasonable to share serialization and SSE machinery for compatible endpoints. Preserve per-gateway adapters for:

- Authentication and headers.
- Model catalog.
- Routing and provider options.
- Usage and cost fields.
- Reasoning configuration and returned state.
- Structured response support.
- Tool-call quirks.
- Error and retry semantics.
- Resolved provider identity.
- Prompt-cache mode, breakpoint limits, minimum cacheable length, TTL options, affinity-key support, and cache usage reporting (section 28.4).

Model capabilities should be discovered or represented explicitly rather than inferred only from a model-name pattern. The resolved capability record for each provider and model pair is recorded in the trace.

### 19.4 Credentials

Initial credential sources:

- `AI_GATEWAY_API_KEY` for Vercel AI Gateway.
- `OPENROUTER_API_KEY` for OpenRouter.
- Configured header or host-managed credential leases for Open Responses endpoints.
- Host-managed mode where the embedding application authenticates at its network boundary and the runtime sees no credential bytes.

The model, MCP servers, skill content, logs, and traces must not receive provider secrets.

### 19.5 Retry ownership

Gateways may perform routing and provider fallback. The runtime may retry model requests. The embedding workflow may retry an entire task. Without explicit ownership, one failure can multiply into many paid calls.

Each request should identify one retry owner and record delivery certainty:

- Definitely not sent: safe transport retry may be possible.
- Possibly sent or billed: repetition can duplicate cost or side effects and requires policy.

Record every attempt, route, fallback, generation ID, and usage result.

### 19.6 Good defaults

The out-of-box experience should require no provider framework knowledge:

```bash
export AI_GATEWAY_API_KEY=...
pablo "summarize the files in this directory"
```

or:

```bash
export OPENROUTER_API_KEY=...
pablo --provider openrouter "summarize the files in this directory"
```

The release should contain a tested default model/profile, while always showing and tracing the exact resolved model. Developers can override it through config, CLI, or `RunSpec`.

## 20. Configuration

### 20.1 Goals

Configuration should be:

- Small enough to understand.
- Fully inspectable with a command.
- Layered predictably.
- Serializable into traces after secret removal.
- Friendly to both local users and embedding hosts.

Suggested precedence, highest first:

1. Explicit values in the host `RunSpec` or CLI flags.
2. Environment variables.
3. Workspace configuration.
4. User/profile configuration.
5. Built-in defaults.

The resolved configuration and the source of each important value should be inspectable with `pablo config explain` or `pablo doctor`.

### 20.2 Illustrative configuration

This is the one complete example; other sections reference these tables rather than repeating them.

```toml
[provider]
name = "vercel"
model = "anthropic/claude-sonnet-4.6"
reasoning = "medium"
prompt_cache = "auto"           # auto | explicit | automatic | off
prompt_cache_ttl = "auto"       # auto | short | long

[execution]
mode = "yolo"
max_steps = 100
max_duration_seconds = 1800
max_cost_usd = 5.00
parallel_tool_calls = true
max_parallel_tool_calls = 8

[shell]
enabled = true
default = "allow"
max_model_output_bytes = 65536
max_retained_output_bytes = 16777216

[filesystem]
roots = ["."]
write = true

[subagents]
enabled = true
max_active_per_parent = 4
max_active_per_root = 8
max_children_per_turn = 16
max_depth = 2
default_workspace = "shared"
inherit_model = true
inherit_tools = true
inherit_skills = true
cancel_temporary_on_parent_exit = true
max_handoff_bytes = 65536
default_join = "all_valid"
checkpoint_valid_outcomes = true

[a2a]
enabled = true
allow_model_selected_agents = false
max_artifact_bytes = 16777216

[[a2a.agents]]
name = "research-service"
agent_card_url = "https://agents.example.com/research/.well-known/agent-card.json"
auth = "host-managed"

[skills]
auto_match = true
roots = [".agents/skills", "~/.agents/skills"]

[mcp]
default = "allow"
startup_grace_ms = 1500
max_concurrent_requests_per_server = 4
advertise = ["filesystem/*", "crm/get_*"]    # native tools; the rest go through the dispatcher

[trace]
enabled = true
content = "private"
directory = ".pablo/traces"
fsync = "turn"

[otel]
enabled = true
exporter = "none"
protocol = "http/protobuf"
content = "none"
```

Names and defaults are illustrative. Avoid committing secrets or machine-specific credentials in workspace configuration.

### 20.3 Static restrictions in yolo mode

Allow developers to restrict:

- Entire built-in tools.
- Shell command patterns.
- Working-directory roots.
- Filesystem read and write roots.
- Environment variables forwarded to commands.
- MCP servers.
- MCP tools/resources/prompts.
- Host tools.
- Network access only when the environment or an explicit proxy can enforce it.
- Per-tool time and result-size budgets.
- Child-agent depth, count, concurrency, model set, tools, skills, workspace mode, and hierarchical budgets.

Policy evaluation should produce an observable decision with a stable rule identity. Denied operations should return useful model-visible diagnostics so the agent can choose an allowed alternative.

## 21. Observability and visibility

Observability is a primary product surface, not merely logging.

### 21.1 Three audiences

Every event and trace field should consider three audiences:

- **User:** safe, understandable progress and results.
- **Operator/developer:** technical behavior, timing, configuration, and failures.
- **Sensitive audit:** content-bearing inputs, raw tool arguments/results, and other material requiring strict storage policy.

Suggested visibility classes:

- `user`
- `operator`
- `sensitive`

The host chooses which classes to retain and expose.

### 21.2 What a complete trace contains

- Run/session/turn identity.
- Root/parent/child agent identities, the complete ownership tree, and the logical execution graph.
- Parent/child spans and ordered sequence numbers.
- Provider and model requested.
- Provider and model resolved.
- Prompt layer fingerprints.
- Tool catalog fingerprint.
- JSON Schema/output contract fingerprint.
- Skill identities and loaded-resource fingerprints.
- MCP servers and negotiated capabilities.
- Input file metadata and optional content-addressed snapshots.
- Model request timing and delivery evidence.
- Model output and tool calls according to content policy.
- Shell command, working directory, output sizes, exit status, and duration.
- MCP requests, progress, results, and errors.
- Host-tool calls and results.
- Inter-agent messages, typed handoffs, dependency joins, waits, child outcomes, and cancellation propagation.
- Child budget reservations, actual usage, and rolled-up totals.
- Validation and repair attempts.
- Retry/fallback decisions.
- Tokens, cache counters, cost, and latency.
- Context size and compaction decisions.
- Artifacts.
- Cancellation and failure reasons.
- Final outcome.

### 21.3 Visibility is not chain-of-thought

Do not promise raw hidden reasoning. Expose:

- What the agent is doing.
- Which capability it selected.
- What inputs and evidence it used when policy permits.
- What changed.
- Which work was delegated, to which child, with what bounded context and authority.
- What the command or tool returned.
- Why the deterministic runtime retried, denied, compacted, or stopped.
- Cost, time, and budget state.

If a provider returns an explicit reasoning summary or opaque continuation state, preserve it according to provider terms and host policy. Do not synthesize or reveal private reasoning as a debugging feature.

### 21.4 Semantic activity metadata

Tools should carry presentation metadata such as:

- Active label: “Reviewing loan documents.”
- Completed label: “Reviewed loan documents.”
- Category/icon hint.
- Primary safe argument for display.
- Whether raw technical detail can be shown.

This avoids Graphline’s current need to infer user-facing progress from raw Bash arguments.

### 21.5 OpenTelemetry compatibility contract

OpenTelemetry is how `pablo` implements operational telemetry natively. Instrumentation is created where model calls, tools, agents, chains, MCP operations, queues, and failures occur; it is not reconstructed later by consuming `RunEvent`. The core uses the OTel API and data model even when no exporter is configured. Compatibility means:

- Use the OTel API and data model for trace, metric, log/event, resource, instrumentation-scope, status, and link semantics.
- Propagate W3C Trace Context by default and support configured OTel propagators.
- Follow applicable stable core semantic conventions.
- Follow the pinned OpenTelemetry GenAI and MCP semantic conventions for operations they define.
- Export traces, metrics, and logs through standards-compliant OTLP.
- Use standard `OTEL_*` configuration instead of proprietary alternatives when OTel already defines the setting.
- Keep every product-specific extension in a documented lowercase `pablo.*` namespace.
- Test emitted OTLP against a real OpenTelemetry Collector and checked semantic-convention fixtures.

The OpenTelemetry GenAI and MCP conventions are still marked Development. At this document’s 2026-09-03 baseline, the inspected GenAI registry is `https://opentelemetry.io/schemas/gen-ai-dev/1.42.0-dev` at repository commit `fee465db333bdd6a7d2faa320edab5cf3101a4f4`, depending on core semantic conventions `1.44.0`. Implementation must pin a tested tag, schema URL, or immutable revision, expose that revision in diagnostics, and upgrade deliberately. Do not silently track the repository’s default branch or describe Development conventions as stable.

If the GenAI registry publishes a usable schema URL for the pinned release, attach it to the instrumentation scope. Until a stable schema and migration path exist, retain an explicit internal mapping version so old native traces can still be projected according to the convention they were recorded against.

### 21.6 Trace boundaries and span topology

Use one OTel trace for one top-level run, workflow invocation, or conversational turn. Do not keep one trace open for an entire durable session. Correlate separate turns with the existing session identity through `gen_ai.conversation.id` when its semantics apply, plus `pablo.root_session.id` where the root ownership relationship is needed.

When `RunSpec` contains valid incoming trace context, the root operation uses it as a remote parent. A standalone CLI invocation without incoming context creates a new root trace. Background work that deliberately starts a new trace should link to the initiating span rather than inventing parentage across an unrelated lifetime.

Recommended mapping:

| `pablo` operation | OTel representation | Important behavior |
| --- | --- | --- |
| Application-visible chain or graph | `invoke_workflow {gen_ai.workflow.name}` `INTERNAL` span | Emit only for a real application-defined multi-agent workflow, not every internal delegation detail |
| One root or child agent turn | `invoke_agent {gen_ai.agent.name}` `INTERNAL` span | Each agent invocation owns only its direct model and tool counts; descendants own theirs |
| Reliably identified planning phase | `plan {gen_ai.agent.name}` `INTERNAL` span | Do not infer a plan span from ordinary hidden reasoning |
| Gateway model request | `{gen_ai.operation.name} {gen_ai.request.model}` `CLIENT` span | Cover the logical generation including internal transport retries; instrument physical HTTP attempts below it when useful |
| Built-in, shell, or host tool | `execute_tool {gen_ai.tool.name}` `INTERNAL` span | Add transport/process child spans only when another convention describes a distinct operation |
| MCP request or notification | OTel MCP client span | Add applicable `gen_ai.operation.name`, tool, or prompt attributes and suppress a duplicate logical tool span |
| Validation, repair, skill activation, queueing, checkpoint, or chain transition | Documented `pablo.*` span or event | Use a span only when the operation has meaningful duration; use an event for a point occurrence |

Local ephemeral child creation is not automatically an OTel `create_agent` operation. That convention primarily represents creation of a hosted agent resource. The child’s actual work is an `invoke_agent` span, while the spawn decision can be an event on the supervisor or the existing `execute_tool subagent` span.

Every span has one primary parent. Use normal parent/child relationships for the active causal or supervising path. Use span links, supplied at span creation when possible, for:

- Fan-in from several upstream agent outcomes.
- A handoff whose producer is not the primary parent.
- A resumed persistent child’s prior turn.
- Checkpoint reuse from an earlier trace.
- Work moved to a new trace because it outlives the initiating request.

Attach `pablo.chain.edge.id` and a documented low-cardinality relationship type to a link when the SDK supports link attributes. The complete dependency graph remains in the native trace because OTel backends differ in how well they render links.

Follow OTel error semantics. Successful spans leave status unset. A terminal failure sets status to `ERROR` and a predictable low-cardinality `error.type`. A handled retry should not make the enclosing logical operation an error when it ultimately succeeds, although a failed physical attempt span may be an error. Intentional cancellation is recorded as an `pablo` outcome and should not automatically be classified as an error.

### 21.7 Standard and `pablo` attributes

Use standard attributes whenever their defined semantics match. Important GenAI fields include:

- `gen_ai.operation.name`
- `gen_ai.provider.name`
- `gen_ai.request.model`
- `gen_ai.response.model`
- `gen_ai.response.id`
- `gen_ai.response.finish_reasons`
- `gen_ai.conversation.id`
- `gen_ai.agent.name`
- `gen_ai.workflow.name`
- `gen_ai.tool.name`
- `gen_ai.tool.type`
- `gen_ai.tool.call.id`
- `gen_ai.usage.input_tokens`
- `gen_ai.usage.output_tokens`
- Applicable cache, modality, and reasoning token attributes
- `server.address`, `server.port`, and `error.type`
- Applicable `mcp.*`, `jsonrpc.*`, and `network.*` attributes

`gen_ai.workflow.name` and `gen_ai.agent.name` must be meaningful low-cardinality logical names such as `otto_analysis` or `document_extractor`, not run IDs, user prompts, generated task titles, or UUIDs. `gen_ai.agent.id` is reserved for the stable hosted-agent resource meaning defined upstream; an ephemeral `pablo` runtime agent ID belongs in `pablo.agent.id`.

Proposed `pablo` extensions include:

- `pablo.run.id`, `pablo.turn.id`, and `pablo.root_session.id`
- `pablo.agent.id`, `pablo.agent.parent.id`, `pablo.agent.root.id`, and `pablo.agent.kind`
- `pablo.chain.id`, `pablo.chain.node.id`, `pablo.chain.edge.id`, `pablo.chain.join.type`, and `pablo.attempt.id`
- `pablo.tool.origin`, `pablo.skill.name`, `pablo.skill.digest`, and `pablo.mcp.server.name`
- `pablo.output.contract.digest`, `pablo.prompt.layers.digest`, and `pablo.tool.catalog.digest`
- `pablo.prompt.prefix.digest`, `pablo.cache.hit_ratio`, and `pablo.cache.break.reason`
- `pablo.policy.decision`, `pablo.policy.rule.id`, and `pablo.visibility`
- `pablo.retry.owner`, `pablo.delivery.certainty`, and `pablo.validation.outcome`
- `pablo.cost.amount` and `pablo.cost.currency` until an applicable standard cost convention exists
- `pablo.trace.format.version` and `pablo.otel.mapping.version`

Names, types, allowed values, cardinality, sensitivity, and stability of every `pablo.*` field must be published as part of the protocol documentation. Never emit the same concept under both a standard key and a custom alias. Provider-specific identifiers remain in their standard provider namespace when one exists.

For Vercel AI Gateway and OpenRouter, distinguish the gateway endpoint from the upstream model provider. Populate `gen_ai.provider.name` according to the pinned convention’s provider meaning for the API being instrumented, use `server.address` for the contacted gateway, and preserve gateway plus resolved upstream routing in documented `pablo.*` attributes when no standard attribute represents both. Do not label a gateway call as direct Anthropic or OpenAI merely because the model originated there.

### 21.8 Metrics

Emit the applicable standard GenAI metrics when their inputs are known rather than estimated:

- `gen_ai.client.token.usage`
- `gen_ai.client.operation.duration`
- `gen_ai.client.operation.time_to_first_chunk`
- `gen_ai.client.operation.time_per_output_chunk`
- `gen_ai.invoke_workflow.duration`
- `gen_ai.invoke_agent.duration`
- `gen_ai.invoke_agent.inference_calls`
- `gen_ai.invoke_agent.tool_calls`
- `gen_ai.execute_tool.duration`

Emit applicable `mcp.client.operation.duration`, `mcp.client.session.duration`, and corresponding server metrics only for sides the runtime actually instruments. Agent inference/tool-call metrics count direct calls exactly once and exclude calls owned by descendants.

Product-specific metrics may cover active agents, queued children, queue delay, handoff bytes, validation/repair counts, budget exhaustion, trace drops, exporter failures, and monetary cost. Custom metric names use the `pablo.*` namespace and documented UCUM units.

Metric attributes must remain low-cardinality. Never attach run, session, trace, span, agent-instance, chain, tool-call, request, artifact, file, prompt, command, URL, or user identifiers to metric points. Do not attach raw error messages; use a bounded documented `error.type`. Exemplars may provide trace correlation when the SDK and backend support them.

### 21.9 Events, logs, and content

The native `RunEvent` stream is more detailed than OTel should be. Map significant point-in-time lifecycle changes to span events or correlated OTel log records, but do not export every token, text delta, terminal byte chunk, or progress repaint by default. Record aggregate chunk/byte counts and timing on the enclosing span while the native trace retains replay fidelity.

Use the standard `gen_ai.client.inference.operation.details` event for opted-in model request/response detail and `gen_ai.evaluation.result` for evaluation results when their contracts apply. Other event names use `pablo.*`. Correlated JSON logs carry lowercase hexadecimal `trace_id`, `span_id`, and `trace_flags` in the standard locations. Record an unhandled exception once rather than duplicating it on every enclosing layer.

Model instructions, messages, tool arguments/results, MCP resources, skill bodies, shell commands/output, and handoffs may contain secrets or personal data. They are never exported to OTel by default. Content capture requires a separate explicit opt-in from metadata telemetry and must follow the structured GenAI schemas when standard content attributes are used. Where an SDK cannot represent complex attributes, serialize the standard object as valid JSON only where the pinned convention permits it.

For production, prefer storing content in a host-controlled content store and attaching redacted, immutable references and digests to `pablo.*` attributes. OTel export permissions and retention may differ from private trace storage, so enabling private native traces must not implicitly enable OTel content export.

### 21.10 Context propagation

The cross-language boundary must carry explicit OTel context fields through negotiated namespaced ACP metadata containing:

- `traceparent`
- Optional `tracestate`
- Filtered `baggage`
- Optional span-link contexts for asynchronous or fan-in relationships

TypeScript, Python, and Rust callers extract from their active context and inject it through their ergonomic `RunSpec` API; the SDK maps it to ACP metadata, and response updates expose the resulting trace/span identifiers for correlation. Host-tool callbacks propagate the active tool context back to the host. A separately hosted `pablo` runtime treats extracted context as remote. A2A propagation uses the minimal documented extension and degrades to local correlation when a peer does not negotiate it.

Subagents inherit the active OTel context through the supervisor. MCP injects configured propagators into `params._meta` as required by the OTel MCP convention. Provider HTTP instrumentation uses the active model-call span so transport spans correlate without replacing the logical GenAI span.

Do not inject trace or baggage environment variables into arbitrary shell commands by default. A trusted instrumented subprocess may opt into an explicit environment carrier. This avoids leaking tenant or telemetry metadata to untrusted programs.

Baggage is not an authorization channel and never enters model context. Propagate only allowlisted, bounded, non-secret keys across each trust boundary. Clear unknown baggage before sending work to an untrusted MCP server, sandbox, host callback, or child process. Never place credentials, prompts, document content, user identifiers, or unbounded task names in baggage.

### 21.11 OTLP export and OTel configuration

Version 0.1 includes OTLP/HTTP with binary Protobuf for traces in the default `pablo` distribution. Metrics and logs pipelines are post-0.1. This stages signal breadth without changing the decision that lifecycle instrumentation, context, identities, and semantic conventions are OTel-native. OTLP/gRPC may be a Cargo feature or separate distribution until its binary-size and dependency cost justify inclusion in the ordinary binary. Vendor-specific exporters are unnecessary because an OTel Collector can route OTLP to backends.

Honor applicable standard configuration, including:

- `OTEL_SDK_DISABLED`
- `OTEL_SERVICE_NAME`
- `OTEL_RESOURCE_ATTRIBUTES`
- `OTEL_PROPAGATORS`
- `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`
- `OTEL_TRACES_EXPORTER`, `OTEL_METRICS_EXPORTER`, and `OTEL_LOGS_EXPORTER`
- `OTEL_EXPORTER_OTLP_ENDPOINT` and signal-specific endpoint overrides
- `OTEL_EXPORTER_OTLP_PROTOCOL` and signal-specific protocol overrides
- OTLP header, compression, timeout, TLS, batching, and temporality settings supported by the Rust SDK
- `OTEL_CONFIG_FILE` when the selected Rust OTel SDK supports the declarative configuration specification

OTel-specific precedence and endpoint construction must follow the OpenTelemetry specifications, including signal-specific overrides. Do not reinterpret these variables through general `pablo` configuration precedence. A local `[otel]` configuration can provide explicit product defaults, but standard environment variables and an injected host SDK must retain their defined behavior.

No network exporter is enabled by default. When run as a standalone process, use `service.name = "pablo"` unless the operator supplies `OTEL_SERVICE_NAME`, attach the `pablo` version as `service.version`, and populate only resource fields the runtime knows reliably. When embedded as a Rust library, do not overwrite the host’s resource or global providers. Instrumentation scope name is `pablo`, scope version is the runtime version, and the pinned semantic-convention schema URL is attached when available.

Use bounded batch processors and the OTLP retry behavior defined by the exporter specification. Export failure must never change model/tool behavior, enter model context, or block shutdown indefinitely. For bounded CLI runs, flush with a short configurable deadline and report dropped telemetry through stderr and the native diagnostic stream without corrupting structured stdout.

Exporter headers and certificates are credentials. Resolve them outside model-visible config, mask them in `pablo doctor`, and never serialize them into native traces.

### 21.12 Native trace and OTel relationship

Initial observability surfaces should include:

- Versioned lossless JSON Lines native trace.
- In-memory callback/event stream.
- Human-readable `pablo trace show` rendering.
- OTel traces, metrics, and correlated logs.
- OTLP/HTTP Protobuf export.

Create native OTel instrumentation and the lossless trace record in the same internal lifecycle transition rather than instrumenting separate execution paths or deriving OTel spans asynchronously from presentation events. Reuse OTel trace/span IDs and exact timestamps in native events so an operator can move from an OTel backend to `pablo trace show`. Store native event IDs and sequence numbers separately because they have different semantics from span IDs.

The native trace remains authoritative for ordered streaming deltas, agent ownership, dependency graphs, handoff payload references, artifacts, budget reservations, provider delivery certainty, checkpoints, and deterministic replay. OTLP is authoritative only for the exported operational signals. Do not promise that an OTLP trace can reconstruct a native replay tape.

Native embedding accepts injected `TracerProvider`, `MeterProvider`, and `LoggerProvider` implementations. The library does not replace process-global providers or install a global subscriber behind the host’s back. The standalone CLI owns and configures its SDK instance. Instrumentation detects or configures suppression when host/provider/MCP libraries already emit the same logical operation so automatic instrumentation does not create duplicate spans and metrics.

### 21.13 Sampling, privacy, and cardinality

Content and export defaults are defined in sections 21.9 and 21.11: no network export by default, no content in OTel by default, and native trace content configured independently. In addition:

- Secrets are removed before values reach the OTel SDK or native serializer, not only hidden in the TUI.
- Every exported trace is reviewable and redactable before sharing; `pablo trace export --redacted` produces the shareable form.
- Honor parent-based and configured OTel sampling, and put low-cardinality sampling-relevant attributes on spans at creation time.
- Keep local replay capture independent of OTel sampling so an unsampled operational trace does not remove a requested debug artifact.
- Prefer Collector tail sampling for policies based on terminal failure, latency, or cost, because a head sampler cannot see those facts at span creation.
- Metrics remain useful whether or not an individual trace is sampled.

## 22. Replay, evals, and feedback

The trace format should be designed so a production task can become an eval case.

### 22.1 Replay modes

- **Presentation replay:** replay recorded events without running tools or models.
- **Tool-result replay:** rerun the model loop while supplying captured tool results.
- **Agent-graph replay:** replay the complete ownership tree, dependency graph, handoffs, and message flow without starting models.
- **Full task replay:** rerun from snapshotted inputs in a fresh environment.
- **Candidate comparison:** run the same task against another model, prompt, skill, or runtime version.

### 22.2 Input snapshots

Anything volatile that affects a result should be snapshot-able at trace time:

- Files.
- Retrieved resources.
- Host-tool responses.
- Relevant timestamps.
- Model/tool/schema/skill configuration.

Snapshotting is optional because inputs may be sensitive and large, but an application cannot reconstruct lost transient inputs later.

### 22.3 Feedback events

Support structured links from a trace/output to:

- Rejected result.
- Retry.
- Regeneration.
- User edit.
- Undo.
- Abandonment.
- Final accepted replacement or diff.

Distinguish negative-only signals from corrected-to-right examples. This makes future evals and learning substantially more useful.

## 23. Context management

The runtime should not simply append forever.

Version 0.1 only needs bounded-run context management: account for system instructions, messages, tool schemas/results, and activated skills; estimate remaining capacity from provider usage with a safety margin; compact once with a visible summary when needed; and perform one bounded retry after a context-overflow rejection. It has no durable session or persistent-child history to restore.

Preserved 0.x behavior:

- Measure usable provider context rather than only raw message bytes.
- Include tool schemas, skills, files, and provider framing in capacity estimates.
- Keep recent tool-call/result relationships intact.
- Compact before exceeding model limits.
- Persist a durable summary and the identity of the history it replaces.
- Preserve system/developer instructions and active capability constraints.
- Preserve unresolved tasks, artifact identities, and host context version.
- Preserve child ownership, persistent-child session references, outstanding messages, and settled child outcomes.
- Preserve active chain identities, dependency state, validated handoff references, and checkpointed node outcomes.
- Emit compaction events and fingerprints.
- Estimate tokens from provider-reported usage and a per-model bytes-per-token calibration rather than bundling tokenizers, and treat a provider context-overflow rejection as a compaction trigger with one bounded retry (section 28.8).
- Compact only history behind the byte-stable instruction and tool prefix so provider prompt caches survive compaction (section 28.4).

For Graphline, a session must remain explicitly bound to an exact loan/context version. The host decides whether a changed canonical record starts a new session, rebases context, or adds a new versioned turn. The runtime must not silently treat old context as current business truth.

## 24. Artifacts

Artifacts are first-class outcomes for non-coding workflows.

Prefer an explicit `artifact.publish` capability or runtime protocol over scanning filesystem modification times. An artifact declaration should include:

- Artifact ID.
- Local path or byte-stream reference.
- Media type.
- Kind/category.
- Display name.
- Size and content digest.
- Producing tool/run/span.
- Producing agent and parent/root identities.
- Sensitivity.
- Optional preview metadata.

The host decides whether and where to persist it. A process adapter may still offer a configurable artifact collector for legacy agents that only write files.

## 25. Errors and recovery

Errors should be typed enough for applications to make safe decisions.

Suggested categories:

- Configuration.
- Credential/authentication.
- Provider capability mismatch.
- Provider request rejection.
- Provider transient failure.
- Provider permanent failure.
- Possibly billed/delivered provider failure.
- MCP startup/transport/protocol/tool failure.
- Shell spawn/process/timeout failure.
- Filesystem failure.
- Skill discovery/activation/resource failure.
- Host callback failure.
- Child spawn, queue, message, wait, persistence, or supervision failure.
- Policy denial.
- Output parsing/schema/semantic validation failure.
- Context overflow.
- Budget exhausted.
- Cancellation.
- Internal invariant failure.

Each error should expose:

- Stable machine code.
- Safe human message.
- Retryability classification.
- Whether the underlying action may already have occurred.
- Relevant provider/server/tool identity.
- Trace ID.
- Optional private diagnostic details.

Repair is distinct from retry:

- Retry repeats a failed transport or attempt.
- Repair continues the same semantic task with validation feedback.

Both must be bounded and visible.

## 26. TUI and CLI experience

### 26.1 Philosophy

The default interface should feel closer to a Unix program than a terminal IDE. It should be fast, readable, scriptable, and compatible with terminal scrollback.

The TUI should make the runtime’s important state visible without becoming the owner of that state.

### 26.2 Default interactive view

Version 0.1 shows the transcript, provider/model, current tool or child activity, bounded usage, yolo status, and trace ID. It needs a reliable composer, cancellation, terminal restoration, and no private execution state. The fuller product view may show:

- Conversation/output transcript.
- Current model and provider.
- Active skills.
- Connected MCP servers.
- Active, queued, waiting, and persistent child-agent counts.
- Active chain, ready/blocked node counts, and current critical path.
- Current activity/tool.
- Step, token, time, and cost budgets.
- Prompt cache read ratio for the current turn.
- Yolo/static-policy status.
- Session and trace identity.

Tool calls may be summarized inline with an expandable technical view.

### 26.3 Useful inspector surfaces

- Timeline of model and tool spans.
- Expandable live ownership tree and execution graph with per-child status, dependency, task, model, budget, and outcome.
- Shell stdout/stderr and process status.
- MCP server health and catalogs.
- Skill instructions and loaded resources.
- Context composition and token usage.
- Resolved configuration with provenance.
- Artifacts.
- Errors, retries, and repairs.
- Raw versioned events for debugging.

### 26.4 Noninteractive behavior

- Structured stdout must remain parseable.
- Human diagnostics should not corrupt machine output.
- Exit status must reflect the terminal outcome.
- Partial narration and valid final output must be distinguishable.
- Cancellation should produce a typed incomplete outcome.
- `pablo run --json` should include a schema version.
- `pablo run --stream` emits JSON Lines events on stdout and ends with the outcome record; `--json` emits only the final outcome document.

## 27. Developer experience

### 27.1 Installation

Goals:

- One command installs the native `pablo` binary.
- Prebuilt macOS and Linux binaries for x86_64 and arm64 initially.
- Windows support should be planned in the process and path contracts even if it follows later.
- No Node.js or Python runtime required for the agent binary.
- TypeScript/Python SDK packages may download the matching binary or accept an explicit path.

### 27.2 First run

With one provider environment variable, this should work:

```bash
cd workspace
pablo "organize these research files and write a summary"
```

The agent should have shell and filesystem capabilities immediately, discover compatible skills, connect configured MCP servers, stream useful activity, and leave a trace.

The first run also decides whether a developer trusts the tool. The first interactive run in a workspace prints one non-blocking stderr line stating that execution has no approval prompts and where static policy lives, and never prints it again once acknowledged. The first token should appear before every configured MCP server has finished starting (section 28.2). `pablo init` scaffolds a commented `.pablo/config.toml` and an empty `.agents/skills/` directory for developers who prefer to start from a file.

The five failures every developer meets first each produce a one-line cause, a one-line fix, a documented exit code, and a pointer to `pablo doctor`:

- No provider credential in the environment.
- A rejected credential.
- A model name the gateway does not serve.
- An MCP server that fails to start or negotiate.
- A shell or tool call denied by static policy.

These messages name the failing component and the configuration source that produced the value, without printing secrets.

### 27.3 Diagnostics

`pablo doctor` should verify without executing an agent task:

- Binary/version/platform.
- Config files and precedence.
- Provider credentials without printing them.
- Provider/model reachability when explicitly requested.
- Shell availability.
- Skill roots and invalid skills.
- MCP server configuration and optional connectivity.
- Subagent limits, model overrides, inheritance, and persistence configuration.
- Trace directory writability.
- OTel SDK state, propagators, service/resource identity, pinned GenAI convention revision, sampler, exporter protocol, endpoint host, and masked exporter credentials.
- Registered or bundled add-ons, their versions, supported protocol revisions, bind configuration, and compatibility with the running core protocol.
- Name collision for the `pablo` executable.
- Startup timing for each phase, so a slow skill root, MCP server, or certificate store is visible without a profiler.

`pablo doctor` completes in under one second without network access and reports its own timing.

### 27.4 Extension workflow

Developers should be able to grow capability in this order:

1. Ask the shell to use an existing installed CLI.
2. Add a script.
3. Package the workflow as a skill.
4. Connect an existing MCP server.
5. Expose an application-native host tool.
6. Delegate suitable work to a bounded child with a specialized skill or toolset.
7. Chain children through typed outcomes and artifact references when work has real dependencies or parallel partitions.
8. Use native Open Responses, ACP, or A2A interoperability when it matches the boundary; add an optional AG-UI, Vercel AI SDK stream, or OpenAPI package only when the host needs that projection.
9. Build a custom Rust tool only when native embedding or performance genuinely requires it.

This is the central extensibility story.

### 27.5 Compatibility and stability

- Version public runtime contracts and negotiated ACP/A2A `pablo` extensions independently from the binary release.
- Use explicit capability negotiation.
- Preserve unknown event fields in SDKs when possible.
- Qualify dynamic capability identities.
- Provide migration notes for config and protocol changes.
- Treat trace format as a public artifact once replay/eval tooling depends on it.
- Publish a machine-readable matrix of core, wire-protocol, add-on, and upstream-protocol versions.
- Distinguish `core`, `bundled`, `official add-on`, `experimental add-on`, and `community add-on` support in documentation.
- Define compatibility by exercised operations and conformance fixtures, not by the ability to parse one happy-path message.
- Keep add-on dependencies one-way so removing an adapter cannot change core execution semantics.

## 28. Performance design

Fast to start, cheap to run, and predictable under load are product requirements, not tuning left for later. Model latency dominates most runs, but a runtime whose own overhead is invisible is what makes the "tiny native binary" thesis true. This section sets provisional budgets, the hot-path rules that protect them, the prompt cache contract, and proposed resolutions for the open questions that most affect speed and cost. It is the canonical home for performance statements; other sections point here rather than restate them.

### 28.1 Provisional budgets

These numbers are targets to design against, not all version 0.1 release gates. Version 0.1 records the default/headless binary size, cold start, idle RSS, event path, in-process child, and native trace measurements; Phase 1 evidence ratifies or revises the remaining targets before they become gates. After a target is ratified, CI fails on a regression larger than 10% from the recorded baseline. Measure release builds on linux-x86_64 and macos-arm64 with a warm filesystem cache unless stated otherwise.

| Budget | Target | How it is measured |
| --- | --- | --- |
| Default distribution binary, stripped | ≤ 15 MiB target, 20 MiB ceiling | `cargo bloat` attribution per crate in CI |
| Headless build without TUI and A2A server | ≤ 10 MiB | Same |
| Process start to provider request ready, no MCP or skills | ≤ 30 ms p50 | Internal `pablo.startup.*` spans; `hyperfine` for wall clock |
| Process start to first byte written to the provider, typical network | ≤ 150 ms p50 | Same; TLS warm-up overlaps other startup work |
| TUI first paint | ≤ 50 ms | Virtual-terminal test |
| Idle RSS, headless ACP agent | ≤ 25 MiB | RSS sampled after `initialize` |
| Idle RSS, TUI open | ≤ 40 MiB | RSS sampled after first paint |
| Provider chunk parsed to in-process subscriber wake | ≤ 100 µs p50, ≤ 1 ms p99 with 8 active agents | Criterion benchmark on the event path |
| Skill catalog scan, 1000 skills | ≤ 100 ms cold, ≤ 5 ms with a valid cache | Benchmark fixture |
| Request assembly per step, 200 messages and 1 MiB history | ≤ 5 ms | Criterion benchmark |
| In-process child ready after spawn | ≤ 2 ms, ≤ 512 KiB baseline heap | Benchmark |
| Native trace writer share of run wall time | ≤ 2% | Same run with and without trace enabled |
| Cached-prefix stability across steps in one turn | Byte-identical on every step | Deterministic unit test, no provider needed |
| Cache-read share of input tokens on steps after the first, Otto fixture | ≥ 80% | Provider-reported usage in the trace |

Publish the measured values for each tagged release alongside the protocol-path overhead numbers described in section 28.10.

### 28.2 Startup path

Work that is not needed for the first model request must not delay it.

1. Parse configuration once, layered, and keep the resolved form with provenance for `pablo config explain`.
2. Start the provider TLS warm-up immediately and let it overlap everything below.
3. Scan skill roots concurrently. Cache the catalog keyed by root path, directory mtime, and each `SKILL.md` size and mtime, so a warm start reads no frontmatter.
4. Connect every configured MCP server concurrently. Servers marked `required = true` gate the first request. Others get `startup_grace_ms` (default 1500); a server that misses it joins at the next turn boundary with a catalog-change event rather than delaying the run or perturbing the current prompt prefix.
5. Initialize the OTel SDK without network work; create exporters lazily on first export.
6. Bind no listener and perform no A2A discovery unless a remote child or server profile is actually used.
7. Paint the TUI before any network activity completes.

Each phase is an `pablo.startup.*` span, so `pablo run --timings` and `pablo doctor` show where startup time went without a profiler.

TLS certificate loading is a known cold-start cost, especially reading the platform trust store on macOS. Bundle `webpki-roots` by default and offer the platform store as an explicit option for environments with private certificate authorities.

### 28.3 Provider transport

- One HTTP client per provider per runtime, reused across steps, turns, and children; never a client per request.
- HTTP/2 with keep-alive where the gateway supports it, `rustls` for TLS, explicit connect and read timeouts.
- Incremental SSE parsing that yields events as bytes arrive and never accumulates the whole response body.
- Incremental request assembly: cache the serialized bytes of the stable prefix and previously sent messages so each step serializes only what changed.
- Per-request timing captured as time to first chunk and time per output chunk, exported through the standard GenAI metrics.

### 28.4 Prompt cache control

Provider prefix caching is the largest single lever on latency and cost in an agent loop, and a runtime can silently defeat it. Cache control is therefore a runtime responsibility with a contract, a capability record per provider, accounting, diagnostics, and a deterministic test. This subsection is the canonical statement; sections 15.4, 16.2, 17.5, 19.3, and 23 point here.

The version 0.1 gate is deliberately smaller: deterministic ordering, a byte-stable prefix within a turn, provider-reported cache usage in the trace, and a cache-break diagnostic. The breakpoint priority, adaptive TTL, cross-run affinity, dry-run planner, status-line ratio, and 80% Otto target below are preserved optimization work after the first end-to-end slice.

#### Cacheable prefix

Requests are assembled from fixed logical layers: runtime invariants, host system instructions, host developer instructions, tool catalog, skill catalog, then conversation history. Everything before the history is the **prefix**. Each provider adapter serializes those layers in its provider's cache-prefix order, which for Anthropic-style caching is tools, then system, then messages, and the stability requirement applies to the serialized bytes rather than the logical order.

Within one turn the prefix is byte-identical on every step. Across turns of one session it changes only when the host or user changes it. Across separate runs of the same host application it is identical whenever the application's instructions and tool set are identical, so a fleet of Otto analyses shares one cached prefix inside the provider's TTL and account scope.

Rules that keep the prefix stable:

- Serialize tools sorted by qualified identity, never in discovery order, using canonical JSON with sorted keys and no insignificant whitespace.
- Keep the advertised tool set fixed for the session. Newly needed catalog tools go through the stable dispatcher in section 15.4; promotion into the native tool array happens only at a turn boundary.
- Put nothing volatile in the prefix: no timestamps, run or session IDs, sandbox IDs, nonces, step counters, or per-run file listings. Volatile facts the model needs go in the first user message.
- Splice activated skill bodies, late-arriving MCP catalogs, context attachments, and compaction summaries into the history as messages at the point they appear, never into the prefix. Host-pinned skills declared at session start may live in the prefix.
- Children built from the same `SubagentSpec` template share an identical prefix, so a fan-out of N workers pays one cache write and N−1 reads. A child overlay is appended after the shared layers rather than merged into them.
- Compact history only behind the prefix boundary, and prefer compaction at turn boundaries, because rewriting history invalidates the cached conversation tail.

#### Breakpoints and TTL

Providers differ in how caching is controlled, and each adapter's capability record (section 19.3) declares which applies: explicit breakpoints with a maximum count, a minimum cacheable length, and TTL options; automatic prefix caching with an optional affinity key; a separately managed cached-context resource; or none.

For explicit-breakpoint providers the runtime places, in priority order:

1. One breakpoint at the end of the prefix.
2. One rolling breakpoint at the last message of the request, so each tool-loop step reads everything before the previous step and writes only the new tail.
3. One after the most recent large insertion, such as an activated skill body, a large attachment, or a compaction summary, when a breakpoint remains.
4. One after the host-pinned skills block when the host declares that it changes independently of the system layers.

The adapter drops lower-priority breakpoints when a provider allows fewer, and never places one inside a block shorter than the provider's minimum cacheable length.

For automatic providers the runtime sets the affinity or cache key to a stable session-derived value where the API offers one, and otherwise relies on prefix stability alone.

TTL is `auto` by default: the shorter TTL for bounded runs, the longer TTL for resumable sessions where human think time between turns is expected. Hosts may pin either. A cache write costs more than a plain input token on some providers, so the runtime requests a write only when a run is expected to make more than one model request, which is true of every tool-using run; hosts running fleets of identical bounded jobs may force writes because the prefix is shared across runs.

#### Gateways and Open Responses

Vercel AI Gateway and OpenRouter forward provider cache controls and report cache usage in their own field names. Each adapter maps both and records in its capability record whether pass-through was verified against the pinned gateway behavior. When an Open Responses endpoint declares no cache control, the runtime treats it as automatic and still enforces prefix stability. A remote A2A agent's caching is invisible and is reported only if the peer reports usage.

#### Accounting and diagnostics

- Every model request records `pablo.prompt.prefix.digest`, the breakpoints applied, the TTL requested, and the cache-read and cache-write token counts the provider returned.
- Usage and cost accounting price cache reads and writes separately, per provider, and roll up through the agent tree.
- A request whose prefix digest differs from the previous step in the same turn emits a `prompt.cache_break` event naming the layer that changed and the cause: tool set, skill activation, compaction, host instruction change, provider switch, or model switch.
- The TUI status line shows the cache-read ratio for the current turn; `pablo trace show` shows read and write tokens per step; `pablo run --dry-run` prints the planned layers, breakpoints, and prefix digest without calling a model.
- The OTel model span carries the pinned convention's cache token attributes plus `pablo.cache.hit_ratio`, and `prompt.cache_break` is a span event with a low-cardinality `pablo.cache.break.reason`.

#### Tests

- A deterministic unit test asserts byte-identical serialized prefixes across every step of a fixture turn for each provider adapter; no provider is needed.
- A unit test per adapter asserts breakpoint placement and pruning against the adapter's declared limits.
- The Otto dogfood run reports its cache-read share against the provisional target in section 28.1.

### 28.5 Parallel tool execution

This is a post-0.1 optimization. Version 0.1 executes ordinary model-emitted shell, filesystem, and MCP calls deterministically in call order. The subagent supervisor may run two explicitly spawned temporary children concurrently because bounded fan-out is part of the release proof. After tool effect metadata and cancellation are proven, the intended general behavior is:

- Dispatch up to `max_parallel_tool_calls` (default 8) at once and queue the rest.
- Tools may declare `serial = true` to opt out. `shell.interact` and `shell.stop` serialize per process handle; `shell.run` calls run in parallel because each has its own process.
- MCP calls additionally respect a per-server `max_concurrent_requests` (default 4). Shell and MCP share the global limit; there is no second pool.
- Results return to the model in the model's call order regardless of completion order.
- One failed call returns its own typed failure and does not cancel siblings; cancelling the turn cancels every in-flight call.
- Reserve tool-call and process budgets per call before dispatch.
- The trace records dispatch order, completion order, and one span per call.

Hosts set `parallel_tool_calls = false` when a workspace cannot tolerate concurrent writes. Subagent spawn calls are ordinary parallel tool calls, which is what makes fan-out cheap.

### 28.6 Event path and backpressure

- Events are small structs whose shared identity fields live once in a reference-counted run context; string and UUID encodings appear only at serialization boundaries.
- Delta payloads use reference-counted byte buffers so a text chunk is not copied per subscriber.
- Subscribers are either **lossless** (native trace writer, ACP clients, add-on projections) or **lossy-ok** (the TUI renderer). Lossy-ok subscribers keep only the latest render state and never block the producer.
- Lossless subscribers use bounded queues. When a queue crosses its soft limit, adjacent text, tool-input, and shell-output deltas for the same target are merged and the merged delta carries its `seq` range. When the hard limit is reached the producer waits and a `consumer.slow` diagnostic is emitted. Lifecycle events are never dropped or merged.
- External transports coalesce deltas at whichever comes first of 16 ms or 4 KiB per target. In-process subscribers receive deltas as parsed.
- Sequence numbers are assigned at publication, monotonic per run, and unaffected by coalescing.
- The TUI renders on a timer at most once per display refresh, diffs frames, and never touches the runtime hot path.

### 28.7 Persistence and trace writing

Version 0.1 writes one bounded append-only JSONL trace for a run and does not provide durable session resume, snapshots, or a content-addressed blob store. Large tool results and artifacts use bounded files plus recorded paths and digests inside the configured workspace.

The proposed post-0.1 resolution for session storage is preserved here:

- Sessions live at `.pablo/sessions/<session_id>/` as `session.json` (the `SessionRef` and latest snapshot pointer), an append-only `events.jsonl`, and `snapshots/<seq>.json` written at turn end. Resume loads the latest snapshot and replays the tail, so resume time depends on one turn, not the session's age.
- Tool outputs, attachments, and artifacts larger than 64 KiB are stored once under `.pablo/blobs/<sha256>` and referenced by digest from events, sessions, traces, and handoffs.
- Native traces are JSON Lines with a header record holding the static run context, followed by compact events. Deltas are recorded as the provider produced them, before any transport coalescing.
- The trace writer runs on a dedicated writer task fed by a bounded channel, batches writes through a buffered writer, and syncs to disk at turn end, session close, or on request; `fsync = "always"` is available for hosts that require it.
- Crash recovery replays from the last snapshot and marks in-flight children interrupted rather than silently resuming them.
- When durable sessions arrive after version 0.1, session encryption remains a host and filesystem concern and stays open in section 35.

### 28.8 Token accounting without tokenizers

The default binary bundles no tokenizer. Provider-reported usage from the previous response is the ground truth for context size; the runtime estimates the size of new content with a per-model bytes-per-token ratio calibrated from that usage, applies a safety margin, and compacts before the estimate reaches the model limit. A provider context-overflow rejection is a compaction trigger followed by one bounded retry, not a terminal failure. This keeps the binary small, keeps request assembly cheap, and stays correct as providers change tokenizers.

### 28.9 Skills and capability catalogs

- Skill catalogs advertise names and descriptions in a stable order, and the model selects from the catalog by default, which costs no extra request. A lexical prefilter is available for catalogs large enough to crowd the prompt, and hosts may supply their own matcher.
- MCP capabilities beyond the advertised default set are reached through the stable dispatcher in section 15.4, so the tool block never changes mid-turn.
- MCP catalog metadata is cached per server and invalidated only by catalog-change notifications or reconnection.
- JSON Schema compilation is cached by canonical digest across steps, turns, and children.
- Skill and resource digests are computed once per process and cached by path, size, and mtime.

### 28.10 Subagents and protocol paths

Version 0.1 exercises depth-one in-process ACP children and one configured A2A remote proxy. It shares clients and schemas, uses one atomic root budget, and moves bounded results inline or by workspace file reference. External ACP child processes, recursive subtree cancellation, content-addressed handoffs, and the full chain scheduler described below are post-0.1.

- In-process children are tasks on the shared async runtime. They share provider clients, HTTP pools, MCP connections, compiled schemas, immutable blobs, and the cached prompt prefix, with per-child attribution. ACP is the semantic contract on this path, not a JSON round trip: typed dispatch over shared immutable buffers, never serialize-then-parse inside one process.
- Budget reservation is one atomic operation against the root ledger so concurrent children cannot overspend.
- Handoffs move by blob reference and digest; a referenced value is materialized into a child workspace only when the child needs a file.
- Cancelling a subtree cancels queued and speculative descendants first, then in-flight work.
- ACP subprocess sessions use bounded buffered reads and writes and the coalescing thresholds in section 28.6; they do not flush per delta.
- A2A clients pool HTTP and TLS connections and cache validated Agent Cards by HTTP validators and content digest with an explicit refresh policy. A2A servers and discovery initialize lazily.
- Initial native A2A support uses the official core types plus one JSON/HTTP streaming binding. gRPC, SLIMRPC, push-notification infrastructure, and alternate bindings stay out of the ordinary binary until measurements and real integrations justify them.
- Chain scheduling rules live in section 17.18, and the efficiency diagnostics defined there are recorded in the trace.

Expected overhead by path, to be measured rather than claimed:

- A root run with no delegation performs no A2A discovery, networking, server setup, or protocol encoding. Linked size and cold start are its only A2A costs, kept behind lazy construction and narrow features.
- A local in-process child pays typed dispatch, queueing, supervision, and event bookkeeping.
- An external local child adds process start or reuse, context switches, stdio buffering, and JSON encoding. Coalescing keeps this small relative to model and tool latency, but high-rate throughput and slow-consumer memory must be benchmarked.
- A remote A2A child adds Agent Card lookup when uncached, HTTP/TLS transport, serialization, and streaming. With cached cards and pooled connections, network and remote work dominate.

Publish absolute and relative results for the no-protocol baseline, in-memory ACP, warm and cold ACP subprocess, and warm pooled and cold-discovery A2A paths. Model latency normally dominates, but regressions in a "minimal" native binary must stay visible.

### 28.11 Telemetry overhead

- One OTel span per logical operation, created with its attributes at start; no span or OTel event per delta, terminal byte, or repaint.
- Batch processors are bounded and export runs off the hot path; exporter failure behavior is defined in section 21.11.
- Bounded CLI runs flush with a short configurable deadline (default 2 s) and report dropped telemetry on stderr.
- When a Rust host injects its own providers, the runtime creates no SDK of its own.

### 28.12 Build and dependency posture

- Release profile: link-time optimization, `codegen-units = 1`, `panic = "abort"`, symbols stripped, and `opt-level` chosen by measurement rather than habit.
- Prefer crates that share dependencies already in the tree; every new dependency is justified in its PR by measured size and startup contribution.
- Cargo features gate what the settled decisions allow to be optional within the native package: the TUI, the A2A server binding, OTLP/gRPC, and each official add-on. The version 0.1 default distribution enables the TUI and A2A client but not the A2A server listener; the native server module can land after the client path without changing core semantics.
- CI publishes binary size, cold start, idle RSS, and event-path benchmarks for every tagged build so a regression is visible in the PR that caused it.

## 29. Version 0.1 scope

Only section 29.1 defines the version 0.1 release gate. Native support in 0.1 means a deliberately small, interoperable subset implemented on the standard protocol and shared runtime, not every optional operation in that protocol. Sections 29.2 through 29.5 preserve the larger design and sequencing decisions without making them blockers for the first release.

The north-star proof is one Otto-style bounded analysis: install one binary in an existing sandbox, invoke it from TypeScript over ACP, use a model plus shell/filesystem, one MCP tool, one Agent Skill, temporary local delegation, validate the result, and inspect the complete OTel-correlated trace. A separate protocol fixture proves one remote A2A delegation; Otto does not depend on a remote agent service.

### 29.1 Focused release contract

**Distribution and interfaces**

- One Rust workspace producing a reusable core crate and one `pablo` executable for macOS and Linux.
- `pablo "task"`, `pablo run --json`, and a basic streaming TUI built on the same runtime events.
- Stable ACP v1 stdio is the only required cross-language API. Ship a tiny TypeScript reference client used by Otto; do not stabilize custom TypeScript or Python SDKs yet.

**Bounded agent loop**

- One in-memory ACP session per bounded run, a streamed model/tool loop, typed terminal outcome, cancellation, and hard limits for steps, wall time, model usage/cost, tool calls, process count, and result bytes.
- Tool arguments and optional final structured output use JSON Schema Draft 2020-12. One same-session repair attempt is enough.
- Preserve a byte-stable prompt prefix and record provider cache usage. Adaptive breakpoint placement, TTL policy, affinity, and cache-ratio targets are not release gates.
- Emit one append-only bounded JSONL event trace. Presentation replay may be a small inspection command; deterministic tool, graph, and full-task replay are later work.

**Providers**

- Vercel AI Gateway and OpenRouter are configured through focused adapters sharing HTTP, SSE, message, tool, and usage machinery.
- Native Open Responses support uses the same provider-neutral boundary.
- Test one known-good model/profile per gateway and one conforming Open Responses fixture. Dynamic model catalogs, automatic provider fallback, and broad capability matrices are not required.

**Capabilities**

- `shell.run` executes a noninteractive command with explicit working directory, environment additions, timeout, output bound, exit status, cancellation, and process-tree cleanup. Interactive PTY sessions and durable background handles are later.
- Filesystem read, write, edit, list, and search stay small and explicit.
- Yolo mode executes immediately inside exact static tool, executable, MCP-server, and filesystem-root allow/deny rules. The policy makes no sandbox claim.
- Version 0.1 MCP support covers initialize, tool discovery, tool calls, results, cancellation, and bounded progress over stdio and Streamable HTTP. Resources, templates, prompts, roots, dynamic catalog routing, sampling, tasks, OAuth UX, and elicitation can follow.
- Version 0.1 Agent Skills support discovers local compatible skills, validates their metadata and paths, exposes a stable name/description catalog, supports explicit activation, and loads selected instructions/resources progressively. Fuzzy matching, remote installation, lockfiles, and a registry can follow.
- Out-of-process application tools use MCP. A direct reverse host-tool extension is not required in 0.1.

**Subagents and native protocols**

- Support temporary, depth-one children only. Each has its own ID, bounded context, model loop, narrowed tool/skill/MCP view, outcome, usage, events, cancellation state, and OTel spans.
- Provide `spawn`, `wait` for any/all, `inspect`, and `stop`. Two children can run concurrently. Persistent names, later messages, recursive descendants, and direct sibling communication are not required.
- Children share one root atomic budget ledger and intersect their requested authority with root policy. Per-child accounting is visible; hierarchical reservation trees are later.
- A parent or host can pass one validated bounded result or artifact reference into a later child. This proves sequential handoff and fan-out/fan-in without a general dependency-graph scheduler, checkpoints, join DSL, quorum, or cycle detector.
- Local children use typed in-memory ACP dispatch with no JSON round trip. The host-facing ACP subset covers `initialize`, `session/new`, `session/prompt`, `session/update`, and `session/cancel`; unsupported optional methods are advertised honestly.
- Native A2A 1.0 in version 0.1 is client-side delegation over one configured HTTP/SSE binding: bounded Agent Card validation, message/task streaming, status, one Artifact result, and cancellation. A remote agent appears as an untrusted local proxy. The native server role, listeners, registries, push notifications, gRPC, SLIMRPC, durable contexts, and broad discovery are later.

**Observability and proof**

- OTel is native from the first operation. Version 0.1 requires W3C Trace Context plus spans for the run, model call, shell/filesystem/MCP tool, local child, and remote A2A task, exported over OTLP/HTTP with content disabled by default.
- Metrics and correlated OTel logs remain part of the native telemetry design, but traces are the only signal required for version 0.1. Usage, latency, errors, and concurrency remain attributes/events in the native trace until stable low-cardinality instruments are promoted.
- The native JSONL trace shares OTel trace/span IDs and makes model, tools, skills, child identity, usage, outcome, and safe errors inspectable.
- One Otto bounded read-only analysis passes end to end through the built binary and TypeScript ACP reference client. Record binary size, cold start, idle memory, event latency, and ACP/A2A path overhead; ratify budgets after measurement rather than delaying the release for guessed numbers.

### 29.2 Explicitly not required for version 0.1

These remain valid product ideas. They move out of the first release because they introduce durable state, another integration surface, or a combinatorial test matrix before the central loop is proven:

- Durable root-session resume, named persistent children, crash recovery, migrations, pluggable stores, encryption, and content-addressed blob storage.
- Recursive children beyond depth one, external third-party ACP children, the native A2A server role, generic agent graphs, checkpoint reuse, conditional scheduling, quorum joins, wait-cycle analysis, and reusable chain specifications.
- Interactive PTYs, `shell.interact`, retained background process handles, and terminal multiplexing.
- The full MCP surface beyond tools, or a large-catalog dispatcher.
- Automatic/fuzzy skill activation, remote skill installation, lockfiles, registries, and trust distribution.
- Custom TypeScript and Python SDKs; the Python/Graphline integration; direct ACP host-tool callbacks.
- AG-UI, the Vercel AI SDK `UIMessage` adapter, and the OpenAPI importer. They remain official add-ons, but can begin after the native event and ACP contracts survive Otto.
- Provider fallback orchestration, live model catalogs, advanced prompt-cache control, tokenizer calibration, and parallel execution of arbitrary model-emitted tool calls.
- Full input snapshotting, tool-result replay, graph replay, full-task replay, candidate comparison, feedback/learning records, and a general artifact store.
- Complete OTel metrics and logs coverage, tail-sampling guidance, OTLP/gRPC, vendor exporters, offline telemetry buffering, and telemetry profiles.
- A polished graph/tree TUI, every inspection command, exhaustive diagnostics, add-on manifests, and oldest-to-newest compatibility matrices.
- Graphline acceptance, an E2B-specific smoke test, Windows distribution, and every later product exclusion already listed in section 29.4.

This staging does not turn any P0 foundation into an add-on. JSON Schema, JSON-RPC, Open Responses, ACP, A2A, MCP, Skills, shell/filesystem, and native OTel remain in the package. Version 0.1 simply implements the smallest useful operation set for each.

<details>
<summary>Preserved full 0.x product target and exclusions</summary>

### 29.3 Preserved full 0.x product target

The inventory below captures the intended product surface after version 0.1. It is retained as design input and should be promoted into a release only when a real integration needs it.

**Runtime core**

- Rust native executable with the `pablo` CLI and interactive TUI.
- Provider-neutral streamed model and tool loop; one-shot runs; resumable local sessions.
- Cancellation, deadlines, and step, token, time, cost, tool, process, and MCP budgets.
- Context measurement and compaction; same-session validation repair.
- Text/Markdown and JSON Schema Draft 2020-12 output contracts, using the same dialect for tools and handoffs.
- Prompt cache control: prefix stability, breakpoint placement, accounting, and cache-break diagnostics.
- Stable event stream; trace files with configurable content capture and redaction; artifact declaration.

**Agents**

- Root and child identities in every run, event, trace, and session contract.
- Temporary bounded children and named persistent child sessions.
- Model-facing and host-facing spawn/run, message, send, wait, list, inspect, and stop.
- Concurrent children with bounded queues, depth and count limits, and deadlock-safe scheduling.
- Typed chaining: sequential dependencies, bounded fan-out and fan-in, explicit joins, checkpointed outcomes, and wait-cycle detection.
- Bounded handoffs through ACP/A2A-native content shapes, never transcript copies.
- Hierarchical budgets with reservation and roll-up; authority that can only narrow.
- Multiplexed root event stream and per-agent filtered views.

**Capabilities**

- Shell enabled by default with run, interact, and stop and long-running process handles.
- Filesystem read, write, edit, list, and search.
- Yolo execution with no approval UI; static allow/deny for shell, tools, MCP, and filesystem.
- MCP client over stdio and Streamable HTTP: tools, resources, templates, prompts, roots, progress, catalog changes, and the long-tail dispatcher.
- Agent Skills discovery, explicit activation, automatic matching, and resource loading.
- Host-tool callbacks.

**Providers and protocols**

- Vercel AI Gateway, OpenRouter, and native Open Responses with pinned acceptance fixtures.
- Headless stable ACP v1 agent over stdio, an in-memory transport for local children, and versioned, negotiated `pablo` extensions only for missing semantics.
- Native A2A 1.0 client and server with configured Agent Cards, one HTTP binding, and local proxy-node supervision.
- Thin TypeScript and Python clients.

**Observability**

- Native OTel spans, metrics, and correlated logs at the instrumented operation, following a pinned GenAI/MCP convention revision.
- W3C Trace Context and filtered Baggage across SDK calls, host-tool callbacks, subagents, and MCP.
- OTLP/HTTP Protobuf export with standard `OTEL_*` configuration, network export off by default.

**Official optional add-ons and tooling**

- AG-UI HTTP/SSE projection; TypeScript Vercel AI SDK `UIMessage` adapter; OpenAPI 3.1.x importer.
- Machine-readable add-on compatibility manifests and conformance matrices.
- `doctor`, `init`, config explain, and session, MCP, skill, and trace inspection commands.
- Real binary and PTY smoke tests.

### 29.4 Explicitly outside the current product scope

- Built-in E2B, Daytona, or Vercel Sandbox provisioning.
- A durable workflow engine or scheduler.
- Database persistence.
- Multi-tenant quota management.
- A generic business approval product.
- A model-based security reviewer.
- Interactive permission prompts.
- Dynamic Rust plugins or a stable Rust ABI.
- A proprietary skills registry or skill format.
- Dozens of native SaaS integrations.
- A proprietary swarm planner, fixed multi-agent roles, or cross-tenant autonomous agent pool.
- A durable declarative DAG language, visual workflow builder, or autonomous chain planner.
- Full browser/computer-use implementation unless supplied through shell or MCP.
- MCP sampling/tasks and elaborate elicitation unless needed for an early integration.
- Automatic trace upload or hosted control plane.
- OTLP/gRPC in the default minimal binary, vendor-specific telemetry exporters, and OTel profiles.
- Bundling AG-UI, the Vercel AI SDK adapter, or OpenAPI into every core-library build or the ordinary minimal executable.
- Using A2A for in-process or local-process children where ACP supplies the native session/control path, or treating a remote A2A task as a fully trusted local child.
- Stable ACP v2 support until the upstream protocol is no longer draft and the adapter passes a deliberate migration review.
- A generic runtime marketplace, implicit add-on download, arbitrary native dynamic-library loading, or an add-on ABI that bypasses public runtime contracts.
- Training or model fine-tuning infrastructure.

### 29.5 Why the architecture can remain minimal

The list is substantial, but most user-facing breadth comes from five deep reusable systems: model loop, shell, MCP, skills, and recursive agent supervision. The P0 contracts—JSON Schema Draft 2020-12, shared JSON-RPC 2.0 machinery, Open Responses, ACP, A2A, MCP, Skills, native OTel, shell/filesystem tools, and the initial gateways—ship as one supported package. They should still be internally modular, lazily initialized, and dependency-trimmed so an unused A2A server or external ACP transport creates no listener, connection, or hot-path work. P1 compatibility surfaces—AG-UI, the Vercel AI SDK stream, and OpenAPI import—remain separately selectable projections. Native means available and supported, not eagerly active.

The preserved target is intentionally much larger than the version 0.1 gate. Section 30 sequences it so hosts can integrate against the bounded model loop, shell, ACP, temporary children, and native observability before durable sessions, broad compatibility, or application adapters exist.

</details>

## 30. Suggested implementation sequence

### Version 0.1 critical path

Build vertical slices rather than completing horizontal subsystems in isolation:

1. **Contract slice:** freeze only `RunSpec`, `RunEvent`, `RunOutcome`, `Tool`, temporary `AgentRef`, the supported JSON Schema subset, the minimal ACP methods/extensions, the minimal A2A binding, and the OTel span/redaction topology needed by the Otto fixture.
2. **Single-agent slice:** drive the built binary from a TypeScript ACP reference client, call one gateway model, execute `shell.run` and filesystem tools, stream events, cancel, return a typed outcome, and write correlated native/OTel traces.
3. **Extensibility slice:** add the second gateway and Open Responses, one stdio and one Streamable HTTP MCP tool fixture, one local Agent Skill, structured-output validation/repair, and two concurrent depth-one local children.
4. **Interoperability slice:** add the minimal A2A client task path against a reference server and represent it as a remote proxy, then add the basic TUI, exact static policy, the Otto end-to-end run, protocol fixtures, and measured size/startup/event/protocol overhead.
5. **Release hardening:** exercise real binaries on supported macOS and Linux targets, resolve only failures on the focused acceptance path, document unsupported optional protocol operations, and publish the pinned versions and measured baseline.

Do not start durable session storage, persistent children, a generic chain scheduler, custom language SDKs, optional adapters, or Graphline work before these slices pass. A protocol module may define future-compatible IDs or enums, but unimplemented behavior must not bring storage, dependencies, CLI commands, or release tests with it.

### Preserved 0.x implementation inventory

The phase inventory below retains the fuller architecture and can be reordered after version 0.1. It is not a release checklist; items enter a milestone only when promoted into section 29.1.

<details>
<summary>Show the full 0.x phase inventory</summary>

### Phase 0: contracts and fixtures

- Freeze an initial `RunSpec`, `AgentRef`, `SubagentSpec`, event envelope, terminal outcome, tool descriptor, and session snapshot.
- Freeze the native OTel span/metric/log topology, propagation contract, custom namespace, and pinned GenAI/MCP convention revision alongside those lifecycle contracts.
- Select JSON Schema Draft 2020-12 as the only core schema dialect and publish fixtures for references, validation output, and unsupported vocabularies.
- Pin stable ACP v1 and A2A 1.0 schema/SDK revisions, adopt ACP’s standard lifecycle/transport, and define only the minimal versioned `pablo` extensions needed for budgets, authority, graph identity, typed outcomes, replay, and trace correlation.
- Create provider-stream fixtures, MCP fixtures, skill fixtures, and fake host-tool fixtures.
- Verify ACP v1 stdio framing and bidirectional request ownership against conformance fixtures; define bounded buffering, backpressure, and only the minimal capability-negotiated `pablo` extensions.

### Phase 1: headless native loop

- Implement one provider adapter and a fake provider.
- Implement streamed content and tool calls.
- Add filesystem and basic shell execution.
- Add events, cancellation, budgets, JSON output, W3C trace context, and native OTel spans, metrics, and correlated logs for the initial model/tool lifecycle.
- Run the root agent through the same ACP agent/session/prompt/update handlers that local child agents will use.

### Phase 2: durable shell and sessions

- Add process handles, PTY, interact, stop, retained output, and process-tree cancellation.
- Persist and resume runtime sessions.
- Add context measurement and compaction.

### Phase 3: subagent supervisor

- Make the root an ordinary agent node with an immutable root identity.
- Add temporary and named persistent local children as ACP sessions using typed in-memory dispatch, plus configured external ACP subprocess children.
- Add remote A2A proxy children using Agent Cards, contexts, tasks, status/artifact streaming, and cancellation.
- Implement spawn, message, send, wait, list, inspect, and stop.
- Add concurrency queues, tree-depth/child-count limits, and deadlock prevention.
- Add chain/node/edge identities, typed handoffs, dependency readiness, joins, and checkpoint reuse.
- Project fan-in and cross-trace dependencies as OTel span links while retaining the full native graph.
- Add hierarchical authority, budget reservation, usage roll-up, and cancellation propagation.
- Multiplex child events into the root stream and render a basic agent tree.

### Phase 4: providers and output contracts

- Finish both Vercel AI Gateway and OpenRouter.
- Implement native Open Responses provider compatibility against a pinned upstream acceptance-suite revision.
- Add capability resolution, structured output, usage/cost, retries, and fallback.
- Add JSON Schema validation and repair.

### Phase 5: skills and MCP

- Implement Agent Skills discovery and progressive disclosure.
- Integrate the official MCP Rust client.
- Normalize MCP tools into the unified registry.
- Add resources, prompts, progress, and dynamic capability selection.
- Implement the optional OpenAPI 3.1 importer over the same tool descriptor, policy, schema, trace, and result-bounding path.

### Phase 6: embedding and interface

- Stabilize the ACP v1 implementation, transport behavior, and negotiated `pablo` extensions.
- Add TypeScript and Python SDKs.
- Complete native GenAI/MCP instrumentation coverage and add bounded OTLP/HTTP Protobuf SDK/export configuration.
- Build the TUI entirely on the same events.
- Add config inspection, doctor, and trace tools.
- Freeze the public adapter contract and machine-readable compatibility manifest.

### Phase 7: protocol interoperability and optional add-ons

- Implement the AG-UI HTTP/SSE adapter with run, message, tool, state, activity, interrupt, cancellation, and reconnect coverage.
- Implement the TypeScript Vercel AI SDK `UIMessage` stream projection and exercise it through `useChat`.
- Complete stable ACP v1 and A2A 1.0 conformance using their official Rust SDKs and technology compatibility tests without leaking editor-specific or remote-service state into unrelated core semantics.
- Propagate W3C Trace Context through native ACP/A2A and each shipped optional adapter; correlate external IDs with native run, session, agent, task, and artifact IDs.
- Publish protocol mapping tables, unsupported-feature behavior, conformance fixtures, security guidance, and a tested compatibility matrix for the native protocols and every official add-on.
- Prove that excluding AG-UI, Vercel AI SDK, and OpenAPI add-ons leaves the same native Open Responses/ACP/A2A model and agent behavior and does not link optional projection dependencies.

### Phase 8: Otto dogfood

- Create a prebuilt sandbox image containing `pablo` and its needed utilities.
- Route one representative Otto analysis task through `pablo`.
- Preserve Otto’s semantic validator and durable workflow ownership.
- Compare output quality, validation rate, repairs, latency, cost, and failures with Cursor CLI.
- Run one bounded specialist/reviewer child and verify its context, authority, usage, and result remain independently inspectable.
- Turn real failures into fixtures and eval cases.

### Phase 9: Graphline proof

- Run one read-only Graphline conversational turn through `pablo` while retaining Graphline’s session manager.
- Prove warm workspace reuse and session resume.
- Stream text/tool/result/artifact events through the existing backend boundary.
- Replace one bespoke web-stream path with either AG-UI or the Vercel AI SDK adapter while preserving Graphline-owned persistence, authorization, and replay semantics.
- Replace heuristic activity labels with tool presentation metadata.
- Add one host-defined read tool and one proposal-only action tool.
- Run one persistent specialist child inside the same tenant/context-version boundary and expose its tree, cost, and artifacts to the host.
- Verify disconnect/replay/cancel semantics.

</details>

### Milestones and pre-releases

Version 0.1 should not be one delivery. Each focused vertical slice ends in a tagged pre-release with published binaries, protocol fixtures, and measured baselines so integrators can find contract mistakes while they are cheap.

| Tag | Slice | What a developer can do at this tag |
| --- | --- | --- |
| `0.1.0-alpha.1` | Contract plus single agent | Run one bounded task from the CLI or ACP reference client with one gateway, shell/filesystem tools, cancellation, a typed outcome, and correlated traces. |
| `0.1.0-alpha.2` | Extensibility | Use both gateways and Open Responses, one MCP tool, one Agent Skill, structured output, and two temporary local children. |
| `0.1.0-beta.1` | Interoperability | Use the basic TUI and delegate to one configured A2A remote child; run the Otto fixture; inspect measured performance and protocol compatibility. |
| `0.1.0` | Hardening | The focused section 31.1 gates pass on supported macOS and Linux targets, and unsupported operations are documented explicitly. |
| `0.2` candidate | Durable product work | Add resumable sessions, persistent children, broader MCP, Python helpers, and the Graphline proof as evidence demands. |
| Later independent releases | Optional breadth | Ship AG-UI, Vercel AI SDK, OpenAPI, advanced chaining/replay, and other preserved 0.x work without reopening the 0.1 core. |

Otto can begin experimental integration at alpha.1 and becomes the release proof by beta.1. Graphline is intentionally not a version 0.1 gate.

## 31. Acceptance criteria for the first useful release

### 31.1 Version 0.1 release gates

Version 0.1 ships when all of these focused outcomes are true:

- A developer installs one binary, sets either gateway key, and completes a useful bounded task from the CLI and basic TUI.
- A Rust embedding and a tiny TypeScript ACP reference client exercise the same runtime lifecycle as the CLI; no custom language SDK is required.
- Vercel AI Gateway, OpenRouter, and a conforming Open Responses fixture each complete a streamed tool-calling run through shared provider-neutral contracts.
- `shell.run` and the five filesystem operations execute, stream bounded results, cancel, clean up their processes, and obey exact static yolo policy.
- One stdio MCP server and one Streamable HTTP MCP server each expose and run a tool; one local Agent Skill is discovered, explicitly activated, and uses its selected resource.
- Two depth-one temporary local children run concurrently through typed in-memory ACP, remain independently observable/cancellable, and cannot widen root authority or exceed the shared atomic budget.
- One child’s schema-valid bounded result or workspace artifact reference becomes a later child’s input without copying its transcript.
- `pablo` completes and cancels one task against a reference A2A server over the selected HTTP/SSE binding, receives one bounded Artifact, and represents the remote side as an untrusted proxy.
- A generic stable ACP v1 client initializes, creates an in-memory session, prompts, receives ordered updates and a typed outcome, and cancels; unsupported optional methods are negotiated or rejected explicitly.
- Tool and final-output schemas use JSON Schema Draft 2020-12 consistently, and one invalid final output can be repaired once in the same run.
- A real OTel Collector receives correctly parented traces for the run, model, tools, MCP, local child, and A2A task over OTLP/HTTP; incoming W3C context propagates, exported content is absent by default, exporter failure does not fail the run, and the native JSONL trace shares the same IDs.
- AG-UI, the Vercel AI SDK adapter, OpenAPI, Python helpers, durable sessions, and graph scheduling are absent without changing the focused core behavior.
- One Otto bounded read-only analysis succeeds end to end through the freshly built binary and TypeScript ACP client on supported macOS and Linux targets.
- Binary size, cold start, idle RSS, event latency, native-trace cost, in-memory ACP overhead, and warm/cold A2A overhead are measured and published. Initial numbers establish the baseline; they are not retroactive guessed release ceilings.

### 31.2 Preserved full-product acceptance inventory

The following criteria remain useful for later releases and prevent the architecture from painting itself into a corner. They are not version 0.1 blockers unless promoted into section 31.1:

<details>
<summary>Show later acceptance criteria</summary>

- A developer can install one binary, set one gateway key, and run a useful task.
- A Rust developer can embed the supported core without any interoperability add-on, supply or select a provider, and run the same model/tool/agent lifecycle used by the CLI.
- Excluding AG-UI, Vercel AI SDK, and OpenAPI add-ons removes their dependencies and network surface without changing native Open Responses, ACP, A2A, or core event semantics.
- Shell commands and long-running processes work reliably in a real terminal and a headless child process.
- A configured MCP server can expose a tool without custom Rust code.
- A standard Agent Skill can be discovered and activated without conversion.
- TypeScript and Python hosts can start a run, stream events, service a host-tool callback, cancel it, and receive a typed terminal outcome.
- TypeScript and Python hosts interoperate through stable ACP v1 initialization, sessions, prompts, updates, reverse client capabilities, cancellation, and negotiated `pablo` extensions rather than a second proprietary JSON-RPC lifecycle.
- Tool, result, handoff, and output schemas validate consistently as JSON Schema Draft 2020-12 regardless of whether they originated in Rust, TypeScript, Python, MCP, OpenAPI, or a file.
- A root can start at least two children concurrently, observe each independently, wait for either/all, stop one without stopping the other, and receive typed outcomes.
- A root or host can pass one child’s validated outcome and artifact references into a dependent child, fan out independent work, fan results in under an explicit join policy, and replay the resulting execution graph without copying complete child transcripts.
- A named persistent child can receive a later message after the runtime process restarts while preserving immutable root ownership.
- Child tools and model overrides cannot exceed inherited authority, and all child usage rolls up to the root budget without concurrency overspend.
- Every model and tool step is attributable in a trace.
- Traces show exact model, provider, skills, tools, usage, cost, retries, and configuration fingerprints without leaking configured secrets.
- OTel telemetry is emitted natively even when no `RunEvent` consumer or OTLP exporter is attached; the lossless trace shares its trace/span identities and timestamps.
- A real OTel Collector receives valid traces, metrics, and logs for a run containing a model call, shell tool, MCP call, two child agents, and a fan-in handoff.
- An incoming `traceparent` produces the expected remote-parent relationship, fan-in produces span links, separate resumed turns share `gen_ai.conversation.id`, and content is absent from OTLP by default.
- An unavailable OTel exporter cannot fail, materially delay, or alter an otherwise successful agent run.
- Native Open Responses support completes a streamed tool-calling run against a conforming endpoint and reports every unsupported or lossy feature explicitly.
- A generic AG-UI client can start, observe, reconnect to, steer or interrupt where supported, and cancel a run without creating a second execution path.
- A Vercel AI SDK `useChat` example consumes the official `UIMessage` stream adapter with stable text, tool, data, artifact, error, and finish parts.
- The OpenAPI add-on imports one allowlisted 3.1 operation, invokes it through the ordinary tool policy/trace path, and cannot obtain credentials or authority from the description document.
- A stable ACP v1 client can initialize, create or load a session, prompt, receive updates, use negotiated client capabilities, and cancel the native `pablo` agent; the same ACP handlers carry a local child over in-memory and subprocess transports.
- Two independently hosted `pablo` instances interoperate natively over A2A 1.0: one publishes a bounded Agent Card, the other runs and cancels a remote task, streams status, receives a bounded artifact, and represents it as an untrusted remote proxy child.
- Static deny rules apply even though execution has no approval prompts.
- One Otto analysis task succeeds end to end with domain validation and bounded repair.
- One Graphline-style session can resume across turns without relying on a TUI process remaining alive.
- The built binary is exercised on supported operating systems, not merely unit tested.
- The provisional performance budgets in section 28.1 are measured in CI on both supported platforms, ratified or revised with recorded reasons, and published with the release.
- A fixture turn produces a byte-identical prompt prefix on every step for each provider adapter, and the Otto dogfood run reports a cache-read share of input tokens at or above the provisional target in section 28.1 on steps after the first.
- Tool calls emitted together in one model response run concurrently, return results in call order, and fail independently.

</details>

## 32. Testing strategy

This is the full 0.x test inventory. Version 0.1 CI implements the tests needed to prove sections 29.1 and 31.1; a test for a preserved later feature becomes mandatory only when that feature is promoted into a milestone. Protocol parsers, policy boundaries, shell execution, cancellation, and redaction still receive unit, fuzz, integration, and real-binary coverage in 0.1 because they are foundational risk, not optional polish.

### Version 0.1 minimum test matrix

- **Unit:** provider stream reduction, tool-loop states, JSON Schema validation/repair, exact policy decisions, atomic budgets, event ordering, ACP/A2A mappings, OTel topology, and redaction.
- **Fuzz/property:** streamed provider/SSE chunks, JSON-RPC/ACP/MCP/A2A decoding, unknown fields, size limits, slow consumers, path handling, and terminal-safe output.
- **Integration:** fake provider with a shell round trip; process cancellation/cleanup; stdio and Streamable HTTP MCP tools; one Skill resource; two in-memory local children; one reference A2A server; and OTLP/HTTP export to a real Collector.
- **End to end:** drive the freshly built CLI, basic TUI, TypeScript ACP reference client, both gateways behind credentialed flags, and the Otto fixture on supported macOS and Linux targets.
- **Performance:** record the focused measurements named in section 31.1 and fail only against budgets ratified from evidence.

<details>
<summary>Show the complete 0.x test inventory</summary>

### 32.1 Unit tests

- Provider request serialization and stream parsing.
- Tool-loop transitions and invalid model tool calls.
- Agent-tree identity, ownership, depth, naming, and lifecycle transitions.
- Child authority intersection and attempted privilege widening.
- Hierarchical budget reservation, release, roll-up, and concurrent exhaustion.
- Child queue fairness and ancestor/descendant wait deadlock prevention.
- Chain dependency readiness, join policies, typed handoffs, checkpoint reuse, and wait-cycle rejection.
- Message delivery, acknowledgement, and bounded result projection into parent context.
- Event ordering and sequence IDs.
- Shared JSON-RPC request/response/notification rules plus ACP initialization, bidirectional ID ownership, standard framing, size limits, and cancellation races.
- JSON Schema Draft 2020-12 supported vocabulary, local and configured external references, cycles, `format` policy, canonical digests, and bounded validation errors.
- Lifecycle-to-OTel and lifecycle-to-native-trace emission, span kinds, names, status, links, shared identities, and trace-context encoding.
- Semantic-convention attribute types, required fields, custom `pablo.*` namespace, and metric-cardinality allowlists.
- Retry ownership and delivery certainty.
- Shell request validation.
- Process lifecycle and cancellation.
- Config precedence and static policy decisions.
- JSON Schema validation and repair state.
- Skill frontmatter parsing, collisions, root safety, and resource traversal.
- MCP result normalization and truncation.
- Secret masking.
- Session encoding and migration.
- Context compaction invariants.
- Cacheable-prefix byte stability across steps, breakpoint placement and pruning per provider adapter, and cache-break event attribution.
- Open Responses item/content/tool/stream mapping and explicit loss diagnostics.
- AG-UI lifecycle/message/tool/state/activity/interrupt mapping, extension fields, and disconnect policy.
- Vercel AI SDK `UIMessage` part identity and event reduction.
- OpenAPI parameter/body/response/security metadata projection, operation naming, references, redirects, and effect defaults.
- ACP v1 session/update/capability/error mapping without authority widening.
- A2A 1.0 Agent Card, message/task/status/artifact/cancellation mapping and remote-node classification.

### 32.2 Property/fuzz tests

- Provider stream chunks split at arbitrary byte boundaries.
- MCP JSON-RPC frames and malformed responses.
- Native JSON-RPC frames, interleaved reverse requests and notifications, unknown fields/methods, malformed IDs, oversized frames, and slow consumers.
- JSON Schema and OpenAPI reference graphs, adversarial regex/schema inputs, recursive documents, and decompression/response bounds.
- Native Open Responses, ACP, and A2A plus optional AG-UI and AI SDK stream chunks split at arbitrary byte boundaries with unknown future variants.
- Skill frontmatter and path resolution.
- Event protocol decoding with unknown fields.
- ANSI/terminal-safe command output.
- Output truncation preserving valid envelopes.

### 32.3 Integration tests

- Fake streamed model with multiple tool rounds.
- Real local shell commands.
- PTY interaction.
- Local stdio MCP server.
- Test Streamable HTTP MCP server.
- Skill invoking a bundled script and MCP tool.
- Host-tool callback through TypeScript and Python SDKs.
- W3C context round trips through TypeScript/Python SDKs, host-tool callbacks, and MCP `params._meta`.
- OTLP/HTTP export to a real OTel Collector with traces, metrics, logs, batching, retry, flush, and exporter-failure cases.
- Two parallel temporary children with independent model/tool loops and one parent waiting for any/all.
- Sequential handoff, bounded fan-out/fan-in, partial failure under each join policy, and downstream restart from checkpointed upstream outcomes.
- Named persistent child creation, process restart, resume, follow-up message, and explicit removal.
- Child failure isolation, targeted stop, and root cancellation propagation across descendants.
- A child using a specialized skill and dynamically selected MCP tool under narrowed authority.
- Cancellation during model, shell, MCP, and host-tool phases.
- Resume after process restart.
- JSON Schema failure followed by successful repair.
- Open Responses provider run with streaming text, multiple tool rounds, structured output, usage, and an unsupported capability.
- AG-UI HTTP/SSE run with reconnect cursor, client tool, interrupt, cancellation, and subagent extension events.
- Vercel AI SDK route consumed by a real `useChat` test application.
- OpenAPI 3.1 import and invocation through the normal policy path, including rejected remote references and credential-free model context.
- ACP stable v1 session exercised through the official Rust test client.
- A2A 1.0 server/client pair covering discovery, streaming task work, follow-up input, artifact transfer, cancellation, and remote failure uncertainty.
- Core/default build proving Open Responses, ACP, and A2A are present, while a build without AG-UI and OpenAPI proves those optional add-ons do not affect canonical outcomes.

### 32.4 Protocol conformance and compatibility tests

- Pin an upstream specification, schema, SDK, or acceptance-suite revision for every native interoperability protocol and official add-on.
- Run upstream conformance suites when they exist and retain sanitized wire fixtures for every claimed operation.
- Test the oldest and newest supported native ACP/A2A and `pablo` extension versions against SDKs, peers, and every optional companion process.
- Test the supported upstream-version matrix rather than only each add-on’s newest dependency.
- Compare native events with every outward projection to detect missing terminal outcomes, duplicated tool calls, unstable IDs, reordered lifecycle transitions, and cancellation loss.
- Fail CI when the documented compatibility manifest differs from exercised versions or features.
- Treat experimental ACP v2 or other draft support as a separate matrix that cannot satisfy stable-release gates.

### 32.5 End-to-end tests

- Run the built `pablo` binary in a real terminal.
- Drive the TUI through tmux or a deterministic virtual terminal.
- Run headless JSON/protocol mode and assert stdout remains valid.
- Exercise both supported gateways behind credentialed smoke-test flags.
- Exercise one real E2B sandbox without making E2B part of core.
- Drive a real multi-agent run through the built binary and inspect the live agent tree.
- Exercise native ACP and A2A directly from the built `pablo` binary, then start each optional companion binary against `pablo acp --stdio` and exercise it with a protocol-native client.
- Verify no network listener exists in the ordinary core/default process unless the user explicitly starts a network-facing adapter.
- Dogfood Otto and Graphline paths.

### 32.6 Performance budgets

Section 28.1 sets the provisional numeric budgets and how each is measured; section 28.10 describes the protocol paths whose overhead must be published. CI runs `hyperfine` for cold start, `cargo bloat` for size attribution, Criterion benchmarks for the event path, request assembly, schema validation, and child spawn, an RSS sampler for idle memory, and a slow-consumer soak test for backpressure. Results attach to every tagged build.

Track:

- Cold startup, binary size, and idle memory for the default distribution, the headless build, and each optional companion binary.
- Time from model event receipt to event publication.
- Tool-discovery and context-assembly overhead.
- Prompt prefix stability and cache-read token ratio per turn.
- Trace serialization overhead.
- OTel instrumentation and OTLP batching overhead with export disabled and enabled.
- Per-child supervisor overhead and scheduler throughput under bounded concurrency.
- In-memory ACP, ACP stdio, and A2A overhead above direct-call and pooled-HTTP baselines, plus adapter translation latency, buffering, and memory under slow AG-UI, AI SDK, ACP, and A2A consumers.

</details>

## 33. Security and trust model

Version 0.1 is intentionally powerful and not a containment system.

### 33.1 Threats to acknowledge

- Prompt injection in user files, web content, MCP resources, and tool results.
- Malicious or compromised skills and their scripts.
- Malicious project-provided MCP configuration.
- Shell commands that mutate or exfiltrate data.
- MCP tools with unexpected external side effects.
- Secret leakage through command output, errors, prompts, or traces.
- Duplicate side effects after ambiguous network failures.
- Unbounded output, processes, context, or cost.
- Cross-session or cross-tenant data leakage in an embedding host.
- Network-facing add-ons exposed without authentication, TLS, rate limits, tenant binding, or explicit agent-profile selection.
- Confused-deputy behavior where AG-UI client tools, ACP client capabilities, OpenAPI operations, or remote A2A agents are mistaken for locally authorized capabilities.
- Prompt injection or oversized content in OpenAPI descriptions, Agent Cards, remote-agent messages/artifacts, protocol metadata, and UI state.
- Delegation used to amplify cost, evade tool policy, leak parent context, create unbounded descendants, or orphan background work.
- Chain handoffs used to smuggle instructions, disclose data to a node without matching authority, create cyclic waits, or multiply speculative work.

### 33.2 Version 0.1 posture

- Execution is yolo within the capabilities the host configures.
- The runtime emits visible evidence of every command and tool call.
- Static policy can deny capabilities but is not represented as secure sandboxing.
- Hosts handling untrusted work should run the binary inside an actual sandbox.
- Credentials remain outside model context.
- Results and traces are bounded and redacted.
- Children inherit a strict authority ceiling, reserve bounded resources before execution, and remain owned throughout their lifecycle.
- Business invariants and consequential writes remain deterministic host responsibilities.
- Remote side effects should use typed host/MCP tools with clear result state and idempotency where possible.

### 33.3 Instruction authority

Define prompt precedence explicitly. A likely order is:

1. Runtime safety and protocol invariants.
2. Host system/developer instructions.
3. Root user intent and any user-supplied constraints included in the child context.
4. The parent’s bounded delegation task and child-specific behavioral overlay, neither of which can manufacture user authority or widen host policy.
5. Activated skill instructions.
6. Untrusted files, MCP content, tool results, and retrieved data.

Skills are intentionally procedural instructions, but they cannot override runtime/host policy or claim authority derived from untrusted evidence. MCP prompts should be explicitly invoked rather than silently treated as higher-priority system instructions.

## 34. Key design principles

1. **Headless first.** If a capability exists only in the TUI, it is not part of the reusable runtime.
2. **Powerful by composition.** Shell, MCP, skills, and host tools provide breadth.
3. **Agents are recursive runs.** Root and child agents use one engine and differ through identity, ownership, context, and limits rather than separate implementations.
4. **Ownership is a tree; execution may be a graph.** Dependency edges compose agents without transferring ownership or authority.
5. **Pass results, not histories.** Chains move typed outcomes and immutable references rather than duplicating transcripts and files.
6. **Delegation never widens authority.** A child receives the intersection of host, ancestor, and child-specific constraints.
7. **The host owns truth.** The transcript is not a business database.
8. **The sandbox owns containment.** Command filters do not.
9. **Capability is separate from authority.** A tool can exist without being allowed by a given host configuration.
10. **Yolo is separate from static policy.** No prompts does not mean ignore explicit denies.
11. **Typed boundaries over convention.** Agents, runs, events, tools, outputs, artifacts, sessions, messages, handoffs, and errors have schemas.
12. **Progressive disclosure.** Do not place every skill, MCP tool, child history, or reference in every prompt.
13. **Observable by construction.** Every user-visible result traces back to agent, model, tool, prompt, skill, and input identities.
14. **Visibility without private reasoning.** Show actions and evidence, not hidden chain-of-thought.
15. **Deterministic invariants stay outside the model.** Especially authorization, calculations, idempotency, and canonical writes.
16. **One execution path.** Root agents, children, chains, interactive mode, JSON, SDK, and web integrations use the same core and events.
17. **Bound everything.** Context, hierarchy depth, child count, graph width, concurrency, steps, time, cost, output, retries, processes, and trace content.
18. **Preserve raw fidelity at boundaries.** Normalize for the loop while retaining provider/tool provenance and structured content.
19. **Real workloads define quality.** Otto and Graphline are more meaningful benchmarks than toy chat demos.
20. **Fast by design.** Startup, streaming, prompt caching, and persistence have budgets that CI enforces; overhead the developer cannot notice is the goal.

## 35. Known tensions and unresolved decisions

These questions remain open and should be resolved deliberately:

### Naming and packaging

- Which package managers are supported first?

### Native protocols and optional add-ons

- Which exact stable ACP v1 schema and official Rust SDK revisions are pinned for version 0.1?
- Which internal callers go through typed ACP handlers and which call the lower runtime API directly while preserving the same lifecycle and outcomes?
- What is the smallest namespaced set of ACP `_meta` keys, `_`-prefixed methods, capability bits, schemas, and fallback rules needed for `pablo` authority, budgets, graph identity, replay, typed outcomes, and trace correlation?
- How are host callbacks, root/child updates, messages, waits, and outcomes multiplexed over ACP with bounded buffering, fair backpressure, and globally stable sequence identities?
- How are large binary and multimodal values represented through ACP content/resource references without repeated base64 expansion or copies?
- Which external ACP child executables may be launched, how are their commands configured, and which inherited capabilities are advertised to them?
- Which one A2A 1.0 binding ships first: JSON-RPC over HTTP with SSE streaming or HTTP+JSON with SSE streaming?
- Which official A2A Rust SDK crates and features enter the ordinary binary? Version 0.1 biases toward core types plus only the selected client binding; the later server binding must still exclude gRPC/`tonic`, SLIMRPC, and push infrastructure unless measured use justifies them.
- What are the Agent Card trust, validation, cache, refresh, and endpoint-identity rules?
- How do ACP session IDs and A2A context/task/artifact IDs map to durable `pablo` identities without conflating remote identifiers with local ownership?
- How are remotely reported A2A usage, cost, authority, and budget distinguished from locally enforced values in events, traces, and the TUI?
- Which compatibility guarantees begin at version 0.1, and how are native-protocol and optional-add-on versions negotiated and published independently?
- How are official AG-UI, Vercel AI SDK, and OpenAPI add-ons versioned and tested without making them dependencies of the native runtime?

### Session storage

- Default storage path and on-disk format. Proposed in section 28.7: `.pablo/sessions/<id>/` with an append-only event log, turn-end snapshots, and content-addressed blobs.
- Whether history is a log, snapshots, or both. Proposed in section 28.7: both; resume loads the latest snapshot and replays the tail.
- How session encryption works when content is sensitive.
- How a host supplies its own session store without linking Rust.
- How persistent child registries and in-flight child recovery are encoded transactionally.

### Model loop

- Default model for each gateway.
- Exact system prompt and context-compaction policy.
- When general parallel tool execution should follow version 0.1 and which effect metadata must exist first. Section 28.5 preserves the bounded design; ordinary 0.1 tool calls execute in model call order.
- When parallel execution lands, whether model-generated shell calls and MCP calls share one concurrency limit. The preserved proposal in section 28.5 uses one global limit plus a per-server MCP limit.
- Whether child-agent model calls share provider concurrency pools with the parent and how reservations prevent starvation.
- How provider reasoning state is persisted portably.

### Subagents

- Exact model-facing operation names and whether `run` waits or returns immediately.
- Default active-child, descendant, total-child, and depth limits.
- Initial workspace modes beyond `shared`.
- Persistent-child retention, discovery, deletion, and direct-resume policy.
- Whether children may themselves create persistent children after persistent sessions and recursive delegation are introduced.
- Message delivery and acknowledgement guarantees across process crashes.
- Scheduler strategy and deadlock prevention when ancestors wait on descendants.
- Default budget reservation algorithm and unused-budget release behavior.
- How much child context/result content is automatically projected back into the parent.

### Agent chaining

- Whether chain identities are created only by hosts or may also be created dynamically by a root model.
- Exact normalized handoff shape and its mapping to ACP content/resource references, A2A structured-data Parts/Artifacts, and immutable local content references.
- Whether dependency scheduling belongs in the first protocol version or is initially expressed through explicit spawn and wait calls.
- Which join policies ship first and how partial failures are projected downstream.
- Whether a node retry preserves `node_id` while always receiving a new `attempt_id` and sometimes a new `agent_id`.
- How checkpoints are retained, invalidated, and keyed by input/configuration fingerprints.
- Whether direct sibling messaging is ever supported or all handoffs remain supervisor-routed; version 0.1 uses supervisor-routed bounded outcomes only.
- How dynamic branches appear in an otherwise host-authored execution graph.
- What prompt-cache and context-duplication measurements can be normalized across providers.

### MCP

- Exact stable MCP specification version targeted at release.
- Whether OAuth must ship in 0.1 for practical remote-server compatibility.
- Whether prompts are model-selectable or user/host-invoked only.
- Whether elicitation is necessary for the first Graphline integration.
- Whether an MCP server may request model sampling in early releases.

### Skills

- Exact compatibility roots enabled by default.
- Matching algorithm and threshold. Proposed in section 28.9: model selection from the catalog by default, a lexical prefilter for large catalogs, and an optional host-supplied matcher.
- Whether automatic activation is enabled by default for all sources or only trusted sources.
- Lockfile format for remote-installed skills.
- How skill diagnostics appear in noninteractive mode.

### Static policy

- Exact command-pattern language.
- Whether workspace config may add executable MCP servers automatically.
- Which restrictions are runtime-enforced versus declared as host responsibilities.
- Whether local first run should display a non-blocking yolo warning. Proposed in section 27.2: yes, one stderr line, shown once.
- Whether the configuration key is spelled `mode = "yolo"` or `approval = "none"`. The behavior is settled; the name a compliance reviewer reads in an embedded product's config file is not. `--yolo` can remain a CLI alias either way.
- How config validation proves that no child override can widen an ancestor’s effective policy.

### TUI

- Terminal rendering library.
- Inline transcript versus alternate-screen ownership.
- How much trace detail appears by default.
- How the live child-agent tree, status, messages, and per-child costs are presented without overwhelming the transcript.
- Whether the TUI lives in the main binary through Cargo features or a separate binary.

### Artifacts and replay

- Content-addressed blob interface.
- Default trace content/retention policy.
- How volatile inputs are snapshotted without accidental sensitive-data duplication.
- Deterministic tool-result replay format.

### OpenTelemetry

- Exact pinned GenAI/MCP semantic-convention release or immutable revision for version 0.1.
- Whether the default binary uses the upstream Rust OTel SDK directly or a thinner internal signal layer with an OTLP adapter.
- Which Rust OTel log/event APIs can preserve structured GenAI content without lossy flattening.
- Whether OTLP/gRPC ships only as a Cargo feature or as a separate prebuilt distribution.
- Exact parent-versus-link rules for asynchronous children, persistent-child continuation, and host workflow spans.
- How Vercel AI Gateway and OpenRouter map gateway and resolved upstream provider identities without misusing `gen_ai.provider.name`.
- Which custom `pablo.*` metrics ship initially and their stable units, boundaries, and attribute allowlists.
- Bounded flush deadline and offline buffering policy for short-lived CLI processes.
- How native trace sampling/content policy and OTel sampling/content policy are explained without surprising users.
- How compatibility migrations preserve queries when Development GenAI attribute names change.

### Performance

- Which provisional budgets in section 28.1 survive Phase 1 measurement, and which platforms gate CI.
- Whether `webpki-roots` or the platform trust store is the default certificate source.
- Default delta-coalescing thresholds for ACP stdio and SSE consumers.
- Whether the default distribution's `opt-level` favors size or speed after measurement.
- Whether the blob threshold for content-addressed storage is 64 KiB or lower.
- Default cache TTL policy per run shape, and how breakpoints are allocated when a provider allows fewer than the runtime wants.
- Whether the long-tail dispatcher or native promotion is the default for MCP catalogs above the advertised set, and the size of the default advertised set.

## 36. Decisions to make before writing substantial implementation

The first architecture session should resolve only decisions that block the focused vertical slice, in order:

1. The smallest public `RunSpec`, `RunEvent`, `RunOutcome`, `Tool`, temporary `AgentRef`, and error shapes required by the Otto fixture.
2. The provider-neutral message/content/tool-call model, shared gateway transport, and native Open Responses mapping for one tested profile per provider path.
3. The exact JSON Schema Draft 2020-12 subset; shell/filesystem tool contracts; cancellation and hard-limit semantics; and exact static yolo policy matcher.
4. The pinned ACP v1 methods and minimal extensions for the host and in-memory local children, plus the pinned A2A 1.0 HTTP/SSE operations and Agent Card policy for one remote proxy.
5. The temporary depth-one child lifecycle, shared atomic root budget, authority intersection, two-child concurrency rule, bounded outcome, and direct handoff representation.
6. The native OTel trace topology, W3C propagation, default content redaction, JSONL correlation, OTLP/HTTP failure behavior, and the small metric set that can be implemented without broadening the slice.
7. The exact Otto acceptance fixture and the measurements recorded for the first performance baseline.

Session storage, persistent children, generic graph scheduling, the full MCP and Skills surfaces, custom SDKs, optional add-ons, advanced cache policy, complete OTel signal coverage, and Graphline remain questions in section 35 but do not block implementation.

Avoid starting with TUI rendering or a large provider abstraction before these contracts exist.

## 37. Proposed first design spike

### 37.1 Focused first spike

The first spike stops after proving one complete path:

1. Define minimal `RunSpec`, `RunEvent`, `RunOutcome`, `Tool`, and error types plus the ACP mapping they require.
2. Start `pablo acp --stdio` from a tiny TypeScript client, initialize, create an in-memory session for a temporary workspace, and send one prompt.
3. Call a fake streamed provider and one real gateway profile through the same provider boundary.
4. Let the model invoke one bounded `shell.run` command and return the result for one additional model step.
5. Stream ordered ACP updates, cancel one fixture run, and return an unambiguous typed terminal outcome.
6. Write the bounded JSONL trace while a local OTel Collector receives the correlated run, model, and tool spans over OTLP/HTTP with content disabled.
7. Run the built binary end to end and record size, startup, idle RSS, event latency, trace overhead, and ACP stdio overhead.

The spike succeeds when these seven steps work through one lifecycle with no TUI-only state, no duplicate telemetry path, no proprietary process protocol, and no dependency on sessions, subagents, MCP, Skills, A2A, or optional adapters. Only then start the extensibility slice in section 30.

### 37.2 Preserved broader integration spike

The following sequence remains the integration proof for the fuller architecture, not the first implementation task:

<details>
<summary>Show the broader integration spike</summary>

1. Create the versioned core contracts plus the native JSON Schema, JSON-RPC, Open Responses, ACP, and A2A modules.
2. Start `pablo acp --stdio` from a tiny TypeScript ACP client.
3. Initialize ACP, negotiate capabilities and `pablo` extensions, create a session rooted at a temporary workspace, and send one prompt with a bounded run configuration.
4. Call one gateway model through the provider-neutral boundary and exercise the same tool loop against a conforming Open Responses fixture.
5. Let the model run one shell command.
6. Stream ordered ACP session updates back to TypeScript and return a typed terminal outcome.
7. Persist a trace and replay its presentation.
8. Add one local stdio MCP tool.
9. Activate one standard Agent Skill containing a small script.
10. Have the root spawn two bounded local children through typed in-memory ACP dispatch, stream their events concurrently, and wait for typed outcomes.
11. Stop one local child independently and prove the other child and parent remain live.
12. Feed one child’s schema-valid result and artifact reference into a dependent child using ACP-native content shapes without copying its transcript.
13. Start a second `pablo` instance as an explicitly configured A2A server, validate its Agent Card, and represent one streamed remote task as a local proxy child.
14. Cancel one remote task and receive one bounded A2A Artifact while preserving the distinction between remotely reported and locally enforced state.
15. Replay the resulting ownership tree and execution graph, including local and remote handoffs and the join decision.
16. Start a local OpenTelemetry Collector, inject an incoming `traceparent`, and export the run’s traces, metrics, and correlated logs over OTLP/HTTP Protobuf.
17. Verify child causality and fan-in links in the exported trace, exact correlation with the native trace, and the absence of prompts, commands, tool content, and credentials under the default OTel content policy.
18. Measure the no-subagent baseline, typed in-memory ACP dispatch, ACP stdio transport, and A2A work above a pooled direct-HTTP baseline so native support has an explicit startup, size, memory, and latency cost record. Record every number against the provisional budgets in section 28.1 and ratify or revise them.
19. Assert that the prompt prefix is byte-identical across every step of the spike's tool-calling turn, that the provider reports cache reads on the second and later steps, and that the two child spawns in step 10 dispatched concurrently as parallel tool calls.

The spike succeeds only if the root and children use the same model loop, shell, MCP, skill, event, budget, and trace contracts while retaining distinct identities and lifecycles; ACP and A2A reuse standard lifecycle/content semantics instead of creating a second proprietary protocol; chaining adds only typed dependencies and data movement rather than a second execution engine; unused A2A support performs no network or listener work; and each lifecycle transition creates correct native OTel telemetry together with a replayable lossless trace record. That validates the core thesis before investing in a polished TUI. The spike also produces the first ratified performance baseline.

</details>

## 38. Source references

### Existing codebases

- `fx`: `https://github.com/vercel-labs/fx` at `4351daf29ccf510563bcc63001ffc65b0c7bbe0f`
- OttoV3: `https://github.com/calebjohn24/ottov3` at `ff40ad18327a6b3d7ee0b748f0a5e37f27329df9`
- Graphline: `https://github.com/Deirfgeiz/Graphline` authoritative `origin/main` at `d5942127cd38a59b306db59708e7e6ee0632c6e9`

### Open standards and providers

- Agent Skills: `https://github.com/agentskills/agentskills`
- Agent Skills specification: `https://github.com/agentskills/agentskills/blob/main/docs/specification.mdx`
- JSON Schema Draft 2020-12: `https://json-schema.org/draft/2020-12`
- JSON-RPC 2.0 specification: `https://www.jsonrpc.org/specification`
- Open Responses: `https://www.openresponses.org/`
- ACP stable v1 overview: `https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v1/overview.mdx`
- ACP stable v1 schema: `https://github.com/agentclientprotocol/agent-client-protocol/blob/main/schema/v1/schema.json`
- Official ACP Rust SDK: `https://github.com/agentclientprotocol/rust-sdk`
- A2A specification: `https://a2a-protocol.org/latest/specification/`
- A2A key concepts: `https://a2a-protocol.org/latest/topics/key-concepts/`
- Official A2A SDK index: `https://a2a-protocol.org/latest/sdk/`
- Official A2A Rust SDK: `https://github.com/a2aproject/a2a-rs`
- Official A2A Technology Compatibility Kit: `https://github.com/a2aproject/a2a-tck`
- MCP specification: `https://modelcontextprotocol.io/specification`
- Official MCP Rust SDK: `https://github.com/modelcontextprotocol/rust-sdk`
- AG-UI documentation: `https://docs.ag-ui.com/`
- Vercel AI SDK UI stream protocol: `https://ai-sdk.dev/docs/ai-sdk-ui/stream-protocol`
- OpenAPI 3.1.2 specification: `https://spec.openapis.org/oas/v3.1.2.html`
- Vercel AI Gateway: `https://vercel.com/docs/ai-gateway`
- Vercel AI Gateway APIs: `https://vercel.com/docs/ai-gateway/sdks-and-apis`
- OpenRouter: `https://openrouter.ai/docs/quickstart`
- OpenTelemetry specification: `https://opentelemetry.io/docs/specs/otel/`
- OpenTelemetry semantic conventions: `https://opentelemetry.io/docs/specs/semconv/`
- OpenTelemetry GenAI semantic conventions: `https://github.com/open-telemetry/semantic-conventions-genai` at `fee465db333bdd6a7d2faa320edab5cf3101a4f4`
- OpenTelemetry GenAI agent spans: `https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-agent-spans.md`
- OpenTelemetry GenAI model/tool spans: `https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-spans.md`
- OpenTelemetry MCP conventions: `https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/mcp.md`
- OTLP specification: `https://opentelemetry.io/docs/specs/otlp/`
- W3C Trace Context: `https://www.w3.org/TR/trace-context/`

## 39. Final product statement

`pablo` is a small native multi-agent runtime for arbitrary work. Its useful core includes a powerful shell, filesystem tools, MCP, portable Agent Skills, JSON Schema Draft 2020-12, native Open Responses provider compatibility, stable ACP v1 for hosts and local children, A2A 1.0 for remote agents, and native OpenTelemetry. Root and child agents share one loop and compose through protocol-native typed handoffs and bounded dependency graphs without interactive permission friction inside a developer-chosen execution boundary. Every agent, dependency, message, tool call, budget, and result is visible through native OTel signals plus a lossless replay trace sharing the same identities and lifecycle. Applications retain ownership of their sandboxes, durable workflows, permissions, business state, approvals, telemetry backends, and user experience. Optional AG-UI, Vercel AI SDK stream, and OpenAPI add-ons project the same runtime without becoming alternate execution paths. The same core serves a one-shot analysis job like Otto, a chained or delegated parallel workflow, and a durable conversational workspace like Graphline, while the `pablo` CLI/TUI provides the simplest way to run, inspect, tune, and debug it.

