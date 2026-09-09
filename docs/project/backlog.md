# Deferred work

This is an inventory of future slices, not a second task-status system. Promote work into a new cycle specification and the state file only when its entry condition is met. [The design brief](../context.md), section 29.1, remains authoritative for the full 0.1 release.

| Slice | Capability | Promotion condition |
| --- | --- | --- |
| Extensibility | OpenRouter, native Open Responses, stdio and Streamable HTTP MCP tools, local Agent Skills, structured-output validation/repair, two temporary depth-one children | The bounded single-agent lifecycle and contracts have real integration evidence. |
| Interoperability | Minimal A2A client delegation, basic TUI, Otto bounded read-only acceptance | Extensibility fixtures pass and root/child policy, budgets, events, and outcomes share one lifecycle. |
| Release hardening | macOS/Linux binaries, protocol fixtures, pinned compatibility revisions, project-controlled ACP extension naming/schema review, measurements, distribution | Focused section 31.1 acceptance gates pass; publish only capabilities actually implemented. |
| Durable product work | Resumable root sessions, persistent children, storage/recovery, broader MCP, Python helpers, Graphline | The 0.1 cut line is satisfied and a new cycle is justified by host needs. |
| Optional breadth | AG-UI, Vercel AI SDK streams, OpenAPI import, advanced graphs/replay, cache policy, additional OTel signals | Native contracts survive dogfooding; each add-on or extension has its own small acceptance path. |
| Provider ceiling profiles | Evidence-backed live per-call token/cost upper bounds for hard aggregate budget admission | A provider/profile exposes enforceable bounds; C2.4 proves the ledger with attested offline fixtures and rejects unsupported live ceiling configurations. Reported usage alone is insufficient. |
| Policy recovery and breadth | Model continuation after policy denial, shell-command matching, broader environment forwarding and process/network controls | C2's exact tool/launcher/root rules have integration evidence and a host needs broader behavior; preserve the distinction between policy and containment. C2 retains terminal policy denial. |

The former alpha.1 completion slice is complete in [cycle C2](cycles/002-single-agent-completion.md): filesystem operations, task/JSON output and the selected single-agent policy/accounting contracts. C2.5 acceptance now passes on macOS arm64 and [native Linux x86_64](evidence/c2.5-linux-x64.md), including matched release measurements. This does not claim alpha.1 release completion. General model structured-output validation/repair stays in extensibility; a typed task envelope does not satisfy that gate. The ACP compatibility review retains development names and negotiated fallback; final release namespace ownership/stability, doctor, packaging and publishing remain release hardening. Extensibility is eligible for a new cycle specification; no next checkpoint has been selected.

Release sizing remains an observation to address before ratifying distribution budgets: the Linux x86_64 C2 executable is 12.150 MiB versus 11.886 MiB for C1.7, both above the provisional 10 MiB target. Hosted-runner timings are mixed and do not establish universal performance equivalence. Preserve the [measured methods and limits](evidence/c2.5-linux-x64.md#matched-release-observations); no provisional number is a CI failure threshold.

Windows distribution, a general graph scheduler, interactive PTYs, provider fallback orchestration, and automatic skill installation are outside C1. Preserve their rationale in the brief instead of scaffolding placeholder implementations.
