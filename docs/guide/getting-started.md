---
title: Getting started
description: Build Pablo, run its offline demo, configure a provider, and execute a first workspace task.
---

# Getting started

This guide builds Pablo from source, verifies the binary without a network request, and then runs one real task.

## Prerequisites

- macOS or Linux with a native C build toolchain
- Rust `1.98.1` (pinned by `rust-toolchain.toml`)
- Git
- A Vercel AI Gateway or OpenRouter key for live tasks

Node is not required to run the binary. It is used by the TypeScript ACP example, repository checks and the docs site.

## Build from source

```sh
git clone https://github.com/calebjohn24/pablo.git
cd pablo
cargo build --release --locked -p pablo --bin pablo
```

The executable is `target/release/pablo`. `--locked` ensures Cargo uses the checked-in dependency graph. Pablo is prerelease software and does not yet publish installation archives, so keep the executable with the checkout or copy it into a location you manage.

## Installer preview

The repository includes a checksum-verifying installer for the upcoming versioned archives. It is published for review at <https://runpablo.pages.dev/install.sh>, but it cannot install until a matching accepted archive set exists on the site’s Cloudflare R2-backed release route.

Once release assets are available, download and inspect the script, then choose an exact version and absolute prefix:

```sh
curl -fsSLo /tmp/pablo-install.sh https://runpablo.pages.dev/install.sh
sh /tmp/pablo-install.sh \
  --version v0.1.0-dev.1 \
  --prefix "$HOME/.local"
```

The installer refuses checksum failures and existing commands. Pass `--replace` only to replace the selected prefix’s regular `pablo` executable. `--remove` removes an unchanged installation after checking its receipt and leaves unrelated prefix files in place.

See [Installation and release files](./installation.md) for supported targets, minimum OS and libc versions, archive manifests, manual verification, and signing limits.

## Verify offline

```sh
./target/release/pablo --version
./target/release/pablo demo
```

The demo uses an in-memory scripted provider and prints:

```text
Hello from pablo.
```

It does not read provider credentials or make a network request.

## Add a provider credential

Pablo can read legacy CLI credentials from the process environment or a `.env` file in the directory where it is invoked. Environment variables take precedence.

For Vercel AI Gateway:

```sh
export AI_GATEWAY_API_KEY="your-key"
```

`VERCEL_AI_GATEWAY` is accepted as an alias. For OpenRouter:

```sh
export OPENROUTER_API_KEY="your-key"
```

You can instead create an ignored `.env`:

```dotenv
AI_GATEWAY_API_KEY=your-key
# OPENROUTER_API_KEY=your-key
```

Pablo parses this file as data. It does not source shell code or export values into the process. `--env-file PATH` selects another file. Never commit live credentials.

Configured deployments use explicit credential references and do not automatically inherit this `.env` behavior.

## Run a first task

The default provider is Vercel and the default model is `zai/glm-5.3-flash`:

```sh
./target/release/pablo run \
  "Read README.md and explain the project in five bullets." \
  --no-shell
```

Filesystem read/list/search tools remain available. `--no-shell` prevents shell execution for this task.

To use OpenRouter:

```sh
./target/release/pablo run \
  "Read README.md and explain the project in five bullets." \
  --provider openrouter \
  --no-shell
```

OpenRouter defaults to `z-ai/glm-5.3-flash`. `--model ID` chooses a different compatible model.

## Work in another directory

```sh
./target/release/pablo run \
  "List the top-level files and explain how this project is organized." \
  --workspace /absolute/path/to/project \
  --no-shell
```

The workspace must already exist. It controls the built-in filesystem tools and becomes the shell working directory. It does not change where an implicit `.env` is loaded.

## Choose tool authority deliberately

| Goal | Options |
| --- | --- |
| Read files without shell | `--no-shell` |
| Text-only model call | `--no-shell --no-filesystem` |
| Revision-checked file edits | `--allow-write --no-shell` |
| Shell and read-only filesystem | defaults; omit the flags above |
| Static host rules | `--policy /path/to/policy.json` |

Shell access is powerful: a permitted command runs as your user and can write independently of `--allow-write`. Use process/container isolation when a task must not reach the rest of the host.

## Bound a task

```sh
./target/release/pablo run "Explain the module boundaries." \
  --no-shell \
  --timeout 120 \
  --max-model-calls 3 \
  --max-tool-calls 5
```

Omitted model/tool call limits mean unlimited counts; zero disables the corresponding calls. Payload, context, event and deadline limits remain active either way.

## Machine-readable output

```sh
./target/release/pablo run "Summarize README.md." --json --no-shell > result.json
```

Standard output contains exactly one task envelope plus a newline. Progress is kept off stdout. The model’s answer remains a string inside the envelope; use `--output-schema` when you need locally validated JSON.

## Next steps

- Learn all commands in [CLI and TUI](./cli.md).
- Build a repeatable host in [Deployments](./configuration.md).
- Connect an application through [ACP](./acp.md).
- Call the runtime directly with [Rust embedding](./embedding.md).
