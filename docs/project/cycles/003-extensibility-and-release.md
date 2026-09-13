# Cycle C3: extensibility, interoperability and release delivery

## Objective and authority

Build on completed C1 and C2. Select declarative deployment configuration, ordered model fallback, basic bounded-run context compaction, shell command allow/deny rules, OpenRouter, native Open Responses, MCP over stdio and Streamable HTTP, local Agent Skills, structured-output validation and one repair, temporary children, A2A client delegation, a basic TUI, and release packaging, compatibility, measurements and distribution. The user explicitly excludes Otto integration from C3.

The [design brief](../../context.md), sections 29.1 and 31.1, retains authority over the full 0.1 release. Excluding Otto from this cycle does not remove its release gate. [State](../state.json) alone records checkpoint status; retain the C1/C2 plans, checkpoint objects, evidence and append-only log.

The [scope and contract map](../contracts/c3-extensibility-and-release.md) records the C2 baseline and decisions that each implementation checkpoint must freeze. The [fixture map](../fixtures/c3-extensibility-and-release.md) specifies observable examples. These are requirements for future work, not implemented APIs or passing tests. C3 freezes detailed contracts immediately before their small owning slice so each design review stays focused.

The [deployment configuration design](../contracts/c3-deployment-config.md) applies the user's NixOS-style request: versioned typed options, local modules/imports, named profiles, deterministic composition, secret references, explainable effective values and reproducible deployment presets. Use TOML as suggested by design section 20; Pablo's binary does not require a Nix evaluator. Ordered fallback and shell command matching are explicit additions to the earlier C3 draft. Every later subsystem must extend this shared configuration contract when it becomes usable.

## Cadence and sequencing

Complete one checkpoint per implementation session, including evidence and handoff, then stop. Start with the lowest-numbered ready checkpoint unless the user selects another ready one. Dependencies describe actual prerequisites, not progress; readiness and the selected next checkpoint live only in state. Contract changes require a recorded decision and fixture update before implementation; never weaken an expectation to fit a failing result. If a checkpoint still proves too large, split its remaining work with stable new IDs and explicit dependencies while retaining its history.

| Checkpoints | Result |
| --- | --- |
| C3.0 | Adopt this plan and acceptance map |
| C3.1–C3.4 | Define deployment options, compose configs, inspect effective values and enforce shell command rules |
| C3.5–C3.7 | Select providers and prove OpenRouter offline, then live |
| C3.8–C3.9 | Freeze and implement the Open Responses subset |
| C3.10–C3.12 | Configure ordered model routes, execute fallback and prove failure/accounting semantics |
| C3.12a | Compact bounded-run context once, with visible summary and one overflow recovery |
| C3.12b | Summarize bulky recent results while preserving task-relevant details |
| C3.13–C3.14 | Validate structured output, then add one bounded repair |
| C3.15–C3.18 | Configure MCP, prove each transport, integrate CLI/ACP |
| C3.19–C3.20 | Discover Skills, then activate instructions and resources |
| C3.21–C3.24 | Reuse typed ACP for one child, add concurrency and typed handoffs |
| C3.25 | Accept the combined extensibility slice |
| C3.26–C3.28 | Validate A2A cards, delegate tasks, prove cancellation and remote boundaries |
| C3.29–C3.30 | Build the basic TUI, then prove it in a real PTY |
| C3.31–C3.34 | Add focused diagnostics, prove complete deployment presets, verify compatibility and prepare archives |
| C3.35–C3.38 | Accept each native macOS/Linux architecture separately |
| C3.39–C3.40 | Publish measurements, then distribute the explicitly selected prerelease |

Keep providers, tools and adapters inside the existing workspace; add modules or dependencies only when a checkpoint needs them. Preserve the shared lifecycle, static policy, conservative accounting, native telemetry and reusable process setup. Ordinary model-emitted tools remain sequential; child spawn returns a handle so two children can overlap without introducing general parallel tool dispatch.

## C3.0: Adopt the cycle and acceptance map

Prerequisites: C2.5.

Adopt this plan and its scope/fixture map as a documentation checkpoint before implementation.

Acceptance:

- Map every selected capability to a small checkpoint, prerequisites and observable acceptance; explicitly defer Otto and preserve the brief's release cut line.
- Record current C2 capabilities, shared invariants, initial synthetic fixtures and the checkpoint that must freeze each detailed contract before code changes.
- Activate C3.0 while drafting; retain state format version 1, all prior checkpoint objects, the complete log prefix and the unchanged reviewed source-context object/hash.
- Verify helper tests, project consistency, dependency/fixture coverage, document links and the final diff. Complete only C3.0 and leave C3.1 ready; this checkpoint makes no runtime or release acceptance claim.

## C3.1: Deployment configuration contracts

Prerequisites: C3.0.

Freeze the versioned declarative configuration surface and its option ownership before writing the loader.

Acceptance:

- G01 defines typed TOML options, local imports/modules, named profiles, defaults and merge/list precedence, bounded parsing/import depth, cycle/conflict detection, secret references, path bases and configuration migrations.
- Map every existing and selected subsystem to an option owner. Distinguish ordinary value overrides from immutable host/deployment authority ceilings; lock mode ignores ambient user/workspace settings and rejects prohibited overrides.
- Freeze portable preset examples, resolved-config schema/fingerprint/provenance and CLI/ACP/Rust equivalence. Unimplemented feature options fail clearly instead of being silently accepted; this checkpoint is specification-only.

