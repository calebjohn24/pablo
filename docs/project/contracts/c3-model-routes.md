# Ordered model profiles and routes — F01/F02, c3.11

C3.10 introduced offline named-profile/route resolution. C3.11 implements ordered runtime fallback and distinct attempt deadlines through core, CLI and ACP; C3.12 owns complete failure/accounting acceptance. Schema v1 advances to contract revision `c3.11`. Legacy `options.model` and provider defaults retain their behavior when no route is selected.

## Configuration and resolution

`options.models` is a map of at most 64 named model records. Each composed record requires explicit `provider`, `id` and `credential`; gateway endpoint omission resolves to that provider's fixed URL. Open Responses additionally requires its explicit HTTPS endpoint and capability profile, with existing authentication defaults. Records accept existing provider-specific fields, `model_options.max_output_tokens` (positive u32, default root output bound), and capability narrowing through `capabilities.text_streaming` / `tool_calls` booleans. They cannot add adapter capabilities, arbitrary model parameters, inline secrets or gateway fallback lists. Remote model compatibility remains an operator declaration; offline resolution never queries a catalog or reads credentials.

`options.routes` maps at most 64 names to records with `entries`, an ordered nonempty array of at most 32 `{model="name"}` or `{route="name"}` references. Route references compose only ordered entries: a containing route owns its own policy, with no hidden nested retry scope. Expansion preserves order, permits at most 16 reference levels and 32 final entries, and rejects missing names, cycles, repeated route references and duplicate resolved model entries. A selected route cannot skip an invalid, unsupported or unauthorized entry and claim the rest is valid. All composed named profiles/routes validate offline, including unselected catalog entries. Cross-references are checked after deployment-layer composition; the separate top-level deployment `profiles` namespace retains its existing inactive-layer rules.

`options.model_route` selects one route by name. Without a selection, named declarations may be inspected but legacy `options.model` remains active. With a selection, the selected route determines model identity/options; the legacy model remains a compatibility default in the rendered config and is not an extra fallback. Legacy per-task `--model`/`--provider` overrides conflict with a selected route; change a named profile or route explicitly instead. Selection has no `unset` form in this revision; use an entry without `model_route` for legacy selection. Normal map composition and array replacement apply; route references are not pattern matching or dynamic ranking. Configured route lists have no append/prepend shorthand. A rendered snapshot must reload with the same effective identity. Derived profile defaults retain provenance.

Each resolved entry exposes its name, provider, exact model, endpoint, credential reference, adapter protocol/capabilities and effective output-token bound. `config explain` additionally exposes declared order, resolved order, initial first entry, retry policy, selection reason and execution availability. Each entry separately labels provider-managed upstream routing `opaque_to_pablo`; it is not another Pablo attempt counter. No secret value or credential-presence probe is included. The effective output-token allowance is the minimum of the root allowance and profile allowance; Open Responses still requires at least 16. Requests never increase an inherited root allowance.

## Policy and attempt ownership

Each route has these resolved fields:

| Field | Default and constraint |
| --- | --- |
| `max_attempts` | Flattened entry count; 1–32 and no greater than that count. Counts dispatch attempts for one logical model operation, including its first attempt. No entry is revisited. |
| `per_attempt_timeout_ms` | Root run-duration allowance; positive, at most 86,400,000 ms. An actual attempt deadline is the earlier of this duration and the remaining root deadline. |
| `eligible_errors` | `not_sent`, `rate_limited`, `service_unavailable`; unique subset, possibly empty to disable failure continuation. Optional `transport_uncertain` additionally requires `retry_uncertain_delivery=true`. |
| `retry_uncertain_delivery` | `false`; explicit opt-in is additionally required for any eligible transport failure whose delivery is uncertain. |
| `retry_owner` | Literal `pablo`. No host/gateway retry owner can simultaneously spend this route's attempt allowance. |
| `sticky` | Literal `true`: after successful selection, later logical model operations start there, never at an earlier entry. |
| `required_capabilities` | `text_streaming`, plus `tool_calls` when the configured catalog enables tools. Explicit requirements may add restrictions but cannot omit capabilities needed by the active request. `structured_output` is recognized but currently unsupported by these adapter profiles. |

C3.11 maps HTTP 429 to rate-limited and narrowly selected 502/503/504 responses to service-unavailable without exposing response bodies. Other HTTP errors, provider refusals, unsupported content, malformed streams, invalid configuration/credentials, policy denials, cancellation, root limits and accounting violations do not trigger fallback. A proven pre-delivery transport failure is `not_sent`; generic uncertain transport loss requires both the `transport_uncertain` class and its explicit opt-in and cannot be relabeled proven unsent. There is no backoff loop, repeated-entry retry or gateway model-list option. Opaque gateway upstream routing is reported separately from Pablo's ordered attempts and cannot attest hard spending ceilings.

Selection is sticky only within one task. Every attempt is its own model call/span, consumes the shared root count/time/accounting allowances and retains conservative charges for uncertain delivery or missing actual usage. Prevalidate enforceable hard accounting bounds across every potentially eligible entry before any dispatch. No successful tool is replayed, and no task is restarted. Fallback stops once public text or tool-call output has escaped from the failed attempt. A later model operation continues from real completed tool results. Output repair is a separate later operation in the same run, with its attempts using these same root budgets.

## Continuation and inherited authority

