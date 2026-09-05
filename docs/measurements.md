# Pablo measurement method

Run the release measurement harness after acceptance tests and builds have finished:

```sh
npm run measure -- 30
```

The optional second argument selects the local JSON result path. By default, it writes `.pablo/measurements/c1.7-<platform>-<arch>.json`. Set `PABLO_MEASURE_ENVIRONMENT` to identify a VM or other runner. The harness uses only local synthetic workloads, removes inherited provider/exporter settings, never loads `.env`, and removes its temporary workspace. There are no guessed performance ceilings.

The Cargo example `crates/pablo/examples/measure.rs` is a measurement host, not a shipped CLI command. It calls the actual core and reuses the CLI's option/spec construction. No new runtime lifecycle, dependencies or product behavior are introduced.

## Workloads and timing boundaries

- **Startup:** Node's monotonic clock measures spawn to the first request arriving at a loopback HTTP provider. A separate `--version` measurement covers process start/exit without runtime setup. Both use warm filesystem caches; neither claims a forced-cache cold start or remote HTTPS latency.
- **Core versus ACP:** Every sample runs the same two HTTP/SSE model calls and one real `printf` shell call, followed by 32 output deltas of 16 bytes. The harness asserts the actual shell result, final output, exactly two model requests and clean process exit. Direct-core timing covers `run_with_tools`, with provider, tool registry and SDK construction outside that timer. ACP prompt timing includes per-prompt setup, the worker boundary, streaming and final delivery through the official TypeScript client. Consequently the ratio includes setup costs and is not pure JSON/stdio cost. Separate host-process totals expose startup; ACP totals exclude the deliberately inserted RSS wait and session creation.
- **First text delivery:** A separate fixture writes one text delta, waits 40 ms, then finishes the model response. Measure from that server write to the official TypeScript client update. This isolates initial delivery from terminal flushing; it includes local HTTP/SSE and ACP work.
- **Reused ACP tasks:** Run up to 40 measured independent sessions per process, after five warm-up tasks in that process. The same model/shell/model workload exercises the shared worker, HTTP pool, tool registry and SDK. Measure prompt-to-outcome and first text separately; session creation and RSS sampling are outside the timers. Batches respect the 128-message connection bound. Also sample idle RSS after each measured warm task.
- **Idle RSS:** `ps -o rss=` samples KiB 50 ms after ACP `initialize`, before session/prompt or provider creation. Each sample uses a new process. This is resident memory, not peak RSS, private dirty memory or a steady-state leak test.
- **Event latency:** A fixture provider stamps each normalized text event immediately before yielding it. The inline native sink records elapsed time on entry. The measured path includes fixture clock/mutex overhead, runtime processing and event publication; it excludes network and SSE parsing. Each run yields 1,000 chunks of 32 bytes. The reported percentile population contains all measured deltas from untraced runs.
- **Native trace overhead:** Alternate paired runs of that same event workload with no JSONL and with metadata-only JSONL to a buffered temporary file. Timing includes serialization, writes and flush, but excludes file creation and does not include `fsync`. Both paths retain native OTel instrumentation with no network exporter. Compare absolute time as well as percentage: a CPU-only stream has no model wait to dilute trace costs.
- **Binary size:** Record the actual default-feature release executable and a copy processed by the platform's `strip` utility. Timings use the selected release build (the release profile recorded in each report). The post-strip copy is measured for size and is not installed or published.

Five warm-up samples/pairs are excluded. The default is 30 measured samples, with core/CLI/ACP order rotated and trace pairs alternated. Percentiles use sorted nearest rank; p99 over 30 process samples is the maximum and should not be interpreted as a stable tail estimate. A ten-sample pilot validates the harness but is excluded from the acceptance baseline. Run platform measurements sequentially, without concurrent builds/tests.

The local report includes platform/kernel/CPU/memory, Node/Rust versions, environment description, build hash, source fingerprint, sample counts, methods, individual run timings and aggregate percentiles. Its source fingerprint covers `crates`, `examples`, `scripts`, `tests`, manifests, lockfiles and toolchain/configuration pins. Changing handoff records does not invalidate it. For build comparisons, `PABLO_MEASURE_BINARY` and `PABLO_MEASURE_DIRECT` select saved executables, `PABLO_MEASURE_BUILD` describes their actual profile, and `PABLO_MEASURE_SOURCE_SHA256` identifies their saved source snapshot. Set `PABLO_MEASURE_REUSE=0` only for the C1.6 baseline, whose process cannot accept successive tasks; the report explicitly omits warm-task measurements. Keep the implementation and harness fixed across profile comparisons, and finish builds/tests before measuring. Checked-in cycle evidence keeps sanitized aggregate results and the fingerprint; raw measurements stay ignored.

## Interpretation and scope

C1 requires baseline measurements, not satisfaction of every provisional budget in the broader design brief. Eight-agent concurrency, TUI/A2A/children, real model/TLS/network delay and the full release target matrix are outside this harness. A virtual Linux arm64 result proves that platform's behavior and provides a VM-specific baseline; it does not substitute for the brief's native Linux x86_64 release measurements.

See the [C1 acceptance report](project/evidence/c1.6.md) for the original platform acceptance and the [C1.7 performance report](project/evidence/c1.7.md) for optimization results, profile comparisons and remaining limits.
