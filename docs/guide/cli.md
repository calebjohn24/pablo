---
title: CLI and TUI
description: Complete command guide for run, JSON output, the terminal UI, inspection, diagnostics and Skills.
---

# CLI and TUI

The `pablo` executable exposes one runtime through human, machine and protocol interfaces.

## Command map

```text
pablo tui [TASK] [RUN OPTIONS...]
pablo "TASK" [RUN OPTIONS...]
pablo run "TASK" [RUN OPTIONS...]
pablo acp --stdio [RUN OPTIONS...]
pablo demo [--trace PATH] [--capture-content]
pablo config validate|explain|render --config PATH [...]
pablo skills list|show ... --config PATH
pablo doctor [--json] [RUN OPTIONS...] [--probe provider|mcp]
pablo --version
pablo --help
```

With a capable terminal, running `pablo` without arguments opens the TUI. Redirected input/output prints help instead. `pablo "TASK"` is shorthand for `pablo run "TASK"`.

## Run options

| Option | Meaning |
| --- | --- |
| `--workspace PATH` | Existing task workspace; defaults to the invoking directory |
| `--provider vercel\|openrouter` | Select a legacy CLI provider |
| `--model ID` | Select a compatible model identifier |
| `--env-file PATH` | Select a credential file |
| `--no-shell` | Remove the built-in shell tool |
| `--no-filesystem` | Remove built-in filesystem tools |
| `--allow-write` | Add revision-checked filesystem write/edit tools |
| `--policy PATH` | Apply static tool, launcher and filesystem-root rules |
| `--timeout SECONDS` | Root deadline, 1–86,400 seconds |
| `--tool-timeout SECONDS` | Per-shell deadline, 1–86,400 seconds and no later than the root |
| `--max-model-calls N` | Optional model call limit; zero disables calls |
| `--max-tool-calls N` | Optional tool call limit; zero disables calls |
| `--json` | Emit one machine task envelope |
| `--output-schema PATH` | Require a final JSON value matching a local schema |
| `--reasoning-effort VALUE` | Request a provider-supported effort level |
| `--reasoning-budget-tokens N` | Request a provider-supported reasoning budget |
| `--trace PATH` | Create a new native JSONL trace |
| `--capture-content` | Include content in that native trace; requires `--trace` |
| `--traceparent VALUE` | Continue an incoming W3C trace |
| `--tracestate VALUE` | Supply trace state with `--traceparent` |

Reasoning effort values are `provider_default`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh` and `max`. Effort and a token budget are mutually exclusive. Providers validate support before work begins.

## Streaming mode

Default `run` output is meant for a terminal. Model text streams to stdout. Model and tool progress appears on stderr. Shell output is collected as a bounded tool result rather than attached as a live interactive terminal.

Ctrl-C cancels the shared token. Pablo continues awaiting owned cleanup before it exits. Completion exits `0`; cancellation exits `130`; an admitted non-completed outcome generally exits `1`; setup and argument errors exit `2`.

## JSON mode

`--json` reserves stdout for one bounded task envelope:

```sh
pablo run "Return the project name." --json --no-shell > task.json
```

The envelope contains schema revision, run/session/trace IDs, outcome, error and accounting. Unsigned counts and micro-USD values are decimal strings so JavaScript cannot round them. Unknown provider usage is `null`. Parse those numbers with `BigInt` when doing arithmetic.

If setup fails before admission, IDs, outcome and accounting are null and `error` contains a closed code such as `invalid_arguments`, `invalid_configuration` or `credential_unavailable`.

## Interactive terminal

```sh
pablo tui --provider openrouter
```

The TUI renders Markdown, tool lifecycle entries and retained visual history. Each submission starts a new task with fresh model context. Controls:

- Enter submits the current prompt.
- Page Up/Page Down or the mouse wheel scrolls history.
- Ctrl-End returns to the live tail.
- Ctrl-C cancels an active task and waits for cleanup; with no task it exits.
- Terminal modes are restored after normal exit, cancellation and handled failures.

The TUI requires text output. A deployment with `interfaces.cli_output = "json"` must be changed before it can drive the TUI.

## Offline demo

```sh
pablo demo
pablo demo --trace .pablo/traces/demo.jsonl
```

The demo uses a scripted provider. A trace destination must not already exist. On Unix it is created with private permissions.

## Configuration inspection

```sh
pablo config validate --config deployment.toml --bind workspace=./work
pablo config explain --config deployment.toml --bind workspace=./work
pablo config render --config deployment.toml --bind workspace=./work > rendered.toml
```

Inspection is offline and does not resolve secrets. `validate` prints the effective fingerprint. `explain` returns resolved JSON with provenance. `render` creates a self-contained TOML entry with imports/profiles applied and credential references preserved.

## Skills inspection

```sh
pablo skills list --config deployment.toml --bind workspace=./work
pablo skills show root/name --config deployment.toml --bind workspace=./work
```

These commands discover metadata only from explicitly configured roots. Run/ACP uses `--skill NAME` to replace the deployment’s active Skill set when override policy allows it.

## Doctor

```sh
pablo doctor
pablo doctor --json --provider openrouter
pablo doctor --probe provider --provider openrouter
pablo doctor --config deployment.toml --bind workspace=./work --probe mcp
```

Doctor is offline by default. Credential presence is inspected privately; reachability remains unknown. Provider and MCP probes are explicit, bounded operations and do not run an agent or invoke an MCP tool.