Ordinary text/tool history may cross gateway profiles that can faithfully represent it. Open Responses private continuation requires exact provider, endpoint, requested/resolved model, protocol revision and capability-profile compatibility; no flattening or silent loss is allowed. Distinct credentials may be selected only through their declared scoped references without changing that compatibility identity. An incompatible next entry stops fallback with explicit safe metadata, rather than skipping over it or discarding prior state. No adapter-global continuation cache is introduced.

Every eligible entry must satisfy every host/deployment `model_ids`, `provider_endpoints` and `credential_ids` ceiling. Configured per-profile options are narrowed by root limits. Locked deployments cannot accept route/profile overrides through ordinary task flags. Child configuration, once introduced by its owning checkpoints, receives an immutable approved resolved route or ordered subsequence of exact entries; it cannot substitute a same-named profile, reorder, repeat or append entries/destinations, widen capabilities or increase attempt/deadline/error/uncertain-delivery permissions. F01 provides the typed subsequence operation now, without activating child execution.

## Verification boundary

F01 requires real Rust resolution and CLI validation/explain/render with a three-provider route, precise order/options/credential/capability identities, bad/missing/duplicate/cyclic references, unsupported requirements and all-entry authority checks. It verifies typed inherited narrowing and render/reload equivalence. A single-entry route must perform the same actual CLI/ACP tool task as the corresponding legacy profile, without fallback effects. C3.11 enables multi-entry execution and shortened attempt deadlines; `execution_available` is true for validated routes. The retained `fallback_owner` field identifies C3.11 as the implementing checkpoint. Ordinary tests remain local/offline and preserve selected gateway GLM models and unlimited filesystem work.

## F02 runtime behavior

The host constructs immutable adapters for every resolved route entry, with its exact credential reference and destination. All credential and enforceable accounting-bound checks complete before the first request. ACP may reuse those resources only for matching deployment, bindings and private credential values; each prompt starts a fresh cursor at entry zero. Resource reuse never preserves selection or task continuation.

The runtime emits a separate model start/finish/span for each dispatched attempt. An eligible failure advances exactly one entry; a later operation starts at the current entry. Each operation has its own `max_attempts` allowance while all attempts share root call/time/token/cost budgets. Per-entry output-token bounds are clamped independently against the root; a low primary bound cannot accidentally narrow a later entry. The prepared root RunSpec retains its root token ceiling.

A distinct attempt deadline returns `model_attempt_timed_out` with uncertain delivery. Root expiry remains `timed_out` and cancellation remains `cancelled`; neither triggers fallback. Explicit uncertain-transport opt-in may continue after an attempt deadline while retaining its reservation. HTTP 429 and 502/503/504 retain `provider_rejected` as the public failure code and have closed internal retry classes. Error bodies are never read. Received failures with unknown usage/cost retain their reservations; proven unsent failures settle to zero with their attempt count retained.

Fallback stops after any emitted text delta or begun tool-call accumulation. Accounting violations, sink/closing-event failures and noneligible provider failures also stop. An incompatible next adapter stops before dispatch with `continuation_incompatible`; the runtime neither discards private state nor skips to a later compatible entry. The current Open Responses adapter requires its own complete private projection for prior assistant items; ordinary cross-gateway history remains supported by the chat-completions adapters. Completed tools and accepted history are retained once.

F02 covers first failure → second success with no third request, sticky selection across a real filesystem tool, fallback after completed tool history, shared reservations, root call limits, cancellation, escaped text, incompatible private continuation, distinct attempt deadlines and fresh ACP reuse. C3.12 remains responsible for the complete failure matrix and explicit per-attempt failure ledger. No live route fallback or hard spending guarantee is claimed by offline fixtures.


## F03 attempt observability — model-route-v1

C3.12 adds optional `model_route` metadata to routed native `model.started`,
`model.finished` and `run.finished` events. Legacy runs omit it. The task envelope
remains the closed `c2.3` contract. Native `c2.4` permits this additive optional
metadata; deployment configuration remains `c3.11` because its meaning is unchanged.

The record contains `schema_version: "model-route-v1"`, route and entry names,
zero-based entry index, provider and requested model, decimal-string logical
operation number (one-based), one-based per-operation attempt, selection reason
(`initial`, `sticky`, `fallback`), phase (`selected`, `started`, `finished`,
`blocked`), dispatched boolean, delivery certainty, nullable status/failure code/
limit/retry class and nullable cumulative accounting snapshot. Snapshots are taken
at attempt settlement; the terminal event's top-level accounting remains final
run truth. Complete attempts are streamed, never accumulated in runtime memory.
The terminal carries only the latest selection, including pre-dispatch rejection;
a selected entry is not evidence that a request reached its provider. Admission
failures such as missing credentials occur before a run and have no attempt ledger.

CLI `--trace` exposes the complete native ledger under either content policy.
ACP peers negotiate both `pablo/v1` and `pablo/model-route-v1` in capability `_meta`.
Only those peers receive `_pablo/model_attempt` extension notifications containing
`sessionId`, native event `type` and `pablo/v1` correlation (including model_route).
The terminal response carries the latest record in the existing correlation
metadata. Standard session updates and the task envelope are unchanged. Extension
notifications use the same bounded queue, physical-write backpressure and shutdown
as ordinary updates; there is no detached notification channel.

Delivery is tracked separately per attempt. Once a provider event is received,
subsequent transport errors cannot downgrade it to proven unsent, refund its
reservation or bypass uncertain-delivery opt-in. A provider's explicit NotSent
opening failure remains refundable. Cancellation/root limits and sink failures
never authorize a later request. Failed attempts retain one model finish and one
root outcome when the event sink remains writable.
