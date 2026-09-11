# Basic context compaction — CP01/CP02, c3.12b

This checkpoint implements one task-local summary pass and one overflow recovery.
It does not add durable sessions, persistent memory, retrieval, an online model
catalog, tokenizers or a provider-specific encrypted compaction service. See the
[scope review](../evidence/c3-compaction-scope.md) and design sections 23/28.8.

## Configuration and estimation

Deployment schema v1 advances to revision `c3.12b`. `options.context` has:

| Field | Default | Bounds / meaning |
| --- | --- | --- |
| `enabled` | true | Enables the single compaction allowance. |
| `safety_margin_percent` | 10 | Integer 1–50; leaves room before declared token capacity and the existing raw context byte ceiling. |
| `max_summary_tokens` | 1024 | Integer 16–65536, clamped to the selected profile/root output bound. |
| `max_summary_bytes` | 16384 | Integer 256–1048576, clamped to root output bytes. |
| `keep_recent_turns` | 0 | Integer 0–32; minimum raw recent complete groups to retain. Zero prefers summarizing all completed turns. |

Legacy `options.model.context_window_tokens` and each named model's same field
accept a positive integer up to 1,000,000,000 or `{unset=true}` (unknown, the default).
No live capacity is inferred from a model name. The resolved profile exposes the
optional capacity; each route entry owns its own value. Direct-core hosts provide
it in typed context settings. Context options compose normally but are not ordinary
per-task overrides. Existing root budget/authority ceilings still constrain every
summary and recovery; context settings cannot grant a new provider or capability.

Measure the existing complete logical request bytes, including instructions,
messages and tool descriptors; substitute complete private continuation bytes for
its public projection. For token estimation add 512 framing bytes plus 64 per
message/tool descriptor. Active Skill text will be part of instructions/messages
when that owning slice is implemented. Start with ceil(bytes/4) and conservatively
calibrate each exact profile from its latest nonzero reported input usage: use the
larger of that baseline and ceil(current_bytes * reported_tokens / reported_bytes).
Use checked/widened integer arithmetic, never float money or a tokenizer. Missing
usage/capacity remains unknown rather than becoming zero.

Before a normal request, trigger when raw bytes reach the byte ceiling minus its
margin, or estimated input plus selected maximum output reaches the declared token
window minus its margin. The latter trigger is absent for unknown windows. The soft trigger requires replaceable history; otherwise normal hard admission applies. Initial
oversized input with no replaceable history stops explicitly; it is not truncated.
After the one successful compaction, normal hard byte/token admission still applies;
no recursive compaction occurs. Declared capacity is an operator constraint, not a
provider attestation or a new hard spending guarantee.

## Summary and atomic history replacement

A complete turn is an accepted assistant tool-call item plus all of its settled
results, including multiple tool calls. Keep the original user task verbatim. Prefer summarizing all completed groups,
including bulky recent results, while retaining any configured minimum raw suffix.
If the summary source cannot fit, try larger recent suffixes through 32 groups,
retaining the fewest that fit. Summarize all selected older complete
groups in one request on the currently selected exact provider/model, including
its required private continuation. Keep the system/instruction and tool-schema
prefix unchanged; append a bounded runtime summary request as user-role task data.
It requests original constraints, decisions with reasons, exact findings/numbers/
units/dates/identifiers, uncertainty, failed approaches, next steps, artifact/path/
revision references and completed effects, and forbids performing new work. Dense
factual sections replace repetition and raw logs; fidelity takes priority over a
fixed reduction ratio. This implements the user-selected task-relevant summary
policy, without a raw-output archive or a claim of lossless arbitrary recall. Do not promote
summary text to system authority. Tools remain present in the stable prefix with
selection disabled, and any attempted summary tool call fails before an effect.

The summary request includes all selected groups, including newest results when
none are retained. It excludes retained recent turns to leave room after overflow.
It must itself fit the known usable capacity and hard byte limit; never drop an
older group to make an unreviewed partial summary. If it cannot fit, retain history
and settle explicitly. The original task is also retained outside the summary, so
semantic summary quality cannot erase it. No test can prove perfect semantic recall;
fixtures prove preserved constraints/references and unchanged execution boundaries.

