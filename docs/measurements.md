# C1 measurement method

Run the release measurement harness after acceptance tests and builds have finished:

```sh
npm run measure -- 30
```

The optional second argument selects the local JSON result path. By default, it writes `.pablo/measurements/c1.6-<platform>-<arch>.json`. Set `PABLO_MEASURE_ENVIRONMENT` to identify a VM or other runner. The harness uses only local synthetic workloads, removes inherited provider/exporter settings, never loads `.env`, and removes its temporary workspace. There are no guessed performance ceilings.

The Cargo example `crates/pablo/examples/measure.rs` is a measurement host, not a shipped CLI command. It calls the actual core and reuses the CLI's option/spec construction. No new runtime lifecycle, dependencies or product behavior are introduced.

## Workloads and timing boundaries

- **Startup:** Node's monotonic clock measures spawn to the first request arriving at a loopback HTTP provider. A separate `--version` measurement covers process start/exit without runtime setup. Both use warm filesystem caches; neither claims a forced-cache cold start or remote HTTPS latency.
- **Core versus ACP:** Every sample runs the same two HTTP/SSE model calls and one real `printf` shell call, followed by 32 output deltas of 16 bytes. The harness asserts the actual shell result, final output, exactly two model requests and clean process exit. Direct-core timing covers `run_with_tools`, with provider, tool registry and SDK construction outside that timer. ACP prompt timing includes per-prompt setup, the worker boundary, streaming and final delivery through the official TypeScript client. Consequently the ratio includes setup costs and is not pure JSON/stdio cost. Separate host-process totals expose startup; ACP totals exclude the deliberately inserted RSS wait and session creation.
- **Idle RSS:** `ps -o rss=` samples KiB 50 ms after ACP `initialize`, before session/prompt or provider creation. Each sample uses a new process. This is resident memory, not peak RSS, private dirty memory or a steady-state leak test.
- **Event latency:** A fixture provider stamps each normalized text event immediately before yielding it. The inline native sink records elapsed time on entry. The measured path includes fixture clock/mutex overhead, runtime processing and event publication; it excludes network and SSE parsing. Each run yields 1,000 chunks of 32 bytes. The reported percentile population contains all measured deltas from untraced runs.
- **Native trace overhead:** Alternate paired runs of that same event workload with no JSONL and with metadata-only JSONL to a buffered temporary file. Timing includes serialization, writes and flush, but excludes file creation and does not include `fsync`. Both paths retain native OTel instrumentation with no network exporter. Compare absolute time as well as percentage: a CPU-only stream has no model wait to dilute trace costs.
- **Binary size:** Record the actual default-feature release executable and a copy processed by the platform's `strip` utility. Timings use the original release build (`thin` LTO, `strip=debuginfo`). The post-strip copy is measured for size and is not installed or published.

Five warm-up samples/pairs are excluded. The default is 30 measured samples, with core/CLI/ACP order rotated and trace pairs alternated. Percentiles use sorted nearest rank; p99 over 30 process samples is the maximum and should not be interpreted as a stable tail estimate. A ten-sample pilot validates the harness but is excluded from the acceptance baseline. Run platform measurements sequentially, without concurrent builds/tests.

The local report includes platform/kernel/CPU/memory, Node/Rust versions, environment description, build hash, source fingerprint, sample counts, methods, individual run timings and aggregate percentiles. Its source fingerprint covers `crates`, `examples`, `scripts`, `tests`, manifests, lockfiles and toolchain/configuration pins. Changing handoff records does not invalidate it. Checked-in cycle evidence keeps sanitized aggregate results and the fingerprint; raw measurements stay ignored.

## Interpretation and scope

C1 requires baseline measurements, not satisfaction of every provisional budget in the broader design brief. Warm-process/session reuse, eight-agent concurrency, TUI/A2A/children, real model/TLS/network delay and the full release target matrix are outside this harness. A virtual Linux arm64 result proves that platform's behavior and provides a VM-specific baseline; it does not substitute for the brief's native Linux x86_64 release measurements.

See the [C1 acceptance report](project/evidence/c1.6.md) for the actual platform matrix, results, limitations and proposed next cycle.
