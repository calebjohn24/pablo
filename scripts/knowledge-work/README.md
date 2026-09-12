# Knowledge-work comparison

This synthetic suite compares **complete model + harness systems** on local-source office work. Pablo, Pi and Ori + Pi use `z-ai/glm-5.3-flash` through OpenRouter. The additional `pablo-astra` entry uses `openai/gpt-6-astra` through OpenRouter. Codex uses `gpt-6-astra`; Claude Code uses `claude-fable-5-1` through their native authentication. Scores across those model families cannot isolate harness quality.

The six tasks cover invoice reconciliation, date-sensitive expense policy, constrained procurement, conflicting meeting handoffs, incident evidence/uncertainty, and critical-path planning. Seeded numeric variants change several tasks; meeting and incident answers include fixed facts, so repeated seeds are not six entirely independent new problems. Each fresh workspace contains only synthetic source documents. The controller keeps answer keys outside it. This prevents accidental exposure, not deliberate access by an unsandboxed agent. No benchmark skill, external search, delegated agents, or prior conversation is requested. Local shell calculations are allowed; these are business tasks, not code-generation tasks.

## Run

Use existing Node 24 and Python 3 on macOS or Linux. Build Pablo **before** benchmarking. Install Pi/Ori separately; never include installation or compilation in measured trials. Local defaults are `.pablo/knowledge-tools/node_modules/.bin/pi` and `.pablo/knowledge-tools/bin/ori`; Codex and Claude are found on PATH.

```sh
# Inspect the exact matrix; this does not invoke models.
node scripts/knowledge-work/run.mjs

# Bounded pilot: one reconciliation task per system configuration, 180 seconds each.
node scripts/knowledge-work/run.mjs --live --tasks reconciliation \
  --timeout 180 --output .pablo/measurements/knowledge-pilot

# Six tasks, three variants, three repeats, six configurations: 324 paid attempts.
node scripts/knowledge-work/run.mjs --live --seeds 41,42,43 --repeats 3 \
  --timeout 300 --output .pablo/measurements/knowledge-full

# Pablo with Astra (same six tasks, OpenRouter native Pablo credentials).
node scripts/knowledge-work/run.mjs --live --harnesses pablo-astra \
  --output .pablo/measurements/knowledge-pablo-astra

# A single system; useful while other providers are not authenticated.
node scripts/knowledge-work/run.mjs --live --harnesses pablo \
  --output .pablo/measurements/knowledge-pablo

node --test --test-concurrency=1 scripts/knowledge-work/benchmark.test.mjs
```

Every output directory must be new. A repository-local lock prevents two copies of this runner from overlapping. Do not run builds, tests or other benchmarks during retained measurements. The lock does not coordinate unrelated programs. Interrupted or incomplete result directories remain available for diagnosis; a force-killed controller can leave a lock, which must only be removed after verifying its processes ended.

By default the runner does **not** load `.env`, source shell profiles or read authentication files. Explicit credential-reuse flags described below allow private key extraction for the user-authorized comparison. Pablo privately loads the repository `.env` through its native `--env-file`. Codex and Claude use their saved login. Direct Pi and Ori resolve their own credentials or an inherited `OPENROUTER_API_KEY`. Configure authentication using each client's documented workflow; never put keys in CLI arguments, benchmark config, or checked-in evidence. A private `.env` supplied for Pablo does not automatically authenticate the others.

Pinned pilot installations: Pi `@earendil-works/pi-coding-agent@0.85.1` (npm `--ignore-scripts`, isolated prefix); Ori `0.14.3+6e62568`, macOS arm64 release SHA-256 `116131f0b0c9c7f2f0b8be5dbac20ef1f8090bc05f43beb26a7c9816caa93782`. Override executable locations with `PABLO_KNOWLEDGE_PABLO_BINARY`, `PABLO_KNOWLEDGE_CODEX_BINARY`, `PABLO_KNOWLEDGE_CLAUDE_BINARY`, `PABLO_KNOWLEDGE_PI_BINARY`, or `PABLO_KNOWLEDGE_ORI_BINARY`. Ori must also find `pi` on PATH. The runner records installed versions and entry hashes; an entry script hash is not a hash of the entire dependency distribution.

## Explicit credential reuse and costs

The user authorized privately reusing Pablo's OpenRouter key for Ori/Pi and a separate OpenAI API key for Codex. These are opt-in flags; dry runs never load credentials.

```sh
node scripts/knowledge-work/run.mjs --live --harnesses ori,pi \
  --reuse-pablo-openrouter --output .pablo/measurements/knowledge-openrouter

node scripts/knowledge-work/run.mjs --live --harnesses codex \
  --codex-api-key-file /absolute/path/to/private.env \
  --output .pablo/measurements/knowledge-codex-api
```