Run at most one summary model request per task; it does not use route fallback.
Validate nonempty text, Stop finish, no tools, bounded bytes/tokens and a strictly
smaller replacement that fits admission. Build the candidate separately, then
atomically replace the old groups with a user-role, explicitly labelled derived
history summary. Retain recent private carriers exactly and remap their message
indices; discarded carriers disappear only with the successful boundary. Summary
private output is never exposed or flattened. Failed/empty/oversized/non-shrinking
summaries, cancellation, insufficient room/budget or event delivery failure preserve
the original history. Existing completed tools, IDs and accounting are not reset.

## Overflow and budgets

`context_overflow` is a closed provider failure, not a route-transient class. Match
only explicit `context_length_exceeded` / `context_window_exceeded` codes in bounded
JSON error envelopes (including one nested provider error envelope); do not infer
context overflow from arbitrary free-form messages or generic HTTP 400/413. For
HTTP 400/413, inspect at most 8192 private bytes within the existing request deadline;
malformed, oversized or other errors remain provider rejection. Streamed error
frames use the same code classifier. Error text is never logged or returned.

An overflow before public text or tool accumulation may spend the same compaction
allowance, then make at most one recovery request on the same selected entry. No
route fallback is permitted for that recovery request. Another overflow settles
without another summary. A proactive summary already spent the allowance. Normal
later successful operations retain the existing sticky route policy.

Summary and recovery both spend root model calls, deadlines, tokens and cost using
the normal reserve/settle lifecycle. Preflight summary bounds for the selected
request shape before delivery. Require room for a summary and a following generation
when a root call-count ceiling exists. Before committing a candidate, also probe
token/cost admission for the following generation on a cloned ledger. Failed/uncertain delivery retains reservations.
Cancellation/root expiry outrank compaction and cannot create more requests.

## Events and privacy

Native `context.compaction.started` / `context.compaction.finished` events and one
`compact_context` OTel span expose a UUID, trigger (`threshold` or `overflow`),
before/after raw bytes and estimates, nullable capacity, removed group count,
replaced-history SHA-256 and completion status. Fingerprint complete discarded
public/private history without copying it into diagnostics. Native numeric counters
in new metadata use decimal strings. A summary model span is a child of the
compaction span, in the same root trace and accounting ledger. Its text deltas never
appear as ordinary assistant output. The terminal carries only bounded compaction
metadata; the closed task envelope stays `c2.3`.

Live core events may carry summary text at compaction finish. JSONL exposes it only
with `capture_content=true`; otherwise it contains a null summary and byte count.
OTel is always metadata-only, including summary length but no summary content.
ACP peers opt into `pablo/compaction-v1` plus `pablo/v1`; `_pablo/compaction`
notifications carry session/type/correlation and metadata, plus summary only when
content capture is explicitly enabled. They share physical-write backpressure and
shutdown with existing notifications. Other clients keep existing shapes. No raw
private continuation or credentials enter either capture mode. Every new ACP
session gets fresh history, calibration and compaction allowance.

## References and proof

The local Codex inspiration and official compaction reference are recorded in the
scope review. [Open Responses](https://www.openresponses.org/reference) specifies
that disabled truncation rejects over-context input; Pablo keeps this explicit
boundary. [OpenRouter message transforms](https://openrouter.ai/docs/guides/features/message-transforms)
describe provider-side middle-message removal, which is separate from this runtime
summary boundary. No gateway plugin or provider model fallback is added. Only
recognized machine-code errors trigger recovery; undocumented gateway wrappers
remain explicit failures rather than guessed overflow classification.

CP01/CP02 must exercise actual core, CLI and ACP continuation across all three
adapters, byte reduction, preserved prefix/task/recent pairs/private state, one
summary/recovery, shared budgets, failure atomicity, privacy/session isolation and
metadata/OTel correlation. Measure ordinary overhead and actual compaction/recovery
workloads, recording context reduction separately from runtime latency.
