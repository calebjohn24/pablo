# Project brain

## Purpose

Pablo is a small headless Rust agent runtime for applications doing arbitrary work. Hosts own their sandboxes, business state, approvals, and user experience. CLI and protocol clients share one runtime lifecycle.

This file holds durable context. [State](state.json) reports current progress, [the log](log.jsonl) records history, and [cycle C2](cycles/002-single-agent-completion.md) defines the selected work. [Cycle C1](cycles/001-first-spike.md) and its evidence remain historical. Run `node scripts/project.mjs context` for a focused handoff.

Private source remote: [calebjohn24/pablo](https://github.com/calebjohn24/pablo), with `main` tracking `origin/main`. Repository creation is recorded in LOG-0044; native Linux acceptance remains a separate gate.

## Read the design selectively

- [Architecture brief](../context.md), section 29.1: authoritative 0.1 release contract.
- Section 37.1: focused first spike; the boundary for cycle C1.
- Sections 12–14: typed contracts, run lifecycle, cancellation, shell behavior.
- Sections 19 and 21: providers and native OTel instrumentation.
- Sections 28, 31, and 32: performance baselines, release acceptance, tests.
- [Backlog](backlog.md): preserved later slices and when to consider them.

## Accepted decisions

### D001 — One checkpoint per implementation session

The user selected this cadence to keep changes manageable. Finish verification and project records, leave the next checkpoint ready, and stop. Partial checkpoints can resume across sessions; explicit user steering can change the cadence.

### D002 — Cycle C1 proves the focused spike

Use section 37.1, not the whole 0.1 release or alpha.1, as the completion boundary. Prove one model/shell loop through ACP plus native/OTel trace correlation. Keep filesystem tools, other providers, extensibility, TUI, and durable runtime state in later slices.

### D003 — Repository files are the development memory

Use curated Markdown, one structured current-state file, and append-only JSONL work history. A dependency-free Node helper reads and validates those records. This keeps context inspectable in Git and avoids introducing a database or separate service before a runtime exists.

### D004 — Vercel is the first live provider

The user selected Vercel AI Gateway. Use `AI_GATEWAY_API_KEY` and an explicit profile, initially `openai/gpt-4.1-mini`, following the approved cycle plan; D014 records the replacement default. The user supplied the credential in the root `.env`; its contents are private and uninspected by the project helper. The end-user preview in D011 brings a narrow live CLI smoke forward; live ACP acceptance remains C1.4.

### D005 — Start with two Rust crates

Create `pablo-core` and the `pablo` executable at C1.1. Keep providers, tools, protocol adapters, and telemetry in owned modules initially. The brief's larger crate map describes ownership, not a scaffolding requirement.

### D006 — One lifecycle with native telemetry

Root execution, model calls, tools, ACP updates, JSONL records, and OTel spans originate from one runtime lifecycle. Instrument the first operation; add the network exporter later. ACP is the process protocol. No alternate proprietary loop or process lifecycle is introduced.

### D007 — Reuse installed development tools

Rust is installed. Node 24.20.0 and npm 11.19.0 are installed through nvm; noninteractive shells may need nvm initialization. Python/uv are also available. Use Node's built-in modules and test runner for project management; pin dependencies and protocols when introduced.

### D008 — Inject the tracer and stream into a host-owned sink

The core accepts an OTel tracer and inline event sink, with no global installation or detached worker. The CLI owns its SDK. This gives the ACP adapter one existing lifecycle to drive. Sink calls must return promptly; asynchronous transport backpressure belongs to C1.3. Checkpoint-versioned contracts expose only implemented behavior. The `c1.2` contracts add sequential tool turns and explicit cancellation through this same lifecycle. See [runtime contracts](../runtime.md).

### D009 — Pin the native telemetry mapping from the first run

Rust 1.98.1, OTel API 0.32.0, SDK 0.32.1, and all introduced dependencies are pinned. The GenAI mapping retains the brief's immutable revision `fee465db333bdd6a7d2faa320edab5cf3101a4f4` in the separate GenAI conventions repository. Native events reuse SDK identities and exact lifecycle timestamps; JSONL content capture is independent of metadata-only OTel spans. See [the mapping](../runtime.md#otel-mapping).

### D010 — Shell is an explicit capability with owned cleanup

`ToolRegistry::with_shell()` enables the single built-in tool; empty catalogs grant none. Shell uses a contained canonical cwd, cleared environment plus `PABLO_TASK_` additions, bounded results, and a new process group. Cancellation awaits group kill, leader reaping, pipe draining, and group disappearance. The two-second cleanup allowance follows the execution deadline. No global subreaper is installed; hosts provide containment and orphan reaping. Details and the observed macOS zombie-group case are in [the shell contract](../shell.md).

### D011 — Bring a small end-user preview forward

The user asked to test real tasks immediately after C1.2. Add C1.2a before ACP: a one-task `pablo run` CLI and reusable Vercel adapter through the existing runtime, with a small live synthetic-evidence smoke. The user explicitly selected direct HTTP; use a general Rust HTTP client, with no Vercel SDK. The existing `.env` key name `VERCEL_AI_GATEWAY` is supported as an alias for `AI_GATEWAY_API_KEY`. This changes the checkpoint order without claiming C1.3 or C1.4 complete. Keep ACP, durable chat, and the remaining live acceptance separate.

### D012 — Pin stable ACP v1 and preserve native outcome truth

C1.3 uses the official Rust and TypeScript SDKs with exact releases/schema fingerprints in [the ACP lock](../acp-lock.json). C1.3 initially admitted one session and one prompt per process (D019 extends process reuse), negotiates `pablo/v1` metadata, and advertises only implemented capabilities. Standard stop reasons control the ACP turn; metadata preserves the immutable native outcome if a late cancellation arrives during final output draining. This meets cancellation semantics without rewriting trace history. See [the ACP contract](../acp.md); release-stable extension naming and broader session support remain deferred.

### D013 — Bound ACP traffic around the official SDK

The selected SDK has unbounded internal channels. Enforce connection/frame limits before ingress and acknowledge physical stdout writes before enqueueing another update. A bounded eight-event queue bridges the existing synchronous sink to one joined runtime thread, allowing the protocol to process cancellation while the worker waits for capacity. Stall/disconnect handling wakes the producer and awaits owned cleanup. This retains one core lifecycle and the embedding contract; [transport limits](../acp.md#transport-bounds-and-ownership) explain the exact bounds.

### D014 — Use the user-selected Gemini Flash default

The user selected `google/gemini-3.8-flash` for both CLI and ACP. The exact identifier was verified in the [official Vercel catalog](https://vercel.com/ai-gateway/models/gemini-3.8-flash). Preserve `--model` overrides and the existing direct HTTP adapter. Prior GPT-4.1 mini evidence remains historical; [C1.4 evidence](evidence/c1.4.md) verifies the live Gemini ACP model/shell/model path and reported usage. Repeat it explicitly with `npm run smoke:live:acp`; the fixture privately selects the root credential file and uses small opt-in caps to keep acceptance bounded.

### D015 — Call-count budgets are opt-in

The user requested no default tool budget. Default both model and tool call counts to unlimited so the former four-model-call ceiling cannot silently replace the removed two-tool ceiling. Hosts use optional `RunLimits` counts; CLI/ACP expose `--max-tool-calls` and `--max-model-calls`. Zero disables those calls. Explicit caps remain hard limits, with a provider hint requesting a final answer when no further tool result can be consumed. Timeouts, memory/transport bounds, and cancellation remain separate. See [runtime limits](../runtime.md).

### D016 — Use generous execution and transport capacities

The user asked for generous timeouts and sizes before committing C1.3. Defaults are one hour per run, 15 minutes per shell, 1 MiB input/arguments, 8 MiB tool results, 4 MiB model output, 32 MiB context, 65,536 requested output tokens, one million events, and 256 MiB native traces. Increase gateway and ACP capacities together so protocol framing and escaping do not impose the old smaller caps. Explicit run/tool timeout overrides accept up to 24 hours; cancellation and the separate short cleanup allowances remain. See [runtime limits](../runtime.md) and [ACP transport bounds](../acp.md#transport-bounds-and-ownership).

### D017 — Keep Collector export owned, bounded and separate from task content

C1.5 shares one standalone SDK setup across CLI/ACP, using the pinned OTLP 0.32.0 exporter and bounded SDK batch processor. A dedicated current-thread executor lets shutdown cancel HTTP/retry work before the two-second deadline and join the batch thread without blocking the model loop. Adapt the pinned SDK's service-name precedence and overly broad retry classification; sanitize diagnostics and count losses, including partial rejection. Incoming context uses the existing negotiated `pablo/v1` ACP namespace (stable ACP has no dedicated fields), CLI flags, or explicit host OTel context. Baggage has an empty allowlist. The real Collector 0.160.0 proof checks native/span IDs and timestamps without a paid provider. See [configuration and ownership](../telemetry.md) and [evidence](evidence/c1.5.md).

### D018 — C1 acceptance establishes scoped baselines, not release ceilings

C1.6 verifies the focused fixtures on native macOS arm64 and an isolated Ubuntu 24.04 arm64 VM, plus a fresh explicit live Vercel ACP run on macOS and real Collector proof on both systems. Release measurements use 30 samples after five warm-ups; event latency uses 30,000 deltas. Report timing boundaries, virtualization and source fingerprints so setup costs are not mistaken for pure protocol overhead. C1 is complete, while native Linux x86_64 measurements and broader alpha.1/0.1 gates remain unclaimed. No provisional performance number becomes a CI failure threshold. See [acceptance](evidence/c1.6.md), [methods](../measurements.md), and [the original C2 proposal](proposals/002-single-agent-completion.md), adopted by D021.

### D019 — Optimize Pablo without changing model behavior

The user requested a C1.7 performance follow-up and explicitly excluded provider routing and model/prompt optimization. Serialize borrowed redacted trace views into one bounded reusable buffer, count ACP sizes without temporary JSON allocations, and forward the first text of each model operation immediately. Reuse one worker, HTTP client/pool, compiled tool catalog and SDK across successive independent ACP sessions; retain one prompt per session, process-lifetime ingress bounds, per-task cancellation/context and exclusive per-session trace files. This lowers local latency while preserving the existing lifecycle and isolation. See [ACP](../acp.md) and [performance evidence](evidence/c1.7.md).

### D020 — Prefer the measured speed/size balance of thin LTO

Use thin LTO, one codegen unit and stripped symbols for release builds. Full LTO makes the binary smaller but repeated alternating comparisons show slower trace encoding; the selected profile already fits the provisional 10 MiB headless target on measured macOS and Linux arm64 systems. Preserve unwinding and portable target defaults. The [C1.7 report](evidence/c1.7.md) retains all candidate measurements, workload boundaries and remaining performance limits.

### D021 — Adopt the bounded single-agent completion cycle

C2 follows the verified C1.7 foundation and selects filesystem tools, one-task JSON output, exact static policy and conservative accounting before extensibility. [The cycle](cycles/002-single-agent-completion.md), [contracts](contracts/c2-single-agent.md) and [fixtures](fixtures/c2-single-agent.md) freeze behavior without advertising it as implemented. Preserve C1 checkpoint records and log history in the existing version-1 state model; the selected cycle changes, while helper progress remains cumulative. This keeps historical verification valid without adding another status authority. General model structured-output repair remains alpha.2.

### D022 — Make filesystem behavior explicit and bounded

Use no-follow workspace-relative operations, bounded UTF-8 snapshots, deterministic listing/literal search and explicit read/write capabilities. Mutations require revision preconditions and atomic replacement; hosts own isolation from independent writers. This prevents accidental traversal and silent truncation while avoiding a false filesystem compare-and-swap or sandbox promise. [The filesystem contract](contracts/c2-single-agent.md#shared-filesystem-contract) defines limits, errors, cancellation and trace privacy; implementation starts at C2.1.

### D023 — Separate actual usage from conservative budget charges

Return accounting on every admitted outcome and keep missing provider usage/cost unknown. Optional aggregate ceilings require an attested per-call upper bound reserved before delivery; unsupported profiles fail before sending. Retain the reservation after uncertain delivery instead of inventing zero cost. This makes budget claims reviewable while preserving D015's unlimited default counts. [The accounting contract](contracts/c2-single-agent.md#accounting-admission-and-settlement) defines settlement and explicitly defers unevidenced live ceiling profiles.

### D024 — Reserve enough filesystem result capacity for truthful errors

C2.1 requires requested filesystem results to have at least 1,024 bytes, matching the existing host minimum. Smaller requests cannot hold bounded failure metadata and are rejected before I/O; the 8 MiB default is unchanged. Workers retain bounded data, may read one extra byte to detect growth, and are joined before settlement. [Read evidence](evidence/c2.1.md) records actual path-race, cancellation, privacy and transport proof and the macOS invalid-filename limitation.

### D025 — Preserve committed mutation truth through cancellation

C2.2 admits the complete mutation result before touching a target and installs complete bytes atomically with revision rechecks. Before commit, failures preserve the target; after commit, tool metadata retains `committed:true` even if cancellation or cleanup failure follows. This prevents a terminal cancellation from implying rollback. Hosts own isolation from external writers and crash durability. See [mutation evidence](evidence/c2.2.md).

### D026 — Exact task accounting and negotiated envelope

Project the native terminal event into CLI JSON and opt-in ACP task metadata; use decimal-string u64 values so JavaScript cannot round them. Negotiated task metadata replaces the legacy outcome field to keep large escaped output within the existing frame bound; old peers retain their shape. See [C2.3 evidence](evidence/c2.3.md).

### D027 — Keep policy identity complete without breaking native built-ins

C2.5 records one deciding ID for every applicable allowed tool/launcher/root dimension, including defaults and recoverable filesystem errors. The decisive denial ID appears in native metadata and OTel. Mandatory built-in denials retain C1 enum wire values, mapped explicitly to logical `builtin.*` identities; changing them would break existing clients without improving enforcement. See the [policy contract](contracts/c2-single-agent.md#static-policy).

## Working constraints

- Keep `docs/context.md` as the detailed design source; review deliberate changes using the state file's stored SHA-256.
- Secrets and generated runtime traces stay out of project memory and Git. The root `.env` is ignored; shell subprocesses must not inherit provider/exporter credentials.
- Static execution policy is not containment. Hosts provide isolation.
- Ordinary tests use offline fixtures. Live provider and real Collector gates require actual evidence before cycle completion.
- Unsupported release capabilities stay visible in the backlog. A green checkpoint does not imply a complete 0.1 runtime.
- Prefer measured baselines to speculative performance gates.

## How to resume

Read [working instructions](../../AGENTS.md), run `node scripts/project.mjs context`, and inspect the working tree. The selected checkpoint and next action come from state. Read only the corresponding cycle section and relevant design sections before making changes.