## C3.2: Configuration loading and composition

Prerequisites: C3.1.

Implement the bounded loader and deterministic resolver without starting an agent or activating configured capabilities.

Acceptance:

- G02 passes: compose shared modules and development/production profiles; resolve imports relative to their declaring file, preserve ordered lists, detect cycles/conflicts/unknown options, and produce the same canonical config from the same declared inputs.
- Freeze and enforce merge semantics for scalars, maps, lists and explicit overrides. Configuration evaluation executes no scripts, fetches no imports and resolves no secret bytes; invalid documents fail before side effects.
- Locked deployments require explicit configuration inputs and allowed environment bindings. Verify that ambient working directory, user config and unapproved variables cannot change the resolved behavior; retain useful value/source diagnostics.

## C3.3: Effective configuration and deployment inspection

Prerequisites: C3.2.

Wire the resolved options into the current C2 core/CLI/ACP paths and expose offline validate/explain/render commands.

Acceptance:

- G03 passes: equivalent file/profile, CLI and embedding inputs yield the same admitted provider, workspace, tools, policy, limits, output and trace settings; unchanged invocations retain C2 defaults and credential precedence.
- `config validate`, `config explain` and canonical `config render` expose schema, effective values, sources, overridden definitions and a redacted fingerprint. Rendered presets remain deployable using secret references, without key bytes or runtime task content.
- Enforce host/deployment ceilings after ordinary precedence resolution; reject attempts to widen policy through CLI, ACP, profile or environment. Snapshot configuration per admitted run and document that changes apply to later runs.

## C3.4: Shell command allow and deny rules

Prerequisites: C3.3.

Extend C2's launcher policy with the separately specified command/argument rules in deployment configuration.

Acceptance:

- Q01 passes for allow-only, deny-only and combined rules with explicit defaults, stable rule IDs and deny precedence. Distinguish exact launcher policy from the actual command and argument matching contract.
- Restrictive profiles parse a bounded literal-argument subset and reject unsupported pipelines, substitutions, redirections, compound commands and dynamic expansion before launch. Match resolved executable/arguments without substring tests or executing a differently interpreted command.
- Prove quoting, path aliases, argument-boundary and compound-command attempts cannot bypass the selected rules. Preserve unrestricted legacy shell behavior when command rules are absent, root/child policy intersections, cancellation and metadata-only diagnostics; no OS sandbox or descendant inspection is claimed.

## C3.5: Provider selection and shared transport

Prerequisites: C3.4.

Extract only the transport/configuration seams required for a second gateway. Freeze provider selection, credential precedence, endpoint trust, resolved model/profile metadata and provider-specific capabilities.

Acceptance:

- P01 passes: current Vercel CLI/ACP behavior remains compatible; the user-selected GLM-5.3-Flash defaults supersede the prior Gemini default (D041); explicit provider selection resolves identically for CLI, ACP host configuration and Rust embedding. An unavailable adapter fails before delivery until its implementation lands.
- Credentials remain adapter-owned; synthetic fixture endpoints cannot receive real gateway credentials. Reuse the existing HTTP client, bounded SSE machinery and cancellation path; ordered fallback is introduced and verified at C3.10–C3.12.
- Record an officially verified candidate OpenRouter profile and mapping/fixture expectations for C3.6. Run existing Vercel offline regressions and the shared implementation checks; no paid call is required here.

## C3.6: OpenRouter streamed tool calls

Prerequisites: C3.5.

Implement the direct OpenRouter adapter through the shared provider boundary and wire the selected provider into CLI and ACP.

Acceptance:

- P02 passes against local HTTP/SSE fixtures: model/tool/model round trip, fragmented tool arguments, keepalive comments, accounting-only final chunks, safe pre-stream and mid-stream errors, missing usage and cancellation.
- Assemble usage/cache/cost fields without duplicate finishes or invented values. Preserve provider identity, call IDs, byte-stable instruction/tool prefixes, delivery certainty and exactly one terminal outcome.
- Prove private `OPENROUTER_API_KEY` resolution with synthetic credential fixtures, redaction across all serialized surfaces, bounded parsing and no unintended network destination. Record a runnable terminal command before C3.7.

## C3.7: Live OpenRouter acceptance

Prerequisites: C3.6.

Run a small, explicitly invoked credentialed proof using the actual executable and selected known-good profile.

Acceptance:

- P03 passes through CLI and the TypeScript ACP client: read newly generated local evidence through an allowed tool, then return its exact value. Record model/profile, actual usage, source/build identity and sanitized outcomes.
- The executable privately loads the supplied key; keep raw traces and temporary inputs ignored and clean up owned files/processes. Offline tests remain the ordinary CI path.
- Document copyable commands and remaining capability limits. Missing credentials, unavailable profile or an unrun live surface leaves this checkpoint incomplete; do not substitute a mock.

## C3.8: Open Responses contracts and fixtures

Prerequisites: C3.6.

Freeze the smallest HTTP/SSE text and function-tool subset before implementing its adapter.

Acceptance:

- OR01 defines and pins an immutable specification/acceptance revision, endpoint configuration, supported item/event state transitions, call/result correlation, continuation state, finish/usage mappings and rejected unsupported behavior.
- Specify configured capability resolution, credential ownership, byte/work limits, safe errors and cancellation. Preserve required continuation items without revealing private reasoning; do not silently flatten unsupported content.
- Check request/event fixture examples against the pinned upstream schema and existing provider contracts. This is a contract checkpoint; no Open Responses runtime support is claimed.

