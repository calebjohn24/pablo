---
title: Structured output
description: Return machine envelopes and locally validate final JSON with one bounded repair attempt.
---

# Structured output

Pablo separates two concerns that are often both called “structured output”:

1. The **task envelope** is Pablo’s stable machine-readable result around every run.
2. The **model answer schema** validates the final answer string as JSON against a local schema.

You can use either or both.

## Task envelope

```sh
pablo run "Summarize README.md." --json --no-shell
```

Stdout contains one JSON object plus a newline. A completed result has this shape:

```json
{
  "schema_version": "c3.33",
  "run_id": "…",
  "session_id": "…",
  "trace_id": "…",
  "outcome": {
    "status": "completed",
    "output": "The project is …",
    "finish_reason": "stop",
    "usage": {
      "input_tokens": 420,
      "output_tokens": 82,
      "cache_read_input_tokens": null,
      "cache_write_input_tokens": null
    }
  },
  "error": null,
  "accounting": {
    "model_calls": "1",
    "tool_calls": "0",
    "usage": {
      "input_tokens": "420",
      "output_tokens": "82",
      "cache_read_input_tokens": null,
      "cache_write_input_tokens": null
    },
    "cost_microusd": null,
    "charged_tokens": null,
    "charged_cost_microusd": null
  }
}
```

Exact aggregate counts are decimal strings. Provider-native outcome usage uses JSON numbers when representable. Unknown values remain `null`; they are never guessed as zero.

Non-completed outcomes keep the same outer envelope. Pre-admission failures use `outcome: null`, null identities/accounting and a closed `error.code`.

The checked-in [`docs/pablo-task.schema.json`](https://github.com/calebjohn24/pablo/blob/main/docs/pablo-task.schema.json) is the machine contract. ACP returns the same task projection in negotiated metadata.

## Validate the answer itself

Create a Draft 2020-12 schema:

```json
{
  "type": "object",
  "properties": {
    "summary": { "type": "string", "maxLength": 500 },
    "risks": {
      "type": "array",
      "items": { "type": "string" },
      "maxItems": 5
    }
  },
  "required": ["summary", "risks"],
  "additionalProperties": false
}
```

Then run:

```sh
pablo run "Review README.md." \
  --json \
  --no-shell \
  --output-schema review.schema.json
```

Download the [example review schema](./examples/review.schema.json) from the repository.

Pablo adds a stable instruction asking for a matching JSON value, but local validation is authoritative. It parses and validates the final model string after model completion. A valid answer remains in `outcome.output` as a JSON string; parse it only after `output_validation.status` is `valid`.

## Supported schema subset

Supported assertions and applicators include:

- boolean schemas and `type`
- `enum` and `const`
- numeric bounds and `multipleOf`
- string length bounds
- object `properties`, `required`, `additionalProperties`, `propertyNames`, `dependentRequired`, `dependentSchemas`
- array `items`, `prefixItems` and item count bounds
- `allOf`, `anyOf`, `oneOf`, `not`, `if` / `then` / `else`
- local JSON-pointer `$ref` and `$defs`

`format` is annotation-only. Unsupported or risky features reject before provider dispatch, including external retrieval, regex/pattern properties, `uniqueItems`, `contains`, unevaluated/dynamic references, `$id`, anchors and custom vocabularies. Local reference cycles reject.

Schema admission is bounded by bytes, nodes, depth and expanded visits. Instance parsing is separately bounded by answer bytes, JSON nodes, depth and validation work.

## Diagnostics

An invalid final answer settles as `failed` with `output_validation_failed`. The envelope retains up to eight bounded diagnostics containing only a code plus instance/schema JSON-pointer paths. Instance values, expected constants and free-form validator messages are omitted.

Possible diagnostic codes include `malformed_json`, `json_bytes`, `validation_work`, `json_structure` and `schema_violation`.

## One repair attempt

Deployments can enable one bounded repair:

```toml
[options.output]
schema = '{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}'

[options.output.repair]
enabled = true
max_feedback_bytes = 4096
```

When initial validation fails, Pablo sends bounded structural feedback in the same conversation and allows exactly one additional model attempt. Repair shares the root deadline, context and accounting. It cannot run when model-call budget is exhausted or the remaining context cannot hold the feedback.

The final envelope records whether repair was `not_needed`, `succeeded`, `failed` or `blocked`, with attempt and validation counts. The invalid answer is never relabeled as valid because repair was attempted.

## ACP client handling

The TypeScript reference client exports `taskOf(response)` and `structuredOf(response)`. `structuredOf` parses output only after it verifies both a completed outcome and valid local output validation.

For your own client:

1. Negotiate Pablo metadata during ACP initialization.
2. Read the task object from `pablo/v2` metadata.
3. Require `task.output_validation.status === "valid"`.
4. Require `task.outcome.status === "completed"`.
5. Parse `task.outcome.output` with `JSON.parse`.
