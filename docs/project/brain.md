# Project brain

Pablo is a small headless Rust agent runtime for applications doing arbitrary work. Hosts own their sandboxes, business state, approvals, and user experience. CLI and protocol clients share one runtime lifecycle.

This file holds durable context. [State](state.json) reports current progress, [the log](log.jsonl) records history, and [cycle C3](cycles/003-extensibility-and-release.md) defines the selected work. Completed [C1](cycles/001-first-spike.md) and [C2](cycles/002-single-agent-completion.md) plans/evidence remain historical. Run `node scripts/project.mjs context` for a focused handoff. Private source remote: [calebjohn24/pablo](https://github.com/calebjohn24/pablo), with `main` tracking `origin/main`. Repository creation is recorded in LOG-0044; [native Linux acceptance](evidence/c2.5-linux-x64.md) closes C2.5. C3 selects declarative deployments, extensibility, interoperability and release delivery, excluding Otto integration.

Read [the architecture brief](../context.md) selectively: section 29.1 defines the 0.1 contract; section 37.1 the C1 spike; sections 12–14 contracts/lifecycle/cancellation/shell; sections 19/21 providers and OTel; sections 28/31/32 performance/release/tests. [Backlog](backlog.md) retains deferred slices and promotion conditions.

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

Reuse installed Rust and Node tools; noninteractive shells may need nvm initialization. `.nvmrc` pins Node 24.20.0, but verify and record each host's actual version rather than assuming it matches. Use Node's built-in modules and test runner for project management; pin dependencies and protocols when introduced.

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

### D028 — Make native Linux acceptance reproducible in private CI

Use the private repository's Ubuntu x64 workflow to verify the native architecture and unprivileged fixtures, full suites, real Collector and matched C1.7/C2 measurements. Record the hosted VM environment rather than implying bare-metal timing. Upload only measurement reports, keep provider credentials local and preserve provisional sizing/performance observations without inventing ceilings. This resolves the runner blocker and makes acceptance repeatable; see [Linux evidence](evidence/c2.5-linux-x64.md).

### D029 — Adopt C3 through small independently verified checkpoints

The user selected all proposed extensibility, interoperability and release-hardening work except Otto, preferring smaller checkpoints. [C3](cycles/003-extensibility-and-release.md) separates each gateway/transport, schema validation/repair, child lifecycle/concurrency/handoff, TUI/PTY proof and native target. The [contract map](contracts/c3-extensibility-and-release.md) assigns detailed decisions immediately before their owning slice, while the [fixture map](fixtures/c3-extensibility-and-release.md) fixes observable acceptance. Keep one checkpoint per implementation session, all C1/C2 history and the version-1 state model; only the planning checkpoint completes on adoption.

### D030 — Separate the selected prerelease from the full release contract

C3 excludes Otto by user instruction without rewriting sections 29.1/31.1. Its synthetic composition proof cannot satisfy Otto or justify complete beta.1/0.1 claims. Plan four native macOS/Linux architecture gates from section 27.1, with unavailable runners explicit and no emulation substitute; prepare exact artifacts before the separately selected publication action. Preserve unsupported live spending guarantees and measured performance observations. This keeps distribution claims tied to actual evidence rather than to the breadth of the plan.

### D031 — Make deployments declarative and inspectable

The user requested NixOS-style preconfiguration of every part of Pablo. Use versioned typed TOML, bounded local modules/imports, named profiles, deterministic merge, external secret references, validate/explain/render and locked deployment inputs. Every feature supplies options through one shared resolver; deployment authority ceilings survive convenience overrides. The [deployment design](contracts/c3-deployment-config.md) keeps presets reproducible and inspectable without requiring a Nix evaluator or adding OS provisioning.

### D032 — Select ordered fallback and explicit shell command rules

The user's config request promotes these formerly deferred behaviors into C3. Model routes preserve configured order, have one retry owner, count every attempt, retain uncertain charges and never replay completed tools or restart the task. Child routes only narrow authority. Shell rules distinguish the launcher from bounded literal executable/argument matching, reject unsupported syntax when restricted and retain host-owned containment. See the [design](contracts/c3-deployment-config.md); adaptive routing and general retry orchestration remain deferred.

### D033 — Freeze a closed deployment contract before the loader

C3.1 uses an `options` envelope, TOML 1.0.0, exact typed defaults, ordered import/profile layers and explicit list operations. Named credential/profile conflicts reject; authority layers accumulate independently of ordinary values. Locked inputs exclude ambient files/SDK environment and permit only named narrowing overrides. The [contract](contracts/c3-deployment-config.md), [inventory](contracts/c3-deployment-options.md) and G01 corpus make C3.2/C3.3 reviewable without pretending future providers, routes or extensions work. Legacy no-config behavior remains an explicit compatibility path; no loader or runtime change ships at C3.1.

### D034 — Separate effective identity from source and host identity

Hash canonical defaulted config with secret references and authority, preserving list order and exact counters. Keep source/provenance identity separate so comments and rendering do not change effective identity; physical root bindings remain host-only and secret bytes are never hashed. This supports portable presets without claiming identical workspaces, secret-rotation identity or live reproducibility. The [resolved contract](contracts/c3-deployment-config.md#resolved-identity-provenance-and-inspection) and canonical vectors freeze the encoding and hand off real cross-interface proof to C3.3.

### D035 — Make offline resolution preserve authority and source identity

C3.2 exposes one pure configuration resolver with bounded no-follow local reads and explicit host inputs; activation, credentials and CLI/ACP adapters stay in C3.3. Workspace-relative ceilings become fixed portable references when declared so later workspace overrides cannot move them. Equal named profiles expand once while retaining each origin. Precisely define scalar/list/unset provenance and typed-bootstrap source digests; the C3.1 production config and both fingerprints stay unchanged. See the [deployment contract](contracts/c3-deployment-config.md) and [C3.2 evidence](evidence/c3.2.md).

### D036 — Continue C3 with verified merges and ongoing measurements

The user now authorizes finishing the remaining C3 cycle, measuring performance throughout and merging completed PRs as work proceeds. This overrides the per-session stopping cadence while retaining one active checkpoint, dependency order and honest gate evidence. Preserve the full cycle scope, including live and native acceptance; missing external prerequisites remain incomplete rather than replaced by mocks. Use matched offline measurements at runtime changes and retain source/build identities.

### D037 — Reuse pinned inputs and independent policies at admission

C3.3 exposes load-then-resolve so hosts capture only declared non-secret environment values without rereading files. Per-task overrides retain provenance and immutable ceilings; policy allowlists intersect as separate layers. Offline rendering stays within the entry-file bound so successful output can reload. [Adapter details](contracts/c3-deployment-config.md#c33-adapter-details) define bootstrap spelling and distinguish non-secret preparation from complete runtime admission.

### D038 — Filesystem work quotas are opt-in

The user reported ordinary runs failing with `FilesystemWork` and requested removal of that limit. Default file bytes, visited entries, depth and scan bytes are now unlimited in Rust, CLI, ACP and deployment presets; explicit host quotas remain enforceable. Preserve bounded responses, cancellation, deadlines, policy and mutation preconditions. Iterative traversal avoids replacing the removed depth quota with stack recursion. This deliberately supersedes the numerical C2 defaults and changes the C3 golden identities; the product context is unchanged.

### D039 — Explicit OTel construction needs pinned upstream patches

Pinned SDK builders read ambient detectors before setters, and OTLP merges ambient headers after explicit headers. Narrow vendored constructors avoid those reads without process-global environment mutation, while preserving legacy defaults. Provenance hashes, exact versions, licenses and hostile-environment/rotation tests make the patch reviewable; remove it when pinned upstream supports equivalent explicit construction. See [vendor notes](../../vendor/README.md).

## Working constraints

- Keep `docs/context.md` as the detailed design source; review deliberate changes using the state file's stored SHA-256.
- Secrets and generated runtime traces stay out of project memory and Git. The root `.env` is ignored; shell subprocesses must not inherit provider/exporter credentials.
- Static execution policy is not containment. Hosts provide isolation; unsupported release capabilities stay visible in the backlog. A green checkpoint does not imply a complete 0.1 runtime.
- Ordinary tests use offline fixtures. Live provider and real Collector gates require actual evidence before cycle completion.
- Prefer measured baselines to speculative performance gates.

## How to resume

Read [working instructions](../../AGENTS.md), run `node scripts/project.mjs context`, and inspect the working tree. The selected checkpoint and next action come from state. Read only the corresponding cycle section and relevant design sections before making changes.

### D040 — Match literal executable identity and argv before shell launch

C3.4 keeps the fixed launcher but passes checked canonical executable/arguments through a constant positional-argument wrapper. Optional command, canonical cwd and environment-name rules intersect with every authority layer; missing command rules retain legacy shell interpretation. This prevents quoting/operator/alias bypass without claiming an OS sandbox or script digest trust. Schema v1 advances to contract revision c3.4; old presets retain behavior while rendered identities change. See the [contract](contracts/c3-shell-commands.md).

### D041 — Provider-scoped GLM defaults and one transport implementation

The user selected `zai/glm-5.3-flash` for Vercel and `z-ai/glm-5.3-flash` for OpenRouter, including provider testing. C3.5 deliberately replaces the prior Gemini default and advances the configuration revision to c3.5. Provider-specific defaults and fixed credential destinations share one resolver; HTTP/SSE mechanics are reused while adapter mappings remain separate. OpenRouter inspection is available before its C3.6 adapter; unavailable execution rejects before credentials/effects. See the [selection contract](contracts/c3-provider-selection.md).

### D042 — Normalize OpenRouter accounting once without floating-point money

C3.6 enables OpenRouter through the shared gateway and treats its repeated terminal usage choice as accounting, not a second finish. Read cache counters and the account charge only when reported; parse raw decimal cost into micro-USD with a documented upward adjustment below one micro-USD per call. Never infer prices, sum upstream costs or attest hard ceilings. Exact key/destination scope and cancellation remain shared. See the [adapter contract](contracts/c3-openrouter.md). [C3.7](evidence/c3.7.md) proves live CLI/ACP file reads with the selected GLM model; explicit file-only credential selection prevents stale environment keys from shadowing a replacement.

### D043 — Pin Open Responses and retain task-scoped continuation

C3.8 freezes release 2026-04-24 at upstream 92c12d96d7b61d6d15e2214daa5e9c6000ab6e1c. Named HTTP/SSE events and complete ordered items are required; private opaque reasoning, summaries and assistant phase survive subsequent tool turns within their original endpoint/model scope. Unrepresentable raw reasoning rejects instead of disappearing. Configured capabilities are operator declarations, not remote attestations. C3.9 implements this with a non-serializable carrier, named-event validation, exact accounting and explicit auth-header scope. See [OR01](contracts/c3-open-responses.md).

### D044 — Resolve exact ordered routes before allowing fallback

C3.10 names profiles and composes ordered routes offline, validating every entry's capabilities, credential destination and authority. Children can retain only exact inherited entries in order. Sticky, forward-only runtime selection and one retry owner prevent hidden repeated attempts; all attempts consume root budgets and incompatible private continuation stops fallback. C3.11 executes fallback and distinct attempt deadlines through runtime-owned selection, with all-entry credential/accounting preflight and independent profile/root token clamping. ACP resource reuse never retains a task cursor; incompatible history stops before dispatch. C3.12 streams bounded attempt records with negotiated ACP notifications and preserves received-delivery evidence, preventing false refunds or uncertainty-policy bypass. See [F01/F02/F03](contracts/c3-model-routes.md).

### D045 — Basic compaction belongs in C3

The user explicitly added compaction to C3. C3.12a closes the omitted section 23/28.8 bounded-run requirement with one summary pass and one overflow recovery, preserving prefix, task constraints, recent complete tool pairs, private continuation and shared budgets. Codex informs the summary-and-history replacement pattern; GLM providers remain default, and durable memory/tokenizers are deferred. C3.12a implements conservative profile-local estimates, atomic smaller replacement and negotiated metadata with content-controlled summaries; explicit machine-code overflow is the only recovery trigger. The user-selected C3.12b refinement summarizes recent bulk too, preserving task-relevant facts rather than archiving raw outputs; retain raw turns only by configuration or source-capacity necessity. See [contract](contracts/c3-compaction.md) and [scope](evidence/c3-compaction-scope.md).

### D046 — Validate final JSON locally before exposing structured success
C3.13 pins an optional bounded Draft 2020-12 subset and a 16-entry canonical schema cache. Provider hints cannot establish validity; schema-enabled CLI/negotiated ACP tasks carry explicit provisional/valid/invalid metadata. Unsupported schemas reject before dispatch; invalid finals fail without hidden calls. Format stays annotation-only, diagnostics exclude values, and compilation/validation share finite input/work bounds. C3.14 owns the separate bounded repair. See [J01](contracts/c3-output-validation.md).

### D047 — Repair is one admitted continuation, not a retry owner
C3.14 permits opt-in repair of one invalid final answer in its original history. Keep instruction/tool/private continuation intact; share candidate output bytes, validation work and root model/token/cost/deadline/context/event/trace limits. No tools, compaction, fallback or third repair request may hide inside it. Negotiated repair metadata carries counts/phases only; terminal validity determines structured success. See [J02](contracts/c3-output-repair.md).
