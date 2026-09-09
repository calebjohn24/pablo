# Pablo versus Codex: local overhead

Measured 2026-09-05T10:27:57.134Z on Apple M4, native macOS arm64 (Darwin 25.6.0), Node v24.20.0. This user-requested comparison supplements C2.5; it does not clear the native Linux x86_64 acceptance blocker. No Pablo runtime source or binary changed during this comparison.

## Method

Run `node scripts/compare-codex.mjs 30` after building Pablo in release mode. Override the installed Codex path with `PABLO_COMPARE_CODEX` when needed; the default is `/opt/homebrew/bin/codex`. The optional second argument selects the output JSON path. The [aggregate report](codex-comparison.json) retains exact versions, binary and harness SHA-256, platform, sample counts and percentiles. Individual timing arrays remain in ignored local output.

Each metric has 30 samples after five warm-ups. Fresh-process samples alternate product order. Warm samples run in one reused process per product, Pablo then Codex, creating a fresh independent session/thread per sample. Filesystem caches are warm, with no cache eviction; the temporary Codex home is shared within the run. The arm64 VM was stopped, with no concurrent builds or test suites during timings.

Both products receive the same short task in an empty temporary workspace and perform exactly one loopback HTTP request returning fixed text. The fixture asserts the final output, request count and clean process exits. It uses each product's supported transport: Pablo Chat Completions SSE and Codex Responses SSE. Model names are routing selectors only; there is no model inference, paid call, tool execution or coding task. The CLI timer includes process lifetime. Provider-ready means receipt of the HTTP request body. The server initialization timer ends at the initialize reply, before any task, and is not a complete readiness test for every subsystem. Reused-task completion is event driven without polling.

Codex runs with an isolated temporary home, no user credentials/configuration/plugins/MCP, a custom localhost provider, read-only sandbox configuration and ephemeral threads. Pablo runs with its default catalog and a local fixture endpoint. The child environment is explicitly constructed without provider credentials; the repository credential file is not accessed. The products retain their different built-in instructions and tool catalogs. Their different payloads and protocol semantics are part of this observation, not an isolated implementation-language comparison.

## Results

Installed main executables: Pablo **9.980 MiB**, Codex CLI **210.366 MiB**. Versions: Pablo 0.1.0-dev.1 at the C2 release binary hash recorded in C2.5; Codex CLI 0.153.3. This excludes Codex's additional bundled helper executables and resources, and is not total installation size.

All timing cells below are **p50 / p95 in milliseconds**. These are observations on one machine, not confidence intervals or universal performance ceilings.

| Metric | Pablo | Codex CLI 0.153.3 |
| --- | ---: | ---: |
| Version command process lifetime | 4.733 / 5.061 | 8.134 / 11.626 |
| Fresh CLI spawn → provider request | 7.306 / 7.842 | 29.975 / 35.053 |
| Fresh CLI whole task | 7.926 / 8.448 | 36.447 / 41.430 |
| Server spawn → initialize reply | 3.889 / 4.262 | 15.044 / 19.310 |
| Reused server turn, task creation excluded | 0.246 / 0.307 | 16.740 / 172.587 |
| Reused server task including fresh session/thread | 0.300 / 0.366 | 31.048 / 255.437 |

Idle server root-process RSS, sampled 50 ms after initialize: **7.953 / 7.984 MiB** p50/p95 for Pablo; **49.625 / 49.859 MiB** for Codex. This excludes helper processes and measures neither peak memory nor memory after a long conversation.

Fresh CLI request bodies contain 3,352 bytes for Pablo and 28,759 for Codex; reused-server bodies contain 3,352 and 28,732 bytes respectively. Byte counts are not token usage, model price or evidence of better prompt quality.

The final run confirms a roughly 21× smaller main executable, 4.6× lower fresh CLI time and 6.2× lower idle root RSS for Pablo on this fixture. An earlier 30-sample pass had fresh CLI medians of 8.021 / 35.626 ms and the same idle RSS medians. Codex reused-task timings varied substantially: the final p95 is 255.437 ms versus 50.503 ms in that earlier pass. The cause was not isolated. Codex threads remain loaded until process exit; this is not a matched history-cleanup benchmark. Do not turn the warm-task ratio into a general coding-speed claim.

The C2.5 versus C1.7 benchmark uses two model requests and a real shell call. Its absolute durations cannot be compared directly with this one-request/no-tool workload.

## Capability interpretation

Pablo's implemented slice is a small headless sequential runtime: bounded shell/filesystem operations, task JSON, ACP integration, cancellation, static host policy, accounting and native/OTel telemetry. Hosts own sandbox isolation, approvals, business state and durable history. Live hard token/cost ceilings remain unsupported without an adapter that can attest enforceable bounds. See [C2 acceptance](c2.5.md) and [project decisions](../brain.md).

Codex provides a broader coding-agent product. Its non-interactive CLI supports sandbox modes, resuming work and model output constrained by `--output-schema`; the App Server supports application integration with authentication, conversation history, approvals and streamed events. These features explain why product scope matters, but this test does not isolate their individual performance costs. Sources checked on 2026-09-05: [official non-interactive documentation](https://learn.chatgpt.com/docs/non-interactive-mode), [official App Server documentation](https://learn.chatgpt.com/docs/app-server), and [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference), alongside the installed CLI's version/help output.

Pablo's structured task envelope records runtime outcomes; it does not establish equivalence to Codex's model-output JSON Schema constraint. ACP and Codex App Server are different integration protocols. No feature, sandbox-security or coding-quality parity is claimed. A coding comparison still requires representative repository tasks, matched actual model/settings, isolated worktrees, test-based success criteria, wall time, tokens/cost and repeated trials.

## Verification and handoff

The final 30-sample fixture comparison, `node --check scripts/compare-codex.mjs`, `npm run typecheck`, `git diff --check`, and `node scripts/project.mjs check` passed. The helper includes a bounded wait for the terminal Codex notification. No runtime tests were rerun for this additional benchmark-only change. C2.5 remains blocked solely on the already recorded native Linux x86_64 gate; no next implementation checkpoint is selected.
