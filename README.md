# pablo

A small, headless Rust agent runtime for applications doing arbitrary work.

The focused [C1 spike](docs/project/cycles/001-first-spike.md) and [C2 cycle](docs/project/cycles/002-single-agent-completion.md) are complete. C2 implements bounded filesystem tools, machine-readable task output, static policy and accounting, with acceptance on macOS arm64 and [native Linux x86_64](docs/project/evidence/c2.5-linux-x64.md). This is not a published release. The [architecture brief](docs/context.md) describes the larger product; section 29.1 defines the 0.1 release contract. The project-state command reports checkpoint progress across retained cycles.

The selected [C3 plan](docs/project/cycles/003-extensibility-and-release.md) covers declarative deployment configuration, ordered model fallback, shell command rules, providers, MCP, Skills, structured output, temporary children, A2A, a basic TUI and release delivery in small checkpoints. Otto integration remains deferred. These are planned capabilities; the commands below describe the existing C2 runtime.

## Run a real task

From the project directory:

```sh
cargo run --locked -p pablo -- run "Read README.md and summarize what Pablo can do."
```

Pablo loads your gateway key from the ignored `.env`, streams the answer, and shows shell activity in the terminal. Both `AI_GATEWAY_API_KEY` and your existing `VERCEL_AI_GATEWAY` name work. It uses direct HTTPS through a Rust HTTP client; no Vercel SDK is installed. The default model is `google/gemini-3.8-flash`.

To work in a different folder while keeping credentials in this project:

```sh
cargo run --locked -p pablo -- run "List the files here and explain the project." --workspace /path/to/project
```

Press **Ctrl-C** to cancel and wait for shell cleanup. Each invocation is a fresh task. Tool and model call counts are unlimited by default; the default deadline is one hour per run and 15 minutes per shell call. Shell runs on your machine with your user permissions; the workspace selects its working directory, not an OS sandbox. Filesystem read/list/search tools are also enabled; `--allow-write` opts in to revision-checked write/edit; use `--no-shell --no-filesystem` for text-only tasks. See [filesystem behavior](docs/filesystem.md). There is no persistent chat or TUI yet.

Set `--max-tool-calls N` or `--max-model-calls N` to opt into call-count limits (zero disables the corresponding calls). Omit these options for unlimited call counts. These options work with both `run` and `acp --stdio`, including through the reference client.

Default capacities are 1 MiB input, 8 MiB per tool result, 32 MiB context, 4 MiB model output, and 65,536 requested output tokens per model call. Optional native traces allow 256 MiB.

Use `--model provider/model` to choose another compatible model, `--timeout SECONDS` or `--tool-timeout SECONDS` to change the run or shell deadline (1–86400 each), or `--env-file PATH` to select a credential file. Environment variables take precedence over file values; credential files are parsed privately without sourcing them or modifying the process environment. `--workspace` does not change where `.env` is loaded from.

For a built executable:

```sh
cargo build --release --locked -p pablo
./target/release/pablo run "Read README.md and summarize it."
```

Shorthand and machine output use the same runtime:

```sh
./target/release/pablo "Summarize README.md" --json --no-shell
```

`--json` emits one [task envelope](docs/pablo-task.schema.json) with the native outcome and exact decimal-string accounting. `--policy PATH` sets host tool, launcher and filesystem-root rules. Optional `--max-total-tokens` and `--max-cost-microusd` require attested provider bounds; the live gateway currently rejects these before delivery. See [runtime contracts](docs/runtime.md).

See [the gateway and CLI contract](docs/gateway.md) for transport details, trace options, and the explicit live smoke check.

## Use ACP from TypeScript

```sh
npm ci
cargo build --locked -p pablo
node examples/acp-client.ts "Read README.md and summarize it." /absolute/workspace
```

The reference client starts `pablo acp --stdio`, negotiates stable ACP v1, streams message/tool updates, and reads the typed outcome. Ctrl-C sends cancellation and waits for cleanup. An ACP host can reuse the process for successive independent sessions, with one prompt per session; provider connections, compiled tools and telemetry setup stay warm. The reference CLI still runs one task and exits. See [the ACP contract](docs/acp.md) for host configuration, protocol bounds, SDK pins and the offline acceptance suite.

## Offline demo and development checks

The Cargo workspace contains `pablo-core` and the `pablo` executable. Rust 1.98.1 is pinned in `rust-toolchain.toml`; Cargo dependencies are pinned in the manifests and `Cargo.lock`.

```sh
cargo run --locked -p pablo -- demo
```

The offline demo streams `Hello from pablo.` through the core. To save a native trace, choose a new file:

```sh
mkdir -p .pablo/traces
cargo run --locked -p pablo -- demo --trace .pablo/traces/demo.jsonl
```

Add `--capture-content` to include the synthetic response in that trace. The default records metadata and byte counts. Trace files are created with private permissions on Unix and existing files are preserved. OTel content stays disabled; network export is off by default.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm run typecheck
npm run test:acp
npm run test:telemetry
cargo build --release --locked -p pablo
```

See [the runtime contracts and telemetry mapping](docs/runtime.md) for embedding and limits. The core supports streamed model/tool turns, bounded shell execution, and cancellation. See [the shell contract](docs/shell.md). The `run` command uses the live gateway; `demo` remains an offline text fixture. The ACP command drives the same runtime, with [verified live Vercel acceptance](docs/project/evidence/c1.4.md). Run `npm run smoke:live:acp` to repeat the explicit paid fixture. CLI and ACP support [OTLP/HTTP Protobuf export and incoming W3C context](docs/telemetry.md). Run `npm run smoke:collector` for the pinned real local Collector proof; it uses an offline model fixture and no provider credential. The [C1 acceptance report](docs/project/evidence/c1.6.md) records macOS/Linux arm64 checks, live Vercel and Collector proof, and release measurements. The focused spike is complete; the full alpha.1/0.1 surface remains future work.

Run the real shell round-trip fixture with:

```sh
cargo test --locked -p pablo-core --test tool_loop actual_shell_evidence_round_trip
```

The release profile uses thin LTO, one codegen unit and stripped symbols. The [C1.7 performance report](docs/project/evidence/c1.7.md) compares trace encoding, first-text delivery, reused tasks and build profiles. Repeat the offline release measurements with `npm run measure -- 30`; see [measurement methods](docs/measurements.md) for timing boundaries and limitations.

## Getting oriented

Use the installed Node version with `nvm use`. In a noninteractive shell, load nvm first if Node is absent from `PATH`:

```sh
export NVM_DIR="$HOME/.nvm"
. "$NVM_DIR/nvm.sh"
nvm use
```

The project helper has no external dependencies and runs without `npm install`:

```sh
node scripts/project.mjs status
node scripts/project.mjs context
node scripts/project.mjs check
node --test scripts/project.test.mjs
```

Equivalent npm commands are `npm run project:status`, `npm run project:context`, `npm run project:check`, and `npm test`.

`status` shows checkpoint progress and the next action. `context` assembles a bounded handoff from the brain, state, selected checkpoint, and three recent log entries. `check` validates record consistency and exits nonzero on errors. All three commands are read-only and resolve paths from the repository, so they also work when invoked from another directory.

Read [AGENTS.md](AGENTS.md) for the one-checkpoint-per-session workflow and the project-record format. Use [the brain](docs/project/brain.md) for accepted decisions and [the backlog](docs/project/backlog.md) for deferred work.

Local credentials belong in the ignored root `.env`. Project helper commands do not load that file. The `run` command privately reads the Vercel credential; `demo` stays offline.