## C3.9: Open Responses streamed tool calls

Prerequisites: C3.8.

Implement the frozen adapter in the default core/executable and expose it through the same host configuration as the gateways.

Acceptance:

- OR02 passes through core, CLI and ACP against a conforming local fixture: text, function-call fragments, tool results, continuation and one final outcome with usage.
- Exercise invalid transitions, unknown required semantics, unsupported modalities, oversized items, disconnects and cancellation; preserve bounded provider state, first-text delivery and secret redaction.
- Record supported versus unsupported features and the resolved endpoint capability profile. A conforming fixture is the required gate; a paid Open Responses service is not required.

## C3.10: Ordered model profiles and routes

Prerequisites: C3.9.

Configure named model profiles and explicit ordered routes across Vercel, OpenRouter and Open Responses.

Acceptance:

- F01 passes: each ordered entry resolves provider, exact model, endpoint/credential reference, capabilities and model options. Preserve declared order, reject duplicate/cyclic references and unsupported output/tool requirements, and explain selection without sending requests.
- Freeze route scope, attempt limits, eligible error classes, delivery certainty, continuation compatibility and retry ownership. Children can select only an inherited approved route/subsequence and cannot append a more privileged destination.
- A single-entry route preserves current behavior. The user's ordered fallback is runtime-owned when enabled; gateway model fallback and host retries cannot silently multiply its attempt budget. Record any opaque gateway routing separately.

## C3.11: Ordered model fallback execution

Prerequisites: C3.10.

Try configured model entries in order when an eligible model attempt fails, within the existing run lifecycle.

Acceptance:

- F02 passes: first entry fails with an eligible response or proven pre-delivery error, second succeeds, and no later entry is contacted. Each attempt has its own model/provider/span identity and consumes the root call/time/accounting budget.
- Do not restart the task or replay completed tools. Preserve accepted history and the common prefix; switch only when the next adapter can faithfully represent continuation state. Stop when output/tool-call deltas have already escaped the failing attempt.
- Defaults reject uncertain-delivery retries, cancellation, policy denial, invalid configuration and incompatible continuation as fallback triggers. Any explicitly enabled uncertain model retry retains conservative charges and never implies exactly-once billing.

## C3.12: Fallback failure and accounting proof

Prerequisites: C3.11.

Exercise exhaustion, cross-provider transitions and partial-delivery failures through the actual CLI/ACP route configuration.

Acceptance:

- F03 passes: ordered two/three-entry failure, final-entry exhaustion, mid-stream loss, uncertain delivery, cancellation, missing credential and incompatible continuation all settle with precise attempt/selection metadata and one native outcome.
- Race remaining allowances with fallback attempts; count each request, retain uncertain reservations, stop at configured attempt/deadline limits and reject hard ceilings unsupported by any eligible entry before delivery. No repeated tool side effects or unconfigured destinations occur.
- Run offline Vercel/OpenRouter/Open Responses transition fixtures and the full affected provider/ACP regressions. Forced paid failures are unnecessary; later live gates exercise the selected configured profile and record the resolved model.

## C3.12a: Basic context compaction

Prerequisites: C3.12.

Implement the small bounded-run compaction required by design sections 23 and 28.8, explicitly added to C3 by the user on 2026-09-10. Take inspiration from Codex's summary-and-history replacement pattern; keep the existing provider defaults and one task lifecycle. [The scope review](../evidence/c3-compaction-scope.md) records the source and boundary.

Acceptance:

- Freeze the typed context/profile capacity options, conservative estimation/safety margin, summary/output bounds, complete-turn retention rule, overflow classification and event/privacy contract before implementation. Include instruction/tool/Skill/framing costs, use reported usage where available, and keep the binary tokenizer-free. Unknown capacity must remain explicit; no guessed live model catalog or new credential is required.
- CP01 passes through actual core, CLI and ACP: near the configured usable context threshold, perform at most one bounded model summary pass per run, then atomically replace older completed history behind the unchanged instruction/tool prefix. Keep the original task, constraints, unresolved work, artifact references and recent complete tool-call/result pairs. The summary is derived history, never new authority. Continue from actual completed effects with fewer context bytes and no tool replay or task restart.
- CP02 passes: a recognized provider context-overflow rejection can trigger that same one compaction allowance and at most one recovery attempt. Summary work and recovery consume the existing root model/time/token/cost budgets, with tools disabled during summarization. Cancellation, failed/empty/oversized summary, no available history or insufficient room/budget preserve original history and settle explicitly. No repeated summarization or hidden fallback/repair multiplication.
- Preserve required private continuation for retained turns and never flatten opaque provider state. Replace discarded history only at an explicit successful compaction boundary. Emit native/ACP/OTel compaction identity, trigger, before/after size and replaced-history fingerprint; expose summary content only through the documented content policy. Prove trace/key redaction and fresh ACP-session isolation.
- Cover Vercel/OpenRouter/Open Responses offline, retain the selected GLM gateway models, and measure the ordinary path overhead plus compaction/recovery duration and context reduction. No durable sessions, persistent memory, external retrieval, encrypted OpenAI-only compaction API, tokenizer bundle or recursive summary hierarchy is selected.

## C3.12b: Stronger task-relevant compaction

Prerequisites: C3.12a.

