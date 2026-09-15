---
title: Getting started
description: Install Pablo, verify it locally, and connect the runtime to your application through ACP or the Rust core.
---

# Getting started

Pablo is an agent runtime for applications. Your application owns users, business state, workspaces, sandboxing and approvals; Pablo owns the bounded model-and-tool lifecycle and returns streamed events plus one typed outcome.

This guide installs the runtime, verifies it without a model call, runs one workspace task and then connects it to your application.

## Choose an integration boundary

<div class="integration-choices">
  <a class="integration-choice" href="#embed-pablo-as-a-child-process">
    <span class="integration-choice-label">Most application hosts</span>
    <strong>ACP child process</strong>
    <span>Use from JavaScript, TypeScript, Python, Go, Java or another language. Keep Pablo isolated and independently upgradeable while receiving streamed updates, cancellation and typed outcomes over stdio.</span>
  </a>
  <a class="integration-choice" href="#embed-pablo-in-a-rust-process">
    <span class="integration-choice-label">Rust applications</span>
    <strong><code>pablo-core</code></strong>
    <span>Construct the runtime in process and inject the exact provider, tools, event sink, telemetry and cancellation behavior your host needs.</span>
  </a>
  <a class="integration-choice" href="#run-a-first-workspace-task">
    <span class="integration-choice-label">Prototypes and jobs</span>
    <strong><code>pablo run --json</code></strong>
    <span>Exercise the same runtime lifecycle through one machine-readable command before adopting a persistent integration.</span>
  </a>
</div>

Most application teams should begin with ACP. It gives the host a stable process boundary and lets the Pablo executable move independently from the application. Choose the Rust core when in-process integration is a deliberate requirement.

## Install Pablo

Pablo targets macOS and Linux on arm64 and x86_64. The installer selects the native archive, verifies its SHA-256 checksum and reported version, and writes the executable plus an ownership receipt beneath the prefix you choose. It does not use `sudo`, a package manager, Node or Python.

::: warning v0.0.1 prerelease
The `v0.0.1` archives are available as a public preview while the full four-platform acceptance matrix remains in progress. The macOS binaries are unsigned and not notarized. Pin this exact version and review [release status](./release-status.md) before production use.
:::

### Versioned installer

Install the native Mac or Linux preview binary with one command:

```sh
curl -fsSL https://runpablo.pages.dev/install.sh | sh
```

The script pins `v0.0.1` and installs to `$HOME/.local/bin/pablo`. It never resolves a moving `latest` version. Repeating the command succeeds without changing an identical receipt-backed installation. Add that directory to the current shell if needed:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

A later exact version can replace an unchanged receipt-backed installation with `--update`. See [Installation and release files](./installation.md) for inspection, version and prefix overrides, supported platforms, manual verification, upgrades and removal.

### Build from source

A source build requires Git, a native C build toolchain and Rust `1.98.1`, pinned by `rust-toolchain.toml`:

```sh
git clone https://github.com/calebjohn24/pablo.git
cd pablo
cargo build --release --locked -p pablo --bin pablo

mkdir -p "$HOME/.local/bin"
install -m 755 target/release/pablo "$HOME/.local/bin/pablo"
export PATH="$HOME/.local/bin:$PATH"
```

`--locked` uses the repository’s checked-in dependency graph. Node is only needed for the TypeScript ACP reference client and repository tooling.

This source-built copy has no installer receipt. Remove it or pass `--replace` deliberately when you later switch the same prefix to a versioned archive.

## Verify the installation offline

```sh
pablo --version
pablo demo
```

The demo uses an in-memory scripted provider and prints:

```text
Hello from pablo.
```

It does not read provider credentials or make a network request. Run `pablo doctor` for a credential and configuration check that also stays offline unless you explicitly add a probe.

## Configure a model provider

For Vercel AI Gateway:

```sh
export AI_GATEWAY_API_KEY="your-key"
```

`VERCEL_AI_GATEWAY` is accepted as an alias. For OpenRouter:

```sh
export OPENROUTER_API_KEY="your-key"
```

For local evaluation, Pablo can instead read an ignored `.env` in the invocation directory:

```dotenv
AI_GATEWAY_API_KEY=your-key
# OPENROUTER_API_KEY=your-key
```

Pablo parses the file as data and never sources it as shell code. In an application host, pass only the selected credential to the Pablo child process or inject it into your provider object. Keep credentials out of tasks, deployment files, ACP frames and traces.

