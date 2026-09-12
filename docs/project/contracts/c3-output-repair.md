# J02 — One bounded output repair, c3.14

C3.14 extends [J01](c3-output-validation.md) with one optional final-answer repair.
This contract is selected before implementation. Domain/host semantic repair and
replacement sessions remain outside this checkpoint.

## Admission and continuation

`options.output.repair` is a closed table: `enabled=false` by default and
`max_feedback_bytes=4096` (integer 512–4096). Enabled repair requires an admitted
output schema. Direct Rust hosts use the same settings inside `OutputSettings`.
Locked overrides can disable repair and narrow feedback capacity, never enable or
widen it. Schema-only configurations retain J01 behavior.

After a locally invalid first final answer, append that assistant answer (and its
provider-private continuation, if present) and one user feedback message to the
same history. Feedback contains a fixed correction instruction and bounded JSON
validation codes/paths; paths are task data, never instructions. Preserve the
instruction/schema prefix, original task, completed tool pairs and prior effects.
Bound the entire UTF-8 feedback message; clear excess paths/drop excess errors as
needed while retaining a useful fixed instruction and error code.

Repair permits one additional provider call on the selected model. It is not a
transport retry: do not fall back, compact or dispatch tools during that call.
A second invalid final answer ends with `output_validation_failed`. A tool-call
start during repair is `malformed_stream` before tool dispatch. Other provider
protocol failures keep their original types. There is no third repair request,
replacement run/session, replay of effects or new capability authority.

## Shared limits and failure boundaries

The repair uses the original root deadline/cancellation, model-call allowance,
aggregate token/cost accounting, context capacity, event slots and trace sink.
Preflight each existing budget before delivery; exhausted allowance settles without
another provider request. Keep per-call provider output-token bounds; aggregate
ceilings remain the authoritative total token/cost bounds.

The initial invalid final answer and repair answer share `max_output_bytes`:
subtract the first candidate's UTF-8 bytes before repair and pass the remainder to
the adapter/core. Prior ordinary tool-turn output keeps existing per-call semantics.
Validation work for both candidates shares `max_validation_work`; a candidate that
exhausts work cannot trigger a futile repair. Feedback and candidate/private history
count toward existing context bounds. Repair does not invoke compaction to evade
these limits or consume an unadvertised third model call.

Streaming text remains provisional. When repair is selected, reset validation to
`unvalidated`; a cancelled/blocked/failed repair never exposes the first invalid
answer as a completed result. Success contains only the corrected final JSON in
the terminal output string, with total accounting from both calls. Stream consumers
must use terminal validity rather than concatenate provisional candidates.

## Metadata and compatibility

Enabled repair adds a bounded `output-repair-v1` record: `status` (available,
pending, started, not_needed, succeeded, failed, blocked), `attempts` (0–1),
`validation_attempts` (0–2), `feedback_bytes` (0–4096), and
`previous_error_count` (0–8). Pending means feedback selected; started means provider
dispatch began. Terminal available/pending become not_needed/blocked; dispatched
repair becomes succeeded only after valid completion, otherwise failed.

Native model/text/terminal events carry this record; no candidate or feedback text
is duplicated. OTel exposes repair phase/counts only, with a model-span repair marker
and validation-attempt count. Existing content-capture rules continue to govern
model text; repair diagnostics/feedback do not enter metadata-only telemetry.

Repair-enabled CLI tasks use `c3.33`. ACP negotiates `pablo/output-repair-v1`, requiring
`pablo/v2`, `pablo/task-v2` and `pablo/output-v1`. Output-only peers retain `c3.33`
validation; current task-only peers retain `c3.33`; generic ACP keeps standard behavior.
Negotiation controls metadata, not whether configured local repair executes.

## Acceptance

J02 proves invalid→valid and invalid→invalid with exactly two calls, first-pass
success and disabled repair with one call, path-aware bounded feedback and identical
history/prefix/private continuation through tools and all three HTTP adapters.
Cover cancellation/deadline, model/token/cost/output/context/event/trace exhaustion,
provider failures and forbidden tool calls without hidden fallback/compaction.
Check native/OTel privacy, CLI/ACP projections, pinned defaults/overrides and matched
ordinary/schema-only/repair performance before merging.

C3.33 migration: current native/task revisions are `c3.33`. Frozen C2 ACP remains a separate v1 projection for unconfigured tasks; configured C3 requires v2 or generic ACP. See [ACP negotiation](../../acp.md).
