---
title: Limits and outcomes
description: Default bounds, task results, accounting semantics, exit codes and supported platform behavior.
---

# Limits and outcomes

This page collects the runtime defaults most hosts need when admitting a task. Provider/MCP/A2A transports add their own frame, request and concurrency bounds.

## Default run limits

| Limit | Default |
| --- | ---: |
| Combined input/instructions/tool arguments | 1 MiB |
| Model output bytes per call | 4 MiB |
| Serialized tool result | 8 MiB |
| Serialized model request context | 32 MiB |
| Requested model output tokens | 65,536 |
| Native events | 1,000,000 |
| Native JSONL trace | 256 MiB |
| Run duration | 3,600 seconds |
| Shell/tool duration | 900 seconds |
| Model call count | unlimited |
| Tool call count | unlimited |
| Aggregate token/cost ceiling | unset |

CLI timeouts accept 1–86,400 seconds. A tool deadline cannot extend past its root task. Counts use checked arithmetic and never wrap.

Filesystem work quotas are unset by default. Individual tool responses remain paginated/bounded, and hosts can set maximum file bytes, entries, search depth and scanned bytes in a deployment or typed `RunSpec`.

## Terminal outcomes

| Status | Meaning |
| --- | --- |
| `completed` | Final output stopped normally; includes provider-reported usage |
| `cancelled` | Local cancellation won and owned cleanup settled |
| `timed_out` | Root deadline won |
| `policy_denied` | A built-in or configured policy rule denied work |
| `limit_exceeded` | A named runtime or accounting limit was exhausted |
| `failed` | A closed failure code and delivery certainty describe the failure |

Limit kinds include `input_bytes`, `output_bytes`, `output_tokens`, `model_calls`, `tool_calls`, `tool_input_bytes`, `tool_output_bytes`, `context_bytes`, `events`, `trace_bytes`, `filesystem_work`, `total_tokens` and `cost`.

Failure codes include provider rejection/transport/malformed stream, model-attempt timeout, incompatible continuation, unsupported provider content, context/compaction failure, output validation failure, invalid tool arguments, tool execution/cleanup, child/remote-task failure, event sink I/O and accounting bound violation.

## Delivery certainty

A failed outcome says whether the provider or remote side may have observed the request:

- `not_sent`: Pablo has evidence delivery did not occur.
- `may_have_been_sent`: delivery cannot be proven either way.
- `response_received`: the peer responded or streaming began.

Use this field when deciding whether a host may safely retry. Do not automatically replay uncertain tool or model work.

## Accounting

Every admitted result includes:

- model and tool call counts
- reported input/output/cache token totals
- normalized reported cost where available
- conservative charged tokens/cost when hard limits are supported

Missing usage from any contributing delivered call makes that aggregate unknown. A definitely unsent attempt charges a known zero while retaining its call count. An uncertain delivery keeps its reservation when hard accounting is active.

The machine envelope represents u64 values as decimal strings. The completed outcome’s provider-reported usage is numeric for compatibility. JavaScript clients should use `BigInt` for the envelope counters.

## CLI exit codes

| Code | Meaning |
| ---: | --- |
| `0` | Task completed or offline inspection succeeded |
| `1` | Admitted task ended non-completed, or an inspection found a runtime issue |
| `2` | Invalid arguments/configuration or other pre-admission setup error |
| `130` | User cancellation completed |

`doctor` has its own more specific diagnostic codes for credential, provider, MCP and policy failures. JSON task framing remains unchanged.

## Platform scope

Pablo targets macOS and Linux. Shell process-group cleanup has platform-specific implementations for those systems. The prerelease plan requires native acceptance on:

- macOS arm64
- macOS x86_64
- Linux x86_64
- Linux arm64

Cross-compilation can prepare an artifact but cannot satisfy a native execution gate. Release archives and the full native matrix remain pending.

## Version spaces

Several independent versions appear in integrations:

- Cargo/executable version: currently `0.1.0-dev.1`
- native event/task contract revision: currently `c3.33`
- deployment document schema: `1`
- deployment contract revision: advances with implemented option semantics
- ACP protocol: stable v1 plus `pablo/v1` or `pablo/v2` negotiated metadata
- OpenTelemetry mapping and pinned GenAI semantic-convention revision

Do not substitute one version for another. Negotiate or validate the contract used by your integration.