Configured deployments use explicit credential references instead of automatically reading `.env`. See [Models and providers](./providers.md) for model selection, reasoning controls, Open Responses and fallback.

## Run a first workspace task

Give Pablo a workspace owned by your application and remove shell authority for this first run:

```sh
mkdir -p /tmp/pablo-quickstart
printf '%s\n' 'Quarterly review is due Friday.' > /tmp/pablo-quickstart/brief.txt

pablo run \
  "Read brief.txt and return the deadline in one sentence." \
  --workspace /tmp/pablo-quickstart \
  --no-shell
```

The default provider is Vercel and the default model is `zai/glm-5.3-flash`. Filesystem read, list and search remain available; `--no-shell` prevents shell execution. Add `--provider openrouter` to use OpenRouter’s default `z-ai/glm-5.3-flash` model.

For a machine-readable result:

```sh
pablo run \
  "Read brief.txt and return the deadline in one sentence." \
  --workspace /tmp/pablo-quickstart \
  --no-shell \
  --json > result.json
```

Standard output contains one task envelope plus a newline. The envelope carries the typed outcome, identifiers and exact decimal-string accounting. The answer remains a string; add `--output-schema` when your application requires locally validated JSON.

## Connect your application

### Embed Pablo as a child process

Start one Pablo process for each provider, tool and telemetry configuration that your application wants to keep warm:

```sh
pablo acp --stdio --no-shell
```

The process speaks stable Agent Client Protocol v1 over newline-delimited stdio. A host should:

1. Spawn `pablo acp --stdio` with the selected credentials and process-level options.
2. Negotiate Pablo’s current extension metadata during ACP initialization.
3. Create an independent session with an absolute application-owned workspace.
4. Continuously consume streamed agent and tool updates.
5. Send one task prompt and parse the terminal Pablo task object, not only the generic ACP stop reason.
6. On user cancellation, send `session/cancel` and keep reading until Pablo settles cleanup or closes the connection.
7. Restart the process when provider, tool, policy or exporter configuration changes.

The repository includes a complete TypeScript host using the pinned official ACP SDK. Run it against the installed executable:

```sh
git clone https://github.com/calebjohn24/pablo.git
cd pablo
nvm use
npm ci

PABLO_BINARY="$(command -v pablo)" node examples/acp-client.ts \
  "Read brief.txt and return the deadline." \
  /tmp/pablo-quickstart \
  --no-shell
```

The example shows initialization, session creation, update streaming, cancellation, outcome validation and exact accounting. Use [ACP integration](./acp.md) to adapt that lifecycle to your application.

### Embed Pablo in a Rust process

Use `pablo-core` when your Rust host needs to construct the runtime directly. The host injects a `Provider`, `EventSink`, `ToolRegistry`, cancellation token and OpenTelemetry tracer, then awaits the returned typed outcome.

The crate is not published to crates.io during the prerelease. Pin this repository to an exact reviewed revision in your application and start with the offline `ScriptedProvider` example in [Embed in Rust](./embedding.md). The core discovers no credentials and grants no tools unless your host supplies them.

## Define production authority

Before connecting user work, decide these host-owned boundaries:

| Decision | Pablo integration point |
| --- | --- |
| Which files a task may see | Bind an absolute workspace and configure filesystem roots. |
| Whether commands may execute | Omit shell capability or apply explicit launcher and tool policy. |
| Which external tools are trusted | Configure exact MCP servers, transports, schemas and credential bindings. |
| Whether work may be delegated | Configure supervised local children or admitted remote A2A peers. |
| How long and how much work may run | Set deadlines, output bounds and optional model/tool call limits. |
| How results enter product state | Validate the terminal outcome and optional output schema before committing changes. |
| How operators observe tasks | Consume native events and configure metadata-only OpenTelemetry export. |

The workspace is a filesystem boundary inside Pablo’s built-in tools, not an operating-system sandbox. A permitted shell command has the child process account’s ordinary authority. Run Pablo in your own container or process sandbox when the task must be isolated from the rest of the host.

## Next steps

- Follow the [ACP host guide](./acp.md) for language-neutral process integration.
- Follow [Rust embedding](./embedding.md) for direct runtime construction.
- Turn flags into a reviewable [deployment](./configuration.md).
- Add bounded local or remote delegation with [Subagents](./subagents.md).
- Configure [tools and policy](./tools-and-policy.md), then add [MCP, Agent Skills and A2A](./extensibility.md).
- Validate [structured output](./structured-output.md) and wire [observability](./observability.md) before writing results into application state.
