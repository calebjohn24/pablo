# OR01 pinned Open Responses examples

These are contract examples for [C3.8](../../contracts/c3-open-responses.md), not evidence that an Open Responses runtime adapter exists. Run the offline audit from the repository root:

```sh
node --test docs/project/fixtures/c3-open-responses/audit.mjs
```

`upstream-pin.json` identifies immutable revision `92c12d96d7b61d6d15e2214daa5e9c6000ab6e1c` of `openresponses/openresponses`, with unmodified Apache-2.0 upstream sources and hashes in `upstream/`. OpenAPI release 2026-04-24 is validated directly with the existing Ajv 2020 validator; no dependencies or network schema lookups are added. OpenAPI documentation/discriminator annotations do not alter JSON Schema validation. No runtime accepts arbitrary schema retrieval because this audit exists.

`roundtrip.json` is a synthetic three-turn example: a commentary message and first file read, a second file read with a private summary, then a two-part Unicode answer. Complete function/message/reasoning items are fed back in order before each correlated tool result. Opaque state and summaries stay private; assistant phases survive continuation. Usage aggregates to 450 input, 90 output and 240 cached-input tokens. Cache-write and cost remain unknown. Synthetic model IDs and markers do not name a live service or credential.

`turn-1.sse` through `turn-3.sse` are actual UTF-8, CRLF-framed wires matching the JSON events, including keepalive comments, named events, fragmented argument/text semantics and a data-only DONE sentinel. C3.9 must additionally split these wire bytes arbitrarily over actual HTTP; these static files do not prove incremental runtime parsing.

`profile.json` freezes the configured capability and protocol bounds. `terminals.json` adds upstream-valid completed, incomplete, failed and null/zero usage examples with the selected finish/error mapping; these isolated terminal examples are not complete streams.

The audit verifies immutable bytes; all request/event shapes; SSE/JSON parity; independent lifecycle and two-round continuation consistency; ten malformed schema cases; thirteen schema-valid lifecycle mutations plus missing DONE; usage accounting; and a raw-reasoning shape that is output-valid but cannot be losslessly resubmitted. Negative runtime behavior, cancellation, all-surface privacy and performance remain OR02. Upstream acceptance source is retained for provenance and review; this audit does not execute or claim the full upstream service compliance suite.