The user requested stronger reduction and selected preserving task-relevant details in the summary rather than exact raw-output recall. Default to summarizing all completed turns when the summary request fits; retain the fewest necessary recent complete turns if source capacity requires it. Keep explicit recent-turn retention configurable, preserve the original task/prefix and effect ledger, and strengthen the handoff prompt around exact constraints, identifiers, artifact references, decisions and unfinished work. No raw-output archive or recall tool.

Acceptance:

- Prove substantially smaller continuation context on a workload where a bulky recent result previously dominated retained history, with one summary pass and unchanged root budget/privacy boundaries.
- Cover details distributed across older and newest results, effect non-replay, local capacity fallback, explicit retention, invalid summaries, and all adapters through CLI/ACP.
- Report achieved reduction and the fixed-prefix floor, along with matched ordinary/compaction performance. Summary fidelity is task-relevant, not a claim of lossless arbitrary recall.

## C3.13: Final-output schema validation

Prerequisites: C3.12b.

Add an optional model-output schema to the existing run contract, separately from C2's task envelope.

Acceptance:

- J01 passes: compile/cache the documented Draft 2020-12 subset, including local `$defs`/`$ref`, reject unsupported assertions and external retrieval, and bound compilation, validation and error output. State `format` behavior explicitly.
- Validate final JSON locally regardless of provider schema hints. CLI JSON and negotiated ACP metadata distinguish valid structured output, unvalidated partial text and a typed validation failure; preserve generic ACP fallback and text-only runs.
- Cover malformed JSON, schema violations, oversized output and cancellation. Freeze exact schema/version and failure mappings before implementation; C3.13 performs no repair call.

## C3.14: One bounded output repair

Prerequisites: C3.13.

Permit one validation repair within the same admitted run and conversation.

Acceptance:

- J02 passes: an invalid first result receives bounded path-aware feedback and a valid second result succeeds; another invalid result ends with a typed validation failure. There is no third attempt or replacement session.
- Charge the repair to the same deadline, model/token/cost ledger and output/context/trace limits. If no allowance remains, settle without another provider request; cancellation remains effective during repair.
- Preserve the instruction/tool prefix, distinguish repair from transport retry, and record validation/repair metadata without exporting model content. Ordinary text and tool failures keep their existing semantics.

## C3.15: MCP configuration and policy contracts

Prerequisites: C3.14.

Freeze the tool-only MCP subset and implement bounded host configuration plus exact server/tool policy admission before connecting servers.

Acceptance:

- M01 passes for typed configuration: named stdio/HTTP servers, private credential references, required/optional startup behavior, qualified identities, collision rejection, server/tool/launcher deny precedence and source provenance.
- Pin the audited official Rust SDK, protocol revision and MCP OTel mapping. Freeze discovery pages/catalog/schema bytes, request/progress limits, supported result kinds, output-schema validation, timeout/cancellation and shutdown ownership.
- ACP-supplied server configuration intersects host policy and cannot install workspace execution authority. Untrusted model/Skill content cannot add a server or credential. No transport capability is advertised before its checkpoint passes.

## C3.16: MCP stdio tool path

Prerequisites: C3.15.

Implement initialization, bounded discovery and a tool call against one actual local server process through the ordinary registry.

Acceptance:

- M02 passes: a pinned independent server exposes a synthetic read tool, the model consumes its actual result, and canonical tool IDs map bijectively to provider-safe aliases.
- Validate arguments and supported structured results; distinguish protocol errors from tool errors and reject unsupported result kinds explicitly. Enforce server/tool/launcher policy before launch or dispatch.
- Bound stdout framing, stderr capture, discovery and pending work; cancellation, startup failure and run close join all owned processes and pipes. Add native MCP/tool spans without a duplicate logical tool span.

## C3.17: MCP Streamable HTTP tool path

Prerequisites: C3.16.

Add Streamable HTTP using the same normalized tool contract and a separate reference server fixture.

Acceptance:

- M03 passes for both JSON and SSE responses, initialization, negotiated version/session headers, discovery and one tool call. Handle optional GET/session teardown according to the pinned protocol.
- Bound progress and streams, preserve request IDs, scope header credentials to the configured endpoint, and reject unsafe redirects. A disconnected stream does not imply the peer cancelled its operation.
- Send protocol cancellation and bound local cleanup; document uncertain remote completion. Any supported stream resumption must not replay a tool invocation; unsupported recovery fails explicitly.

## C3.18: MCP host integration and failure proof

Prerequisites: C3.17.

Finish CLI/ACP MCP configuration and cross-surface acceptance using both implemented transports.

Acceptance:

- M04 passes through the built CLI and TypeScript ACP client: required/optional startup, fixed admitted catalogs, denied servers/tools, bounded progress, error disposition and cancellation/disconnect cleanup.
- Slow consumers and malformed/oversized peers cannot bypass C1/C2 bounds. A denied/unconfigured server never starts; MCP subprocesses receive only their explicitly selected credentials, never ambient provider/exporter secrets.
- A real Collector receives correlated MCP spans with propagated context and no content/credentials. Record connection ownership across independent ACP sessions and prevent stale catalog/authentication state leaking between them.

## C3.19: Local Skill discovery

Prerequisites: C3.18.

Discover local Agent Skills metadata from explicit host-selected roots, including the `.agents/skills/` convention.

Acceptance:

