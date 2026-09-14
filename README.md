# pablo

A small Rust agent runtime for applications doing non-coding knowledge work.

Pablo gives your application a model-and-tool loop with streaming output, cancellation, policy enforcement and tracing. Run tasks from the CLI, use the interactive terminal, connect an editor through the Agent Client Protocol (ACP), or embed `pablo-core` in a Rust application. Each interface uses the same runtime; your host owns the sandbox, approvals and application state.

**Status:** prerelease development (`0.1.0-dev.1`). Build from source today. Release archives, installation tooling and the complete native platform acceptance matrix are still pending; this is not a published 0.1 release.

## Documentation

Read the [public documentation](https://runpablo.pages.dev), or start in the repository with the [introduction](docs/guide/introduction.md) and [getting-started guide](docs/guide/getting-started.md). The same Markdown powers both views, so examples remain reviewable in GitHub.

- Use Pablo through the [CLI and TUI](docs/guide/cli.md), [ACP](docs/guide/acp.md), or [Rust embedding](docs/guide/embedding.md).
- Configure [deployments](docs/guide/configuration.md), [providers and model routes](docs/guide/providers.md), and [tools and policy](docs/guide/tools-and-policy.md).
- Add [MCP, Skills and supervised agents](docs/guide/extensibility.md), [structured output](docs/guide/structured-output.md), and [observability](docs/guide/observability.md).
- Consult the [resource benchmark](docs/guide/benchmarks.md), [limits and outcomes](docs/guide/reference.md), [troubleshooting](docs/guide/troubleshooting.md), and the current [release status](docs/guide/release-status.md).

## What it does

- **Work with files and tools:** read, list and search a workspace, opt into revision-checked file writes and edits, and execute shell commands with deadlines and joined cancellation cleanup.
- **Connect models:** Vercel AI Gateway, OpenRouter and explicitly configured Open Responses endpoints, with named model profiles, ordered fallback, context compaction and optional reasoning controls.
- **Extend tasks:** configured MCP tools over stdio or Streamable HTTP, explicit local Skills, supervised local children and remote A2A tasks.
- **Return usable results:** streaming text or a JSON task envelope with outcome and accounting; validate final answers against a supported JSON Schema subset, with configurable single-attempt repair.
- **Operate and inspect:** composable TOML deployments, static tool and filesystem policy, offline diagnostics, native JSONL traces and OpenTelemetry export.

## Build and run

Use the pinned Rust **1.98.1** toolchain and a native build toolchain on macOS or Linux. Node is needed only for the TypeScript client and development tooling.

```sh
git clone https://github.com/calebjohn24/pablo.git
cd pablo
cargo build --release --locked -p pablo --bin pablo
./target/release/pablo demo
```

The offline demo prints `Hello from pablo.` without a provider credential. Commands below run from the checkout and use the built executable directly.

For a real task, supply a gateway credential in your environment or an ignored `.env` in the directory where you invoke Pablo:

```dotenv
AI_GATEWAY_API_KEY=your-vercel-ai-gateway-key
```

Then run:

```sh
./target/release/pablo run "Read README.md and summarize what Pablo can do." --no-shell
```

This sends a task to Vercel AI Gateway. The default model is `zai/glm-5.3-flash`. Filesystem reads remain enabled with `--no-shell`.

For OpenRouter, supply `OPENROUTER_API_KEY` instead and select the provider:

```sh
./target/release/pablo run "Summarize README.md." --provider openrouter --no-shell
```

OpenRouter defaults to `z-ai/glm-5.3-flash`. Use `--model ID` to select another compatible model. Reasoning stays provider-controlled by default; supported models can use `--reasoning-effort low` or `--reasoning-budget-tokens N`. See [providers and credentials](docs/gateway.md) and [reasoning controls](docs/project/contracts/c3-reasoning-latency.md).

Environment credentials take precedence over `.env`; `--env-file PATH` selects a different credential file. `VERCEL_AI_GATEWAY` is also accepted as a Vercel key alias. `--workspace PATH` changes the task workspace, independently of credential-file selection. Deployments using `--config` use explicit credential references instead of automatically reading `.env`.

## Everyday use

```sh
# Work in another directory.
./target/release/pablo run "Explain this project." --workspace /path/to/project --no-shell

# Enable revision-checked filesystem writes and edits.
./target/release/pablo run "Write a summary of README.md to SUMMARY.md." --allow-write --no-shell

# Emit one JSON task envelope for a script or host application.
./target/release/pablo run "Summarize README.md." --json --no-shell

# Run a text-only task with explicit time and call limits.
./target/release/pablo run "Explain how a hash table works." \
  --no-shell --no-filesystem --timeout 60 --max-model-calls 2

# Open the interactive terminal.
./target/release/pablo tui
```

The TUI displays Markdown, tool activity and scrollable history. Page Up/Down or the mouse wheel scroll; Ctrl-End returns to the latest output. Each submitted task has fresh model context, even though earlier output remains visible. Press **Ctrl-C** to cancel active work and wait for cleanup. See [terminal controls](docs/project/contracts/c3-tui.md).

Shell and filesystem reads are enabled by default. Shell commands run with your user permissions and can modify files independently of `--allow-write`; the workspace is not an OS sandbox. Use `--no-shell` for filesystem-only work, add `--no-filesystem` for text-only work, and configure `--policy PATH` for tool, launcher and filesystem-root rules. Hosts provide isolation and approvals. See [shell execution](docs/shell.md) and [filesystem behavior](docs/filesystem.md).

Runs default to a one-hour deadline and shell calls to 15 minutes. Model and tool call counts are unlimited unless you set `--max-model-calls` or `--max-tool-calls`; zero disables those calls. Payload, context and transport bounds still apply. Hard aggregate token/cost ceilings require attested provider bounds and are currently rejected by the live gateways. [Runtime contracts](docs/runtime.md) describe limits and accounting.

`--json` wraps the answer in a [task envelope](docs/pablo-task.schema.json); the output field remains a string. To require a structured answer, add `--output-schema /path/to/schema.json`. See [supported schemas](docs/project/contracts/c3-output-validation.md) and [optional output repair](docs/project/contracts/c3-output-repair.md).

## Configure a deployment

TOML deployments compose provider credentials, model routes, tool policy, Skills, MCP servers, child tasks, output settings and telemetry. Inspect them locally before running a task:

```sh
./target/release/pablo config validate --config presets/v1/production.toml \
  --bind workspace=. --bind preset=./presets/v1
./target/release/pablo config explain --config presets/v1/production.toml \
  --bind workspace=. --bind preset=./presets/v1
```

These commands inspect configuration without model calls or service startup. The [versioned deployment presets](presets/v1/README.md) include base, production and development examples. Copy the whole bundle and replace its example MCP/A2A service URLs before execution; the bundled URLs are placeholders. The guide covers credentials, rendering, execution and migration.

| Configure | Reference |
| --- | --- |
| Composition, profiles and host policy | [Deployment configuration](docs/project/contracts/c3-deployment-config.md) |
| Ordered model fallback | [Model routes](docs/project/contracts/c3-model-routes.md) |
| Context compaction | [Compaction](docs/project/contracts/c3-compaction.md) |
| Open Responses endpoint and capabilities | [Provider example](docs/gateway.md#open-responses) |
| MCP tools and explicit local Skills | [MCP](docs/project/contracts/c3-mcp.md), [Skills](docs/project/contracts/c3-skills.md) |
| Supervised children and validated handoffs | [Local children](docs/project/contracts/c3-children.md), [remote A2A](docs/project/contracts/c3-a2a.md) |

## Integrate with an application

An ACP host launches the executable over stdio:

```sh
./target/release/pablo acp --stdio
```

The process supports successive independent sessions, with one prompt per session. Model connections and runtime setup can stay warm. Streaming updates, cancellation and typed task outcomes use the pinned stable ACP v1 protocol with negotiated Pablo extensions. See the [ACP integration contract](docs/acp.md).

To try the included TypeScript client, use the Node version pinned in `.nvmrc`:

```sh
nvm use
npm ci
cargo build --locked -p pablo --bin pablo
node examples/acp-client.ts "Read README.md and summarize it." "$PWD"
```

The client uses `target/debug/pablo` and makes a real provider request. Rust applications can call `pablo-core::Runtime` directly; see [embedding contracts](docs/runtime.md) and the [configured reference host](crates/pablo/examples/preset_host.rs).

## Diagnostics and traces

```sh
./target/release/pablo --help
./target/release/pablo doctor
```

`doctor` checks local setup and credential presence without model calls or MCP startup. Pass the same provider/configuration options as your task. Explicit `--probe provider` and `--probe mcp` opt into a provider request or MCP startup check; see [diagnostics](docs/project/contracts/c3-doctor.md).

To record an offline demo trace, choose a new filename:

```sh
mkdir -p .pablo/traces
./target/release/pablo demo --trace .pablo/traces/demo.jsonl
```

Native traces record metadata by default and preserve existing files. `--capture-content` explicitly includes task content in native traces. Network telemetry is off by default; [OpenTelemetry configuration](docs/telemetry.md) covers OTLP/HTTP export, incoming W3C context and privacy behavior.

## Development and release status

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm run typecheck
npm run test:acp
npm run test:telemetry
npm run test:deployment
node scripts/project.mjs check
```

These development checks use offline fixtures. Live provider checks are separate, explicit commands documented in the [gateway guide](docs/gateway.md#explicit-verification). The [knowledge-work benchmark](scripts/knowledge-work/README.md) covers factual scoring, latency, memory, CPU and cost comparisons; `npm run bench:knowledge` previews its matrix without model calls.

Release preparation still includes archives and installation checks, native acceptance on macOS arm64/x86_64 and Linux x86_64/arm64, matched release measurements, and prerelease distribution. Minimum OS/libc requirements and signing/notarization limits belong to that pending work. The full 0.1 contract also retains the deferred Otto integration proof. See the [release plan](docs/project/cycles/003-extensibility-and-release.md#c334-release-archives-and-installation) and [product design](docs/context.md#291-focused-release-contract).

For current progress and the next handoff, run `node scripts/project.mjs context`. Contributors should read [AGENTS.md](AGENTS.md); [project decisions](docs/project/brain.md) and the [backlog](docs/project/backlog.md) retain design rationale and deferred work.