`--reuse-pablo-openrouter` selects inherited `OPENROUTER_API_KEY` first, otherwise privately parses that literal key from the root `.env` (bounded to 64 KiB). It passes only the selected key in the Ori/Pi child environment. `--codex-api-key-file` privately selects `OPENAI_API_KEY` from the requested file and supplies Codex's documented per-invocation `CODEX_API_KEY`; it does not change saved login. Other credential fields are discarded. Neither flag puts secrets in argv, monitor specs, reports or copied auth files. Captured streams mask the selected credential across chunk boundaries; first-event receipt timestamps can include this bounded buffering. Native client credential behavior and any native local caches remain those clients' responsibility.

Headline output uses factual accuracy and costs; strict task passes were removed at the user's request. Exact citation checks remain only as diagnostic details. Cost aggregation preserves missing values as unknown. Pablo and the other clients expose their own reported costs, which can be SDK estimates rather than invoices. Codex does not expose a dollar amount, so API-key runs separately estimate standard text cost from reported input/cache-write/cache-read/output tokens using [official Astra pricing](https://developers.openai.com/api/docs/models/gpt-6-astra), verified 2026-09-12: $10/$12.50/$1/$50 per million respectively. Reasoning tokens are included in output and are not added twice. If aggregate input exceeds 272K, the estimator declines because it cannot resolve per-request long-context surcharges. Standard tier and no hosted-tool charges are explicit assumptions; estimates are not account billing receipts.

To regenerate the graphic, install optional Matplotlib 3.11.2 in a local venv and run `python scripts/knowledge-work/plot.py REPORT.json OUTPUT_PREFIX`. It writes PNG and SVG, showing all requested metrics with no strict-pass column. The primary comparison should use multiple seeds/repetitions before drawing rankings.

## Correctness

The agent writes `answer.json` and a short `report.md`. Each finding has a stable ID, typed value, and source filename list. The scorer checks exact values and the required source set, with unordered lists. It rejects duplicate/missing findings, wrong types, invented sources, extra answer fields, and missing reports. The legacy strict-pass diagnostic requires every factual and citation check, a present report, unchanged sources and successful client completion. Headline correctness uses factual checks; citation results remain available for diagnosis.

Citation scoring checks document selection, not entailment of arbitrary prose. The required sets are deliberately strict; a substantively valid alternative citation set may need rubric review before a new suite version. Extra redundant citations can fail. Prose length is only an artifact-presence check. It is **not** a writing-quality score. Review anonymized reports with this separate 0–4 rubric per dimension:

- Factual fidelity: statements/calculations agree with sources; no invented certainty.
- Task coverage: all requested decisions, exceptions and open questions addressed.
- Decision usefulness: clear recommendation, reasons and concrete next actions.
- Communication: concise, coherent, audience-appropriate writing and usable citations.

Use at least two blinded reviewers for a published prose score and adjudicate disagreements. Keep their scores separate from deterministic correctness. This small synthetic suite does not establish broad professional competence, realistic document-layout skill, web research quality, or long-context compaction quality.

## Measurements and fairness

Each trial starts a fresh process and directory, runs without retries, and is measured serially. Harness order rotates by task/repetition. This balances some order effects; it does not flush provider caches or eliminate network variation. Native system prompts, tool implementations, security enforcement, reasoning settings and provider routing differ. Codex uses medium effort, Claude high, Pi off, Ori none/Pi off, and both Pablo profiles the provider default. These settings are explicit but are not equivalent reasoning budgets. Account policies may still affect native clients.

`report.json` retains per-attempt status, exact argv (prompt referenced separately), requested and available reported model, source fingerprint, versions, correctness, events and resource samples. `report.md` is a compact table. Raw stdout/stderr, receipts, answers and reports stay ignored locally; do not commit them indiscriminately.

- **Wall time:** spawn through process reaping and pipe cleanup. Includes startup, network inference and tools; excludes fixture creation, version checks and grading.
- **First stdout:** first received bytes, possibly metadata. Never label this time-to-first-token.
- **First assistant observed:** streaming text event where exposed. Codex exec emits completed-message events, so its boundary is later. Pablo's buffered task envelope has no equivalent streaming measurement.
- **Memory:** sampled sum of RSS over the process group and discovered descendants, nominally every 100 ms plus `ps` overhead. Shared pages can double count; short-lived peaks can be missed. `rusage_maxrss` is separately named and is not simultaneous tree peak memory.
- **CPU:** POSIX `wait4` user + system time for the root and descendant usage reaped into it. Unjoined descendants can be omitted. One-core utilization is CPU seconds / wall seconds × 100 and can exceed 100%. Machine utilization divides by logical CPU count.
- **Usage/cost/tool calls:** only provider/client-reported values, preserving unknowns as null. Token fields and cache semantics vary by provider. Subscriptions' reported API-equivalent cost need not be the user's bill.