- S01 passes: valid `SKILL.md` frontmatter produces a deterministic qualified name/description catalog; invalid metadata, duplicates and ambiguous names have bounded diagnostics. Pin the format revision and resource limits.
- Reject traversal, unintended symlink escape and excessive scan/metadata input. Discovery does not load instruction bodies or resources into model context or execute bundled scripts.
- Record source identities and policy-visible roots. Skills use the standard portable format; no remote installation, implicit download or automatic matching enters this slice.

## C3.20: Skill activation and selected resources

Prerequisites: C3.19.

Expose explicit CLI/host activation, load instructions progressively, and allow selected resources through ordinary bounded capability checks.

Acceptance:

- S02 passes: explicitly activate a synthetic Skill, load its selected resource only when requested, and use that resource in the model's result. Record instruction/resource digests and deterministic activation order.
- Keep the shared prefix stable; distinguish catalog, activation and resource context costs. Resources remain inside the selected Skill root and within host authority; scripts use the existing shell lifecycle.
- A Skill naming a denied tool, MCP server or write path gains no authority. CLI/ACP/native telemetry expose activation safely, with content absent from OTel; cancelled resource work is joined.

## C3.21: Child contracts and typed ACP dispatch

Prerequisites: C3.20.

Freeze temporary child identity, authority, budgets and bounds, then extract the minimum shared ACP handlers needed for typed in-memory dispatch.

Acceptance:

- A01 passes: official ACP types exercise initialize/new/prompt/update/cancel over in-memory dispatch with no JSON serialization, preserving the stdio adapter's lifecycle, cancellation and fallback behavior.
- Specify `AgentRef`, immutable root/parent IDs, depth one, explicit enablement, two active children, bounded total children/pending queue/context, ownership and joined shutdown. Freeze model-facing spawn/wait-any-or-all/inspect/stop and equivalent host operations.
- Define the atomic root ledger, child ceilings, process/MCP/trace capacity, narrowed tool/Skill/MCP/provider views and per-agent event attribution before any child can execute. This checkpoint does not advertise working delegation.

## C3.22: One supervised local child

Prerequisites: C3.21.

Run one temporary child through the existing runtime and typed ACP path, exposing its handle immediately to the parent or host.

Acceptance:

- A02 passes: spawn, inspect, wait and stop return typed state/outcomes; child model/tool activity has independent identity/context and matching native/ACP/OTel spans. Root updates include the child without copying its transcript into the root model context.
- Enforce depth, root-owned atomic admission and all authority intersections from the first child call. Parent/root cancellation joins the child and its owned work before settlement; a child cannot outlive the run.
- Preserve sibling-ready supervision semantics, per-child accounting and shared filesystem mutation serialization. This checkpoint enables only one active child; two-child execution waits for C3.23.

## C3.23: Two children and shared budgets

Prerequisites: C3.22.

Enable two concurrent depth-one children with a bounded pending queue and shared atomic limits.

Acceptance:

- A03 passes with barriers proving two child model operations overlap. Wait-any/all work; stopping or failing one child leaves the other observable and runnable; queued work is cancellation-aware.
- Race model/tool/process/MCP admission and attested token/cost reservations against the final root allowance. No work overspends or double-settles; unknown/uncertain usage keeps C2's conservative meaning. Child limits only narrow root limits.
- Bound total children, queued work and aggregate events/trace bytes; root cancellation drains the queue and joins every child. Denied provider/tool/Skill/MCP/root overrides cannot be escaped through delegation.

## C3.24: Validated child handoffs

Prerequisites: C3.23.

Pass a completed child's bounded schema-valid result or workspace artifact reference into a later child.

Acceptance:

- A04 passes for both inline structured output and a workspace-relative artifact reference; a later child consumes the selected data without receiving the earlier transcript. Prove a two-child fan-out and explicit parent fan-in.
- Reject schema-invalid, oversized, stale or unauthorized references before downstream work. Validate referenced paths/revisions through the existing filesystem contract; do not add an artifact store or claim concurrent-write isolation.
- Preserve source/result IDs, validation metadata, usage and trace links. Failed or cancelled upstream work is never silently treated as a successful handoff; no graph scheduler, persistence or automatic retry is introduced.

## C3.25: Extensibility acceptance

Prerequisites: C3.7, C3.24.

Exercise the combined extensibility surface before selecting interoperability implementation.

Acceptance:

- E01 passes from a fresh build on the development host: Rust embedding and TypeScript ACP drive a synthetic task using an MCP tool, an explicitly activated Skill resource, two children and validated output/handoff. Include forced basic compaction that retains active capability/child ownership and completed handoff state, root cancellation and denied authority variants.
- Run the full offline suite and a real Collector proof covering root/model/native tools/MCP/local children, exact identities, metadata-only export and exporter outage. Preserve both gateway and Open Responses fixture regressions.
- Reference the source-matched OpenRouter live proof and an explicit Vercel live proof; rerun bounded live checks if provider/lifecycle changes invalidate prior evidence. Record this slice's capability assessment; four-target release acceptance remains C3.35–C3.38.

## C3.26: A2A binding and Agent Cards

Prerequisites: C3.25.

Pin A2A 1.0 and one JSON-RPC-over-HTTP/SSE binding, then implement bounded validation of an explicitly configured Agent Card.

Acceptance:

- R01 passes: resolve an allowlisted endpoint/card into an untrusted `remote_a2a` proxy description with distinct local agent, remote context/task, card digest and protocol identities.
- Freeze message/task/artifact/cancel mappings and size/time/error/trace-extension bounds against an independent reference server. Unsupported required versions, bindings or extensions fail before task submission.
- Credentials, endpoint selection and delegated input remain host-controlled; card metadata cannot change local authority, redirect credentials or expose private local policy. No server listener or broad discovery is added.

