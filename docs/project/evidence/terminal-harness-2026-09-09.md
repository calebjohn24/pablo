# Existing harness build and terminal handoff

The user requested a build and test of the existing harness using available provider credentials. This session verifies the existing Vercel adapter; OpenRouter is not implemented or tested. No runtime changes were needed.

## Environment and build

- Source commit: `5367dacdaf81919ac1b0425a6b9d371743716d4d`; initially clean working tree.
- Native macOS arm64; Rust 1.98.1; installed Node 24.11.0. Node satisfies package.json's supported major version but differs from the `.nvmrc` pin of 24.20.0.
- `npm ci` installed the locked dependencies successfully.
- `cargo build --release --locked -p pablo` passed. Executable: `target/release/pablo`, 10,464,624 bytes; SHA-256 `9cb6d1f2c04e8f2906bf05ab80638fe4798e565cefa89249c960840aef1b245c`.
- Release `--version` and offline `demo` passed.

## Verification

- Formatting and Clippy with warnings denied passed.
- `cargo test --workspace --locked`: 93 tests passed, none failed or ignored.
- Project helper suite: 34 tests passed.
- TypeScript type checking passed.
- `node --test tests/acp.test.ts tests/telemetry.test.ts`: 84 tests passed, none failed or skipped, covering protocol compatibility, tool execution, cancellation, backpressure, policy, accounting and telemetry.
- An interim project check rejected `in_progress` with retained blockers. Restoring the checkpoint to `blocked` resolved the record inconsistency; project consistency and diff whitespace checks then passed.

Two explicit live smokes used the release executable and the existing `google/gemini-3.8-flash` default through Vercel. Only the executable privately parsed credentials; no credential file was sourced or printed.

| Fixture | Result | Elapsed | Model calls | Tool calls | Input/output tokens |
| --- | --- | --- | --- | --- | --- |
| `node scripts/smoke-live.mjs target/release/pablo` | Passed | 4.12 s | 2 | 1 shell | 925 / 123 |
| `node scripts/smoke-live-acp.ts target/release/pablo --filesystem` | Passed | 3.52 s | 2 | 1 fs.read | 658 / 185 |

Both returned exact unseen temporary file evidence and one completed outcome. The ACP fixture additionally verified streamed output, native correlation, usage totals, private redacted traces, clean process exit and workspace cleanup. Traces and machine summaries remain in ignored local storage. These timings describe individual smoke runs, not performance baselines.

## Terminal handoff and remaining acceptance

```sh
cd /Users/caleb/Projects/pablo
./target/release/pablo run "Read README.md and summarize what Pablo can do." --no-shell
```

The executable loads the existing root `.env` privately. Replace the task text to try another task. Each invocation is independent; Ctrl-C cancels. The command enables filesystem reads and disables shell execution.

C2.5 remains blocked on the existing native Linux x86_64 runner gate. No new checkpoint is selected. Resume full C2.5 acceptance and comparable release measurements when that runner is available; this terminal handoff does not close the cycle. The real Collector proof and release benchmark suite were not rerun in this session.
