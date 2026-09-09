# Cycle C2: complete the single-agent foundation

## Objective and authority

Adopt the [C2 proposal](../proposals/002-single-agent-completion.md) after C1.7. Complete the single-agent filesystem, machine-result, static-policy and accounting surface through the existing Rust lifecycle, CLI and ACP. The [design brief](../../context.md), sections 29.1 and 30, remains the release authority. [State](../state.json) alone records progress; [the log](../log.jsonl) retains all C1 history.

The [C2 contracts](../contracts/c2-single-agent.md) select behavior and document the gap review. The [acceptance fixtures](../fixtures/c2-single-agent.md) freeze synthetic inputs and expected observations. They specify future behavior, not implemented APIs or passing tests. Change a frozen expectation deliberately in the owning checkpoint's evidence and decision record; never silently weaken it to match implementation.

Retain C1's sequential loop, explicit registry, unlimited default model/tool call counts, generous existing byte/time limits, cancellation cleanup, trace redaction, immediate first text, reusable ACP worker/client/catalog/SDK, independent session traces and C1.7 measurement methods. No provider routing, model/prompt optimization, automatic retry/fallback, interactive approval, durable state or performance CI ceiling enters C2.

## C2.0: Adopt contracts and scope

Review the implemented core, shell, CLI and ACP against sections 12, 14, 20, 29.1 and 30. Select exact contracts and acceptance examples before filesystem implementation.

Acceptance:

- Record implemented capabilities, selected C2 gaps and explicit release/extensibility deferrals, including the distinction between a typed task envelope and model JSON Schema output.
- Specify tool arguments/results, bounds, path/symlink behavior, mutation conflicts/atomicity, error disposition, policy precedence/rule identity and accounting admission/settlement.
- Freeze fixtures for each later checkpoint with concrete inputs, outcomes and required lifecycle/transport observations; future execution gates remain unrun.
- Initialize C2.0 as active before drafting, retain every C1 checkpoint/verification and the log prefix, preserve the reviewed design hash, then select C2.1 on completion. Keep state format version 1; checkpoint records span cycles, while `cycle` identifies the selected cycle. The helper's progress count is cumulative across retained checkpoints.
- Verify project consistency, existing helper tests, local document links, historical preservation and the final diff. C2.0 introduces no runtime code, dependencies, live calls or runtime acceptance claim.

## C2.1: Bounded filesystem reads

Implement `fs.read`, `fs.list` and `fs.search` from the frozen contract in the existing registry and lifecycle. Add the filesystem policy subset required to enforce read roots now; C2.4 must not be the first enforcement point. Keep schema/catalog compilation reusable and support the tools in provider name mapping, CLI and ACP.

Acceptance:

- Execute F01–F08 and L01–L03 from the fixture catalog using real temporary files and the offline provider. Prove actual results reach the next model call, with matching call IDs and a stable catalog.
- Verify schema rejection, bounded memory/work/results, UTF-8 boundaries, pagination, deterministic ordering on unchanged inputs, special files, traversal/symlink denial and cancellation with all owned work joined.
- Verify explicit registry opt-in, independent read capability, content redaction and native/ACP/OTel correlation, including policy and failure paths. Retain existing shell behavior.
- Run Rust workspace tests, formatting and clippy, TypeScript typecheck and ACP/telemetry tests, plus project checks. Record any platform gate not yet exercised for C2.5.

## C2.2: Safe file mutations

Implement `fs.write` and `fs.edit`, requiring explicit write authority and optimistic revision preconditions. Use the same dispatch, limits and telemetry as reads.

Acceptance:

- Execute M01–M06 and the shared L fixtures. Prove create-only behavior, stale-revision refusal, exact single replacement, unchanged unrelated bytes, mode preservation, bounded temporary files and commit/cancellation semantics.
- Verify no symlink traversal, no implicit parent creation, no modification on precommit failure, and no leftover temporary file or worker after settlement.
- Document atomic visibility separately from crash durability and external-writer isolation. No test may claim a portable filesystem compare-and-swap guarantee.
- Run Rust workspace tests, formatting/clippy, TypeScript typecheck, ACP/telemetry tests and project checks.

## C2.3: Single-task machine output

Implement `pablo "TASK"` as the existing `run` command's shorthand and `pablo run --json`. Project one shared task result envelope from the existing runtime outcome and events, including usage/accounting on non-success paths. Model output remains a string; general structured-output validation/repair belongs to extensibility.

Acceptance:

- Execute J01–J04 and L03. Verify one bounded JSON object plus newline, clean stdout, explicit pre-admission errors, exit codes, cancellation and sink failure behavior.
- Verify CLI/core/negotiated ACP outcome and accounting parity without reinterpreting native terminal truth; retain generic ACP fallback and independent-session isolation.
- Run CLI/Rust tests, formatting/clippy, TypeScript typecheck, ACP tests and project checks. No fresh paid-provider gate is needed for framing fixtures.

## C2.4: Policy and accounting

Complete the selected policy configuration and run-local accounting ledger, including admission reservations when explicit token/cost ceilings are requested. Preserve existing defaults and deny unsupported ceiling configurations before delivery.

Acceptance:

- Execute P01–P04 and A01–A06. Prove deny precedence, exact matching, stable rule IDs, capability intersection and equivalent CLI/ACP host configuration.
- Prove unlimited default counts, zero call limits, checked integer accounting, reservation before delivery, settlement on success/failure/cancellation, unknown usage preservation and no work after exhaustion.
- Providers lacking a trusted per-call upper bound must reject hard token/cost ceilings before sending. Neither post-response usage nor a guessed price qualifies as a spending guarantee.
- Run Rust workspace tests, formatting/clippy, TypeScript typecheck, ACP/telemetry tests and project checks. Keep any unavailable live-provider accounting capability explicitly unsupported.

## C2.5: Acceptance and compatibility

Prove the implemented C2 contracts end to end, retaining the C1.7 methods and behavior.

Acceptance:

- Execute all C2 fixtures through supported surfaces on macOS arm64 and native Linux x86_64; additional Linux arm64 VM evidence remains useful but cannot substitute for the native gate. Missing access is an explicit blocker with an unblock condition.
- Run the full offline Rust/Node/TypeScript suite, a real Collector filesystem correlation proof and a bounded, explicitly invoked live Vercel ACP filesystem task. Keep credentials and raw traces in ignored local storage.
- Review every project-owned ACP extension's name/schema/version/capability/size/privacy/fallback requirements from section 11.5. Retain compatibility or document and test an explicit version transition; generic peers must still work. Do not invent a project-controlled URI without establishing ownership.
- Compare release size, startup, memory, event delivery and relevant filesystem tasks with C1 methods and fingerprints, reporting setup/process/tool/transport costs separately. Use 30 samples after five warm-ups where the existing method applies. Record observations without ratifying provisional ceilings.
- Record the resulting alpha.1 capability assessment and remaining release gates in evidence/backlog. Publishing or tagging requires a separately selected action.

## Deferred boundary

OpenRouter, Open Responses, MCP, Skills, model structured-output validation/repair, temporary children, A2A, TUI, Otto acceptance, durable sessions, publishing and full 0.1 hardening remain in the [backlog](../backlog.md). C2 completion alone does not mean the complete 0.1 contract is satisfied.