## C3.27: A2A task streaming and Artifact

Prerequisites: C3.26.

Delegate one bounded task through the supervisor, stream status, and consume one bounded Artifact from the reference server.

Acceptance:

- R02 passes: standard Messages/Parts and task/status updates map to proxy events and a typed result while preserving original remote IDs. Exercise the supported immediate-message and terminal-task result shapes.
- Assemble bounded artifact updates without automatic URL fetching or treating remote files as local paths; reject unknown required content and excess bytes. Remote-reported usage is labelled separately from enforceable local accounting.
- Expose the configured remote path to host and model-facing delegation without copying local credentials, full `RunSpec` or transcripts. Basic cancellation/deadline cleanup works immediately; adverse races are C3.28's focused proof.

## C3.28: A2A cancellation and boundary proof

Prerequisites: C3.27.

Prove the selected remote lifecycle's failure, cancellation and observability guarantees through CLI/ACP.

Acceptance:

- R03 passes: cancellation before/after task assignment, accepted/rejected cancel, late completion, stream loss, timeout, oversized artifacts and remote input-required states settle truthfully with no automatic resubmission.
- Proxy cleanup is bounded without claiming the remote service stopped, refunded usage or obeyed local tool policy. Hard remote spending guarantees remain unsupported unless separately evidenced; no local authority leaks into peer content.
- A real Collector receives the remote-task span under the proxy and propagated W3C context where supported; generic A2A peers still work without the optional extension. Native IDs, content redaction and exporter-failure behavior remain verified.

## C3.29: Basic streaming TUI

Prerequisites: C3.25.

Add a small terminal composer and event-driven run view using the same runtime/session entry points.

Acceptance:

- T01 passes: compose a task and stream transcript, provider/model, active tool/Skill/child, usage, policy status and trace ID. Each submitted task remains an independent bounded run.
- Cancel through the real run handle; bound retained display data and neutralize terminal control content. Rendering has no private model loop, credentials or persistence state.
- Freeze CLI TTY/non-TTY selection and exit behavior. Preserve parseable JSON/ACP stdout and ordinary CLI usage; no approvals, durable chat or elaborate tree/graph inspector is added.

## C3.30: TUI PTY and restoration proof

Prerequisites: C3.28, C3.29.

Test the freshly built TUI in a real pseudo-terminal, including the actual terminal modes and process lifecycle.

Acceptance:

- T02 passes for input editing, resize, streaming output, slow rendering, local/remote child status and Ctrl-C during model/tool/child activity. Verify exactly one typed terminal result and joined owned cleanup.
- On success, cancellation, EOF and injected error, restore terminal modes/cursor and leave no worker/process behind. Malicious ANSI/OSC/control content cannot issue terminal actions.
- Redirected output and JSON/ACP invocations retain their documented framing. Record reproducible PTY commands and platform-specific limits for the native acceptance checkpoints.

## C3.31: Offline doctor and focused diagnostics

Prerequisites: C3.28, C3.30.

Add the release's small diagnostic surface for capabilities actually implemented.

Acceptance:

- D01 passes: `doctor` reports binary/platform, protocol/schema pins, provider/model configuration, credential presence, static policy, shell, Skill roots, MCP/A2A configuration, child limits and trace/OTel configuration without running an agent.
- Default diagnosis performs no network request and starts no configured MCP executable. Report its measured duration; network/model/MCP probes require an explicit invocation and never print secrets.
- Missing/rejected key, unsupported model, failed MCP startup and policy denial produce a concise cause/fix and stable exit code. Document configuration provenance; do not add every inspector, config generator or optional add-on diagnostic.

## C3.32: Complete deployment preset acceptance

Prerequisites: C3.31.

Prove every supported subsystem can be preconfigured through one composed deployment preset, with no hidden flag-only settings.

Acceptance:

- G04 passes for reusable base, read-only production and development presets configuring models/routes, context compaction, shell/filesystem policy, limits, output schema, MCP, Skills, children, A2A, interfaces and trace/OTel; credentials remain external references.
- Run a synthetic end-to-end task from a rendered preset through CLI, ACP and embedding, including eligible fallback and a denied command. Resolve identically in a clean environment and a noisy one under locked mode; preserve source/config fingerprints.
- Compare the option inventory with the implemented public settings, document intentionally host-only callbacks/secrets/platform facts, verify unsupported settings fail explicitly, and carry versioned presets plus migration guidance into packaging and native acceptance.

## C3.33: Compatibility and protocol fixtures

Prerequisites: C3.32.

Review and pin the complete implemented public surface before packaging it.

Acceptance:

- K01 passes with a machine-readable supported-operation/version matrix, immutable upstream/SDK/schema/fixture identities, and executable conformance cases for configuration, ACP, MCP, A2A, Open Responses, Skills and JSON Schema.
- Give each project-owned ACP extension an established project-controlled name/URI, schema/version, capability, bounds, privacy class and generic-peer fallback. Test C2 clients or a documented negotiated migration; do not silently reuse a wire version for incompatible data.
- Include unknown-field/variant, malformed-frame, cancellation, slow-consumer and bounded-input property/fuzz coverage for introduced parsers. Confirm optional adapters and unused native transports add no listener or eager startup work.

