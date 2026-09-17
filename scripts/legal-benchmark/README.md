# Local release benchmark with Jev

This integration runs the existing `~/Projects/legal-benchmark` catalog, process monitor and deterministic scorer without modifying that project. It adds each task's `task.md` and all four `sources/*.txt` documents to the original initial prompt so Jev sees the actual rules and facts before selecting a model. The selected model receives the same context; `key.json` is never included.

The checked-in deployment maps small to `zai/glm-5.3-flash`, medium to `spacexai/grok-4.6`, and large to `openai/gpt-6-astra`, all through Vercel AI Gateway with provider-default reasoning. Jev chooses freely using the configured descriptions. The test does not force an even distribution or guarantee every tier is selected.

```sh
cargo build --release --locked -p pablo --bin pablo
python3 scripts/legal-benchmark/run-jev.py \
  --output ~/Projects/legal-benchmark/results/local-jev-full-r1
```

Use `--tasks notice-cure-01` for a one-task smoke. `--benchmark`, `--binary`, `--config` and `--timeout` override paths or the default 300-second per-task deadline. Output directories must be new. The suite runs serially, once, with seed 42, fresh processes/workspaces, and no retries. Each task allows at most nine model calls: one classification plus the benchmark's eight generation calls. Filesystem read/write is enabled; shell is disabled; the tool-call limit is 80.

The runner freezes the configuration before the first task. Jev has a 30-second classification deadline within the overall task deadline. The prompt distinguishes direct operations, bounded rule chains and reasoning through conflicting interpretations; legal terminology, file counts and output formatting alone do not raise the tier.

`--source-context full` is the default. `--source-context brief` omits source documents for a comparison. `--context-max-bytes` can lower the 48,000-byte context limit. All selected inputs are validated before live calls: oversized context is rejected without truncating controlling rules, and source directories may contain only regular `.txt` files, with no symlinks. The current catalog's complete contexts are 1,681–2,442 bytes. Routing logs record filenames and context size, never document contents. Inline sources can reduce subsequent file reads, so timing, token use and scores are not directly comparable with runs that supplied only a brief.

To check obvious routing cases independently of legal-task scores:

```sh
python3 scripts/legal-benchmark/probe-jev.py \
  --output ~/Projects/legal-benchmark/results/jev-probes \
  --repeats 2
```

This is an explicit live test with nine fixed cases per repetition, three for each tier. It allows exactly one model call, so only Jev runs; the expected terminal result is the model-call limit after selection. The trace's selected route is checked against the expected tier and exact destination model. These probes test classification and mapping, not destination-model quality or multi-turn pinning. Provider failures are retained and make the probe command fail; it never retries silently. Inspect `routing.jsonl` and `report.json` for each outcome.

Only the executable privately reads the Pablo root `.env`, through the config's `secrets` binding and `AI_GATEWAY_API_KEY` entry. The Python runner does not load credentials or forward ambient provider/telemetry settings.

Outputs include:

- `routing.jsonl`: one record per task, appended immediately after the attempt, with selected tier/model, the full model-call sequence, usage, timings and pinning verification.
- `routing.csv`: a compact per-task table with model, completion, factual/citation score and latency.
- `report.md`: benchmark totals and the per-task model-selection table.
- `report.json`: full results, selected-model counts, binary/config/runner fingerprints and measurement boundaries.
- `attempts/*/raw/pablo-trace.jsonl`: redacted native events recording each dispatched model during the task.

Failed attempts remain visible and contribute zero to headline scores. Any partially produced artifact score is retained separately. Actual model cost stays unknown when Vercel does not report it; aggregate tokens from different models are never priced as if they came from one model. A single local run is functional/quality evidence, not a controlled comparison against earlier hosted or fixed-model runs. Cache hits remain upstream-dependent even when model pinning passes.