The supervisor is outside the measured tree. Its `ps` sampling still perturbs the machine. The owned group and observed descendants are killed on timeout and root exit, including observed separate groups. Detached descendants that escape observation can still evade containment. This is a trusted synthetic workload runner, not an OS sandbox or an adversarial resource-accounting guarantee. No local measurement can see remote inference CPU, GPU or RAM.

Failures and unavailable clients stay in the correctness denominator; their fast exit is not a speed win. Inspect statuses and completed-task latency when comparing speed; the JSON also provides passed-only latency so incorrect answers cannot silently become speed wins. Do not rank systems from one task or one repetition. Report per-family scores, failure counts and sample sizes; small-sample p95 is descriptive, not a robust tail estimate.

## Existing benchmarks and primary sources

[WorkArena](https://github.com/ServiceNow/WorkArena) targets browser-based ServiceNow knowledge work and requires its browser/application environment. It is useful for a later shared-browser track, but would add tool/environment differences to this first terminal comparison. [OfficeBench](https://github.com/zlwang-cs/OfficeBench) studies cross-application office work and is another candidate once all clients have the same application tools. This suite is original synthetic material, not a reproduction of either benchmark, and its scores must not be labeled WorkArena/OfficeBench scores.

[Ori Harness](https://openrouter.ai/docs/guides/ori/harness) wraps an existing agent; the default entry here is **Ori + Pi**, separate from direct Pi. [Pi CLI](https://pi.dev/docs/latest/usage) and [JSON events](https://pi.dev/docs/latest/json) describe its headless interface. [Codex noninteractive mode](https://learn.chatgpt.com/docs/non-interactive-mode) describes exec, isolation flags and JSONL events; [models](https://learn.chatgpt.com/docs/models) documents Astra. [Anthropic model IDs](https://platform.claude.com/docs/en/models/overview) specifies `claude-fable-5-1`. CLI flags were also checked against the installed clients.
## Reasoning and transport follow-up

The model-independent controls and diagnostics are described in the [reasoning contract](../../docs/project/contracts/c3-reasoning-latency.md). The historical six-harness pilot stays separate from this repeated comparison.

Build and finish test suites first, then run the serial matrix:

```sh
cargo build --release --locked -p pablo --bin pablo
node scripts/knowledge-work/run.mjs --latency-matrix \
  --harnesses pablo,pablo-astra,pi --seeds 41,42,43 --repeats 3 \
  --reuse-pablo-openrouter --live --output .pablo/measurements/latency-reasoning
```

This schedules 270 attempts. Pablo GLM and Astra each run with `provider_default` and `low`; Pi GLM uses `--thinking low`. Configuration order rotates for every task and repetition. Failure rows remain, and the runner refuses to overwrite results. The native models, prompts, tools and output allowances are unchanged. Reasoning is an explicit profile option, not a model-name heuristic. Requested effort does not attest the backend's actual effort.

Pablo rows retain bounded model diagnostics plus tool, model, runtime-other and process-other durations. Model phase offsets are monotonic; tool/root interval subtraction uses native event timestamps and assumes these nondelegating tasks run serially. First body data can be a heartbeat or private reasoning frame. Header and first-data waits include provider processing and are not pure network measurements. Unknown reasoning tokens, costs and phases stay unknown.

The analysis reports factual accuracy independently of strict legacy scorer passes. A candidate default requires matched attempts, at least 20% lower p50, no worse p95, factual accuracy overall/per family, completion or cost, then report-detail review. The script never auto-promotes defaults:

```sh
node scripts/knowledge-work/analyze-latency.mjs \
  .pablo/measurements/latency-reasoning/report.json \
  docs/project/evidence/c3.33c-latency-measurements.json
.pablo/knowledge-plot-venv/bin/python scripts/knowledge-work/plot-latency.py \
  docs/project/evidence/c3.33c-latency-measurements.json \
  docs/project/evidence/c3.33c-latency
```

The separate transport experiment uses two prebuilt binaries at `.pablo/latency/pablo-h1` and `pablo-h2`; build the latter with `cargo build --release --locked -p pablo --bin pablo --features reqwest/http2` and copy it before rebuilding the ordinary default. `node scripts/knowledge-work/transport.mjs --live` retains 24 alternating single-call observations. Its small sample does not establish a reliable tail-latency improvement. The shipped HTTP feature set remains unchanged.