## C3.33a: TUI rendering and transcript fixes

Prerequisites: C3.33.

User-requested follow-up after the compatibility stopping point: fix flicker, make tool calls and prior task history visible, and render Markdown.

Acceptance:

- Update only changed terminal rows; idle screens produce no repeated output and streaming avoids whole-screen clears.
- Retain bounded, scrollable visible history across tasks, including persistent tool start/completion entries. Prior display history does not change independent task execution.
- Render headings, emphasis, lists, quotes, links and fenced/inline code with bounded layout and neutralized untrusted terminal controls.
- Verify real PTY history navigation, streaming, idle output, Markdown, cancellation, resize, backpressure and terminal restoration; rebuild the release binary and compare relevant TUI performance.

## C3.33b: Knowledge-work benchmark

Prerequisites: C3.33a.

User-requested follow-up, independent of release packaging. Build a reproducible local-source knowledge-work comparison for Pablo, Codex, Claude Code, Ori and Pi. The user also requested Pablo + Astra, private key reuse for Ori/Pi and Codex API auth, costs and a graphic, with strict task passes removed from headline output.

Acceptance:

- Seeded synthetic document, reconciliation, policy, decision and planning tasks have deterministic factual/citation scoring with answer keys outside agent workspaces. Document what subjective quality is not scored.
- A serial headless runner records exact commands/models/versions, task fingerprints, correctness, wall time, process-tree sampled memory, CPU time/utilization and explicit missing metrics. Failures/timeouts remain in aggregates; retained runs do not overlap builds/tests.
- Verify scorer negative controls, process monitoring and cleanup, adapter event normalization and report generation offline. Run a bounded live pilot where authentication/model access exists, recording unavailable competitors explicitly without substituting models or fabricating results.
- Document reproducible commands, benchmark limitations and existing benchmark alternatives. Preserve C3.34 and later release gates as pending.

## C3.33c: General reasoning controls and latency

Prerequisites: C3.33b.

User-approved latency follow-up. Reasoning configuration must work across supported models and providers, without GLM-specific runtime branches. Global defaults remain provider-default; faster model-profile defaults require their own evidence.

Acceptance:

- One typed reasoning option supports provider default, none/minimal/low/medium/high/xhigh/max effort or an explicit token budget. CLI, deployments, routes, compaction, repair and inherited profiles retain it; each adapter validates and translates supported syntax without silently dropping explicit settings.
- OpenRouter, Vercel Chat Completions and pinned Open Responses have focused request, unsupported-setting, privacy and compatibility tests, including non-GLM and unknown-model fixtures.
- Content-free monotonic model timing distinguishes preparation, dispatch, headers, first data/delta, terminal and completion. Reports retain calls, tools, gaps, cache/reasoning tokens, HTTP version and missing values; no network-only inference from combined provider wait.
- Evaluate HTTP/2 with HTTP/1.1 fallback using controlled fixtures and matched requests. Preserve cancellation, accounting, stream validation and connection ownership; keep only demonstrated improvements.
- Serial six-task live comparisons use three seeds and three repetitions for Pablo GLM/Astra at provider default versus low, plus matched Pi/GLM low. Preserve all attempts and historical baselines. Faster defaults require at least 20% lower p50, no worse p95, factual accuracy overall/per family, completion or cost, and reviewed report detail.
- Finish useful tests, evidence and the comparison graphic; keep release packaging held. Failed promotion gates retain existing defaults without blocking the general controls and diagnostics.

## C3.34: Release archives and installation

Prerequisites: C3.33, C3.33a, C3.33c.

Build installable release candidates for macOS arm64/x86_64 and Linux x86_64/arm64, with a small explicit-destination installation path.

Acceptance:

- B01 passes: pinned build inputs produce archives with executable, checksum, required notices and source/build/target manifest; document minimum OS/libc requirements and any signing/notarization limits.
- Test archive layout, checksum failure, installation to a temporary prefix, executable-name collision, replacement policy and removal. An installed binary runs without Node, Python, a source checkout or implicit package downloads.
- Make offline CI and artifacts reproducible without provider secrets. Cross-compilation can prepare an archive but cannot pass a native execution gate; publishing uses the exact artifacts accepted in C3.35–C3.38.

## C3.35: Native macOS arm64 acceptance

Prerequisites: C3.34.

Accept the release candidate on an actual macOS arm64 runner.

Acceptance:

- B02 passes: verify native architecture, non-root fixtures, OS/tool versions and source/archive identity; run the full offline suite, installed CLI/Rust embedding/ACP/TUI paths and real Collector integration.
- Explicitly run bounded Vercel and OpenRouter CLI/ACP proofs against the candidate. Retained evidence is valid only when source/build/profile parity is demonstrated; missing live evidence remains an open gate.
- Record cleanup, policy, compatibility and privacy results plus sanitized platform evidence. No tag or upload of raw traces/credentials is part of this checkpoint.

## C3.36: Native Linux x86_64 acceptance

Prerequisites: C3.34.

Run the same B02 acceptance on native Linux x86_64, extending the proven private CI workflow.

Acceptance:

- Verify architecture, unprivileged execution, OS/libc, tools and source/archive identity; run the full offline suite, installed-binary interfaces, real PTY and real Collector cases.
- Complete the explicitly invoked bounded live gateway matrix through an authorized credential path; never substitute macOS results for Linux execution. Keep ordinary PR CI offline.
- Record the actual hosted VM/runner environment and cleanup/privacy evidence. Missing runner or live execution leaves a concrete blocker and unblock condition.

