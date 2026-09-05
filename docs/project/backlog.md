# Deferred work

This is an inventory of future slices, not a second task-status system. Promote work into a new cycle specification and the state file only when its entry condition is met. [The design brief](../context.md), section 29.1, remains authoritative for the full 0.1 release.

| Slice | Capability | Promotion condition |
| --- | --- | --- |
| Alpha.1 completion | CLI task/JSON surface, filesystem read/write/edit/list/search, full single-agent static policy and accounting limits | C1's ACP/shell/provider/telemetry spike is verified; plan the remaining alpha.1 gaps explicitly. |
| Extensibility | OpenRouter, native Open Responses, stdio and Streamable HTTP MCP tools, local Agent Skills, structured-output validation/repair, two temporary depth-one children | The bounded single-agent lifecycle and contracts have real integration evidence. |
| Interoperability | Minimal A2A client delegation, basic TUI, Otto bounded read-only acceptance | Extensibility fixtures pass and root/child policy, budgets, events, and outcomes share one lifecycle. |
| Release hardening | macOS/Linux binaries, protocol fixtures, pinned compatibility revisions, measurements, distribution | Focused section 31.1 acceptance gates pass; publish only capabilities actually implemented. |
| Durable product work | Resumable root sessions, persistent children, storage/recovery, broader MCP, Python helpers, Graphline | The 0.1 cut line is satisfied and a new cycle is justified by host needs. |
| Optional breadth | AG-UI, Vercel AI SDK streams, OpenAPI import, advanced graphs/replay, cache policy, additional OTel signals | Native contracts survive dogfooding; each add-on or extension has its own small acceptance path. |

Windows distribution, a general graph scheduler, interactive PTYs, provider fallback orchestration, and automatic skill installation are outside C1. Preserve their rationale in the brief instead of scaffolding placeholder implementations.
