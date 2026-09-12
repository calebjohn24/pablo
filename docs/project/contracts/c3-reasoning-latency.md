# Reasoning controls and model-call timing

C3.33c adds optional reasoning intent without changing provider defaults. The core carries the same typed setting through ordinary calls, tools, compaction, output repair and inherited routes. There are no model-name branches or live catalog lookups.

```sh
pablo run "Prepare the handoff" --reasoning-effort low
pablo run "Review the evidence" --provider openrouter --reasoning-budget-tokens 512
```

Effort is one of `provider_default`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. Effort and a token budget are mutually exclusive. `provider_default` omits the field; it does not mean reasoning is disabled. A budget must be a positive u32 below the request's output-token allowance.

For a single configured model:

```toml
[options.model]
reasoning = "low"
# Alternative: reasoning = { budget_tokens = 512 }
# Optional operator declarations, not remote attestations:
reasoning_capabilities = { efforts = ["low", "high"], mandatory = true, token_budget = false }
```

For a named route model, put reasoning in `options.models.NAME.model_options.reasoning` and declarations in `options.models.NAME.capabilities.reasoning`. For example, add `model_options = { reasoning = "low" }` inside the existing `[options.models.primary]` table. Every route entry retains its own setting. Child route subsequences preserve entries and their identities. Root CLI model overrides conflict with a selected route; configure its named entries instead. Locked deployments apply their existing override allowlist to `model.reasoning`.

OpenRouter and Vercel Chat Completions receive `reasoning: {effort: ...}` or `reasoning: {max_tokens: ...}`. The pinned Open Responses schema supports only `none`, `low`, `medium`, `high`, and `xhigh`; `minimal`, `max`, and token budgets reject before dispatch. Unknown model capabilities forward supported protocol syntax; a provider can still reject an unsupported model setting. Explicit declarations reject unsupported levels, disabling mandatory reasoning, or forbidden budgets locally. Nothing is silently clamped or discarded.

Compaction uses the selected setting with its summary output allowance. A budget that cannot fit that smaller allowance fails compaction with `summary_reasoning_unsupported`; increase `context.max_summary_tokens` or choose a smaller budget. The runtime does not silently lower reasoning quality. Open Responses private continuation items retain their existing privacy and scope rules.

## Timing and privacy

Each native `model.finished` event optionally includes `diagnostics`. Existing c3.33 events without this field remain readable. The task envelope and ACP negotiation stay unchanged. Metadata includes requested reasoning, an explicitly reported effort when present, reported reasoning tokens, negotiated HTTP version, and monotonic microsecond offsets:

| Offset | Boundary |
|---|---|
| `preparation_us` | Adapter finishes preparing the request body |
| `dispatch_us` | Immediately before the HTTP send |
| `headers_us` | Response headers become available |
| `first_data_us` | First nonempty body chunk, including heartbeat or private reasoning data |
| `first_text_us` | First nonempty public text delta |
| `first_tool_delta_us` | First tool-call start |
| `terminal_us` | Validated provider terminal event reaches the runtime |
| `complete_us` | Runtime finishes the call and settles accounting |

Offsets share a per-call monotonic origin. Missing phases are null, including after cancellation or rejection; they are never replaced with zero. A tool-only call need not have a first-text timestamp. Header and first-data waits combine network, gateway queues and provider processing; they cannot identify pure network latency. Reasoning tokens are a subset of output tokens, never an additional accounting charge. No returned reasoning text, request body, response headers or credentials are included. The same bounded metadata is available on model OpenTelemetry spans under `pablo.model.timing.*`, `pablo.model.reasoning.*`, and `network.protocol.version`.

## Measurement and default changes

The latency matrix uses the existing six-task benchmark with seeds 41–43 and three repetitions, rotating five configurations serially: Pablo GLM/Astra at provider default and low, and Pi GLM at low. All attempts remain in the denominator. Builds and checks finish before retained trials; raw local artifacts remain ignored. CPU and RAM measure local processes, not inference servers.

A faster profile default requires at least 20% lower p50, no worse p95, factual accuracy overall and per task family, completion rate or total cost, plus review of report detail. Failed or unknown gates retain defaults. HTTP/2 is evaluated separately against HTTP/1.1 before deciding whether to change transport features. Neither experiment adds retries, output caps or prompt shortcuts to the runtime.

Protocol references: [OpenRouter reasoning](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens), [Vercel reasoning](https://vercel.com/docs/ai-gateway/models-and-providers/reasoning), and the [pinned Open Responses schema](../fixtures/c3-open-responses/upstream/openapi.json).