## C3.37: Native macOS x86_64 acceptance

Prerequisites: C3.34.

Run B02 separately on an actual macOS x86_64 runner, including the installed archive, full offline suite, CLI/ACP/TUI, Collector and bounded live gateway matrix.

Acceptance:

- Record native CPU execution, unprivileged fixtures, minimum-OS compatibility, tools and exact source/archive fingerprints; Rosetta or arm64 results cannot substitute for this gate.
- Retain the same lifecycle, policy, cancellation, redaction and supported-protocol observations as C3.35. Document any platform-specific implementation failure in the owning evidence.
- If native access is unavailable, record the runner and unblock condition explicitly. Do not publish an unexecuted archive as a supported target or silently remove this target from the plan.

## C3.38: Native Linux arm64 acceptance

Prerequisites: C3.34.

Run B02 separately on an actual Linux arm64 runner, including the installed archive, full offline suite, CLI/ACP/TUI, Collector and bounded live gateway matrix.

Acceptance:

- Record native CPU execution, unprivileged fixtures, OS/libc/tools and exact source/archive fingerprints. A native arm64 VM is acceptable when disclosed; cross-architecture emulation cannot substitute.
- Retain the same lifecycle, policy, cancellation, redaction and supported-protocol observations as C3.36, including filesystem/process edge cases.
- Record unavailable runner/credentials as concrete blockers with unblock conditions. No skipped gate or historical C1 arm64 binary qualifies as current candidate acceptance.

## C3.39: Matched release measurements

Prerequisites: C3.35, C3.36, C3.37, C3.38.

Publish sanitized candidate baselines using C1.7/C2 methods, extending them only for new supported paths.

Acceptance:

- B03 records stripped/archive size, cold start, idle/warm RSS, first text, event latency, native trace overhead, filesystem workloads, fresh/warm stdio ACP, typed in-memory ACP and cold/warm A2A paths with timing boundaries and fingerprints.
- Use 30 measured samples after five warmups where applicable and retain the 30,000-delta event method. Compare matched C2/C3 builds on the same primary hosts; label additional-target baselines and separate setup, protocol, tool and remote costs.
- Record dependency/feature size contributions and the existing Linux 12.150 MiB observation. Publish measured limits without ratifying provisional ceilings; any optimization or threshold change requires its own recorded evidence and affected acceptance rerun.

## C3.40: Prerelease distribution and cycle handoff

Prerequisites: C3.39.

Prepare reviewable release notes, compatibility fixtures/manifests, install commands and verified archives, then distribute the explicitly selected prerelease through the selected repository/channel.

Acceptance:

- B04 ties every asset, checksum, protocol fixture, measurement and release note to the accepted source/build. Review the concrete version, destination and assets before the separately selected tag/publication action, following C1/C2's publishing practice.
- After authorized publication, download and checksum-verify the distributed archives and smoke the installed binary on each advertised native target. Record the release URL, immutable source/tag and installation/rollback instructions. Preparation alone does not pass publication acceptance.
- Record C3's capability assessment and exact remaining release gates. Otto is excluded and unexecuted, so do not claim the complete beta.1/0.1 contract; unevidenced live hard-spending profiles also remain unsupported. Retain all historical records and select future work only through a later explicit cycle/action.

## Verification and completion rules

For runtime checkpoints, run formatting, locked Rust workspace tests and clippy, relevant executable integration tests, TypeScript typecheck and affected ACP/telemetry tests, plus project consistency and diff checks. Contract-only checkpoints run existing helper tests and schema/example/link audits without paid calls or tests for prose. Freeze fixture input, expected outcome, limits, version mapping and owning command before implementation. Add meaningful tests for behavior and boundaries, not copies of the implementation.

Policy, admission, bounded resources, cancellation and redaction ship alongside each feature's first usable path. Each feature also adds its typed options, default/merge rules, secret references, explain output and deployment fixture to the shared configuration system. Later proof checkpoints deepen adversarial or cross-surface coverage; they do not permit earlier unsafe implementations. No default unlimited call-count policy is replaced by an incidental cap. Unsupported hard token/cost profiles reject before delivery as in C2; live reported usage cannot prove enforceable remote spending limits.

Only completed checks with checked-in, sanitized evidence can satisfy a checkpoint. Keep source/build/harness identities, actual platform/tool versions, fixture counts and incomplete gates explicit. Platform checkpoints may reuse unchanged evidence with demonstrated parity; code or dependency changes invalidate affected artifacts and require targeted re-verification before release. No plan, mock, skipped test or helper consistency check stands in for actual live/native/Collector/PTY/publication proof.

## Deferred boundary

Otto integration is excluded at the user's request and remains a full-release gate in the unchanged brief. Durable root sessions, persistent/recursive children, generic graph scheduling, external ACP children, A2A serving, broad MCP beyond the selected tool subset, automatic Skill matching/installation, adaptive model routing, advanced cache tuning, AG-UI/Vercel AI SDK/OpenAPI add-ons, Python/Graphline, Windows and interactive shell PTYs remain in the [backlog](../backlog.md). Ordered configured fallback is now selected; adaptive routing and general retry orchestration remain deferred. The TUI's PTY test harness does not introduce interactive shell tools. Nix evaluation, package/OS provisioning and hot-reloading active runs are outside the selected deployment-config design.
