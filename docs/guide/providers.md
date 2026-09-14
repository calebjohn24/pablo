---
title: Models and providers
description: Select Vercel, OpenRouter or Open Responses, configure reasoning and use ordered model fallback.
---

# Models and providers

Pablo normalizes streamed model output into text deltas, tool calls, finish information and reported accounting. A provider owns its credentials and wire mapping; the runtime owns the conversation, tool loop, limits and terminal outcome.

## Built-in gateways

| Provider | Legacy selection | Default model | Credential |
| --- | --- | --- | --- |
| Vercel AI Gateway | `--provider vercel` | `zai/glm-5.3-flash` | `AI_GATEWAY_API_KEY` or alias `VERCEL_AI_GATEWAY` |
| OpenRouter | `--provider openrouter` | `z-ai/glm-5.3-flash` | `OPENROUTER_API_KEY` |
| Open Responses | deployment only | explicit | named `provider.open_responses` credential |

Vercel and OpenRouter use direct HTTPS with streamed SSE. Requests have no automatic HTTP retry, proxy inheritance or redirect following. Provider errors are mapped to closed failure codes instead of copying remote response bodies into task output or diagnostics.

```sh
pablo run "Explain this repository." --provider vercel --no-shell
pablo run "Explain this repository." --provider openrouter --no-shell
```

Use `--model ID` for another compatible chat-completions model. Model support is validated before task execution where the adapter has an explicit capability contract.

## Reasoning controls

The default is `provider_default`, leaving the remote model’s reasoning behavior unchanged.

```sh
pablo run "Analyze this design." --reasoning-effort low --no-shell
pablo run "Analyze this design." --reasoning-budget-tokens 512 --no-shell
```

Effort values are `none`, `minimal`, `low`, `medium`, `high`, `xhigh` and `max`. An effort and an explicit token budget are mutually exclusive. Support depends on the selected provider/model capability declaration; Pablo does not clamp or silently translate an unsupported value.

In TOML:

```toml
[options.model]
provider = "openrouter"
id = "z-ai/glm-5.3-flash"
credential = "router"
reasoning = "low"

# Alternative:
# reasoning = { budget_tokens = 512 }
```

Reasoning intent follows a model call through fallback, compaction and output repair. Private reasoning payloads from Open Responses never enter public events, traces, ACP updates or OTel.

## Open Responses

Use an explicit deployment for an OpenAI-compatible Responses endpoint:

```toml
schema_version = 1

[credentials.responses]
consumer = "provider.open_responses"
sources = [{ kind = "environment", name = "RESPONSES_TOKEN" }]

[options.model]
provider = "open_responses"
id = "your-endpoint-model"
endpoint = "https://provider.example/v1/responses"
capability_profile = "open-responses-text-tools-v1"
credential = "responses"

[options.run]
workspace = { base = "binding", name = "workspace", path = "." }

[options.shell]
enabled = false
```

```sh
pablo config validate --config responses.toml --bind workspace=./work
pablo run "Summarize README.md." --config responses.toml --bind workspace=./work
```

The repository includes this [Open Responses example](https://runpablo.pages.dev/examples/open-responses.toml) as a standalone file.

The endpoint, model and capability profile are all required. No endpoint is inferred from a model name. The implemented profile supports streamed text, sequential function calls, full ordered input history, opaque continuation and reported token/cache-read usage. Unsupported modalities, hosted tools and background execution fail explicitly.

## Named model profiles

Deployments can name models independently of routes:

```toml
[options.models.primary]
provider = "openrouter"
id = "z-ai/glm-5.3-flash"
credential = "router"

[options.models.secondary]
provider = "vercel"
id = "zai/glm-5.3-flash"
credential = "gateway"

[options.routes.default]
entries = [
  { model = "primary" },
  { model = "secondary" }
]

[options]
model_route = "default"
```

The route order is authoritative. It is not a dynamic quality or price ranking.

## Fallback behavior

A selected model remains sticky for later calls. Pablo moves to a later entry only for configured eligible failures, such as a mapped transient service/rate-limit response or a failure proven not sent.

Fallback does not occur for cancellation, policy denial, invalid configuration, missing credentials or unsupported provider content. It stops after partial text or a tool call escapes, because replaying could duplicate visible output or side effects. Uncertain delivery requires explicit opt-in and retains conservative accounting reservations.

Every attempt uses the root task’s time, call and accounting ledger. Fallback does not restart completed tools or discard accepted conversation history.

## Context compaction

When enabled, compaction reduces accepted model history before the context limit is exceeded. It preserves the original task, selected Skill instructions and exact facts identified by the task-relevant contract, then keeps configured recent turns.

```toml
[options.context]
enabled = true
safety_margin_percent = 10
max_summary_tokens = 1024
max_summary_bytes = 16384
keep_recent_turns = 0
```

Compaction is itself bounded model work and participates in the root ledger. It never expands tool authority or turns a dropped private continuation into compatible public history.

## Usage and cost

Reported provider usage is accumulated with checked arithmetic. Missing data remains unknown. OpenRouter’s reported decimal USD charge is normalized once to integer micro-USD; Pablo does not fetch price tables or estimate missing cost.

`--max-total-tokens` and `--max-cost-microusd` require the adapter to attest an enforceable per-call upper bound before sending. The live gateway adapters do not currently make that guarantee, so they reject these hard ceilings before delivery.
