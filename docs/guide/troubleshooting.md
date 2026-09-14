---
title: Troubleshooting
description: Diagnose credentials, providers, policy, MCP, traces, ACP and terminal failures without exposing secrets.
---

# Troubleshooting

Start with `doctor` using the same provider, deployment, profile and bindings as the failing task:

```sh
pablo doctor --provider openrouter

pablo doctor \
  --config deployment.toml \
  --profile production \
  --bind workspace=./work
```

Doctor is offline by default. It checks the installed binary/platform, selected provider and models, credential presence, policy, tools, Skill roots, MCP/A2A definitions, child limits and trace/OTel setup. It does not run an agent, contact a provider, start MCP or export telemetry.

## Credential unavailable

For legacy CLI mode, verify the provider-specific variable:

```sh
pablo doctor --provider vercel
pablo doctor --provider openrouter
```

Vercel reads `AI_GATEWAY_API_KEY`, then the `VERCEL_AI_GATEWAY` alias, from the process environment before the selected/default `.env`. OpenRouter reads only `OPENROUTER_API_KEY`.

Common causes:

- `.env` is in the workspace rather than the invocation directory.
- `--env-file` points to a different file.
- An older environment value overrides the file value.
- The credential file has an invalid key or malformed entry.
- A configured deployment references a credential consumer/source that does not match the provider.

Pablo never prints, hashes or includes the resolved secret in diagnostics. Do not paste the key into task input to test it.

## Provider authentication or model failure

Opt into one small provider request:

```sh
pablo doctor --provider openrouter --probe provider
```

For a deployment, include its configuration arguments. The probe sends a fixed text-only request with a short deadline and no tools. It verifies transport/authentication and expected streaming content type, not answer quality or full task completion.

Diagnostic categories distinguish missing credentials, HTTP authentication/access rejection, unsupported model/request shape and transport/response failure. Remote response bodies are not copied into the report.

If a routed task does not fall back, inspect `pablo config explain`. Cancellation, invalid configuration, missing credentials, policy refusal, unsupported provider content, partial visible text and escaped tool calls are intentionally ineligible by default.

## Invalid deployment

Run the three inspection layers:

```sh
pablo config validate --config deployment.toml --bind workspace=./work
pablo config explain --config deployment.toml --bind workspace=./work
pablo config render --config deployment.toml --bind workspace=./work > rendered.toml
```

Frequent errors include:

- missing or non-integer `schema_version = 1`
- unknown option from a newer contract revision
- import outside `--config-root`, duplicate import or import cycle
- missing path binding such as `workspace`
- duplicate credential/profile/rule identity
- inline secret instead of a credential reference
- a locked deployment override that is not listed in `allowed_run_overrides`
- write enabled while filesystem reads are disabled
- content capture enabled without a trace path

Inspection does not contact services, so fix all local errors before using probes.

## Policy denied

The task outcome identifies either a built-in rule (`workspace`, `symlink`, `environment`, `tool_unavailable`, `unsupported_platform`) or a configured rule ID.

Use `config explain` to find every policy and authority layer that applies. Deny wins, nonempty allow lists restrict unmatched values, and all immutable authority ceilings intersect. A later ordinary option cannot override a higher-level denial.

For shell commands, check both launcher policy and literal command rules. A command may be rejected because its syntax cannot be proved against an allow rule, even when the executable name appears permitted.

## Filesystem conflict or missing write tool

`--allow-write` adds mutation tools only when policy permits them. It does not grant shell authority and cannot bypass denied write roots.

A revision conflict means the target changed after it was read. Read the current content/revision again and make a new deliberate edit. Do not reuse the old revision. `fs.edit` also fails when `old_text` is absent or appears more than once.

Symlink and traversal failures are expected protections. Use an ordinary file inside the admitted workspace.

## Tool timed out or cleanup failed

The tool deadline cannot exceed the remaining root deadline. Increase both when a legitimate command needs more time:

```sh
pablo run "Run the report." --timeout 600 --tool-timeout 300
```

Pablo waits for process-group kill, leader reaping and pipe draining. A `tool_cleanup` failure means it could not prove that owned cleanup completed; it is intentionally stronger than reporting a clean cancellation. Inspect the host/container process model and avoid commands that detach outside the owned process group.

## Context or output limit exceeded

`context_bytes` means the serialized instructions, message history, tool definitions/results and private continuation exceeded the task limit. Enable/configure compaction, narrow tool output, reduce the prompt/Skill set or raise the host’s context bound deliberately.

`output_bytes` or `output_tokens` means the response allowance was exhausted. A provider `length` finish does not become a successful completed answer.

`tool_output_bytes` may require a smaller requested tool page/result. Filesystem pages and shell stdout/stderr share bounded serialized results.

## Structured output failed

Inspect `output_validation.diagnostics` in the JSON task envelope. Diagnostics expose safe codes and JSON-pointer locations rather than values.

If repair is `blocked`, check remaining model-call, context and deadline capacity. If it is `failed`, both attempts were invalid. Simplify the schema or prompt, while preserving application requirements. Unsupported schema keywords reject before the provider is contacted.

## Trace file error

Native trace paths are create-only. Pick a new filename or use `{session_id}` for ACP:

```sh
pablo acp --stdio --trace '.pablo/traces/{session_id}.jsonl'
```

Ensure the parent directory exists and is writable. Pablo does not create parent directories. `--capture-content` requires `--trace`.

## MCP startup failure

Probe configured MCP startup without invoking a tool:

```sh
pablo doctor --config deployment.toml \
  --bind workspace=./work \
  --probe mcp
```

For stdio, verify the absolute executable, arguments, configured cwd, policy and credential references. The process receives a cleared environment. For HTTP, verify HTTPS, exact endpoint/header scope and protocol support. Redirects are rejected.

A required server failure rejects the task. An optional server is omitted with a bounded reason, but cleanup failure remains fatal.

## Skill not found or ambiguous

```sh
pablo skills list --config deployment.toml --bind workspace=./work
pablo skills show root/name --config deployment.toml --bind workspace=./work
```

Only configured roots are scanned. Use a qualified `root/name` when the same short name exists in multiple roots. Confirm the directory name matches frontmatter `name`, frontmatter uses the supported portable subset, and no root/package/file component is a symlink.

Discovery does not activate a Skill. Check `options.skills.activate` or an authorized `--skill NAME` override.

## ACP client stalls or disconnects

Consume session updates continuously and avoid blocking stdout. The server applies bounded backpressure; a client that stops reading eventually triggers cancellation/disconnect cleanup.

After requesting cancellation, keep the transport open and read the terminal prompt response. Parse negotiated Pablo metadata for the native outcome. Restart the ACP process when process-level provider/tool/exporter configuration changes.

## TUI display issue

The TUI requires terminal stdin/stdout, a non-dumb `TERM`, and text CLI output. A configured `interfaces.cli_output = "json"` is incompatible. Use `pablo run --json` for machine output.

If the terminal is left in an unexpected state after an external hard kill, run your shell’s normal terminal reset command. Normal exits, handled I/O failures and Ctrl-C paths restore modes before return.

## Safe support bundle

When reporting a problem, include:

- `pablo --version`
- `pablo doctor --json` with the same non-secret options
- operating system and architecture
- the terminal task outcome/error code
- a content-redacted native trace when available
- the output of `config explain` after reviewing operator-authored non-secret values

Do not include `.env`, credential files, raw native content traces, MCP environment/header values or complete provider response bodies.
