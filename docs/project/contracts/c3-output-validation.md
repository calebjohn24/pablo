# J01 — Final JSON output validation, c3.13

C3.13 validates one final model answer locally. It does not perform repair; C3.14
owns one bounded repair in the same conversation and root ledger. This output
contract is independent of the machine task envelope and model tool-call schemas.

## Schema admission

Use the existing pinned `jsonschema` 0.53.0 with retrieval disabled. Dialect is
Draft 2020-12; omitted `$schema` means that dialect, and another declared dialect
rejects. `format` is annotation-only, explicitly disabled as an assertion. Recognized
annotations are `$comment`, `title`, `description`, `default`, `examples`, `readOnly`,
`writeOnly`, `deprecated`, and `format`; none alters the instance.

The supported assertion/applicator subset is boolean schemas, `type`, `enum`,
`const`, numeric bounds/`multipleOf`, string length bounds, object `properties`,
`required`, `additionalProperties`, `propertyNames`, `dependentRequired`,
`dependentSchemas`, array `items`, `prefixItems`, item/property count bounds,
`allOf`, `anyOf`, `oneOf`, `not`, and `if`/`then`/`else`. Support local JSON-pointer
`$ref` and `$defs`, with no cyclic references or percent-encoded URI fragments. Unknown keywords and unsupported
assertions reject, including regex/pattern properties, uniqueness/contains,
unevaluated keywords, dynamic references, `$id`/anchors and custom vocabularies.
External references, file/network retrieval and alternate dialects are rejected
before compilation. Standard meta-schema shape validation still applies.

Admission bounds: 65536 UTF-8 schema bytes, 4096 JSON nodes, depth 32, and 4096
expanded schema visits including local-reference expansion. Each reference target
must be a schema. Bound the process cache to 16 successful compiled schemas, keyed
by SHA-256 of deterministic `serde_json::Value` serialization (sorted object keys;
arrays preserved). Equivalent object ordering reuses compilation. Errors are safe
codes, never compiler messages containing source/schema values. No dependency or
online schema catalog is added.

Validation admits at most the root output byte limit and a separate 1048576-byte
JSON ceiling, depth 64 and 65536 instance nodes. Work is conservatively charged as
(expanded schema visits plus comparison payload bytes for enum/const/type/required/
dependentRequired) × instance UTF-8 bytes, capped by the configured work bound.
The subset excludes regex and quadratic uniqueness; static expansion and input
bounds keep each synchronous library operation finite. Check cancellation/deadline
before and after this bounded work, and between returned diagnostics; do not detach
validation work after terminal completion. At most eight errors, with 256-byte
instance/schema JSON-pointer paths and a fixed code, with at most 2048 serialized
diagnostic bytes (drop excess paths/errors). No instance values, expected
constants or free-form validator messages. `format` never fetches or asserts.

## Configuration and provider request

Deployment revision `c3.13` adds `options.output.schema`: `{unset=true}` (default),
an inline JSON schema string, or a normal bound path reference to a local JSON file.
Files are regular, bounded, UTF-8 JSON and resolved into a pinned inline schema
before effective fingerprinting. Rendering embeds the pinned schema; inspection
retains declaration provenance and adds a canonical digest/source when normalizing
a file or noncanonical inline declaration. Terminal validation always reports the
canonical schema digest. `options.output.max_validation_work` defaults to
16777216, positive integer at most 1000000000. No repair configuration is admitted
in this checkpoint. Ordinary composition/override authorization applies; a schema
is replaced as one scalar contract, never merged as arbitrary policy keys. In a
locked deployment, allowed work-bound overrides may only narrow the bound; schema
overrides must equal the existing scalar declaration.

`--output-schema PATH` is the CLI/ACP-process shorthand for the same bounded schema
file, with explicit deployment override authorization before file access. Direct
Rust hosts supply typed output settings. Configured and direct paths compile the
same contract before provider dispatch. Plain text keeps existing behavior.

A schema request adds a stable user-visible instruction to produce a final JSON
value matching the admitted schema. The schema/instruction is counted in request
context and preserved across tool turns and compaction. No provider-specific
schema hint is required or trusted; local validation is authoritative.

## Results and privacy

Streamed answer chunks are provisional, never evidence of schema validity. A
bounded `output-validation-v1` record distinguishes `unvalidated`, `valid` and
`invalid`, includes the schema digest and safe diagnostics, and never carries a
second copy of the model output. Successful outcome text contains the validated
JSON serialization as supplied; hosts may parse it only after `valid`. Invalid
JSON/schema/work/input bounds produce typed `output_validation_failed`; existing
root output-byte, cancellation/deadline and provider failures preserve their types
and retain unvalidated status. No invalid final text is returned as a completed
result. No repair/fallback is triggered by local validation failure.

Schema-enabled CLI JSON uses task revision `c3.33` with `output_validation`;
ordinary tasks retain `c3.33`. ACP peers negotiate `pablo/output-v1` together with
`pablo/v2` and `pablo/task-v2` for the extended terminal task and provisional-text
correlation. Other peers retain standard text chunks and safe error/stop behavior;
legacy task peers get the prior envelope. New native terminal metadata reports
validation status; OTel exposes only digest/status/count/work, not diagnostics or
schema/model text. JSONL obeys existing content capture policy; no diagnostic path
containing model-supplied property names is exported to OTel or redacted JSONL.

## Sources and acceptance

[Draft 2020-12 validation](https://json-schema.org/draft/2020-12/json-schema-validation)
provides the canonical assertion semantics and annotation distinction. The pinned
[Rust validator](https://docs.rs/jsonschema/0.53.0/jsonschema/) provides local compiled
validation; Pablo owns its explicit subset, no-retrieval policy and work bounds.
J01 covers raw/file/configured schemas, local definitions/references, cache reuse,
unsupported/cyclic/external schemas, malformed/violating/oversized final answers,
path-safe diagnostics, cancellation, tool/compaction continuation, all adapters,
CLI/ACP capability fallback and matched ordinary/structured performance.

C3.33 migration: current native/task revisions are `c3.33`. Frozen C2 ACP remains a separate v1 projection for unconfigured tasks; configured C3 requires v2 or generic ACP. See [ACP negotiation](../../acp.md).
