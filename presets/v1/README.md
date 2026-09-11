# Deployment presets v1

This bundle configures the C3 runtime with an OpenRouter primary
(`z-ai/glm-5.3-flash`) and Vercel fallback (`zai/glm-5.3-flash`), task-relevant
compaction, filesystem and shell policy, output validation with one repair,
Skills, MCP, local children, remote A2A, JSON CLI output, native traces and OTel.
Model calls, tool calls and filesystem work have unlimited defaults. Existing
duration, payload and ownership bounds still apply.

| Entry | Behavior |
| --- | --- |
| `base.toml` | Reusable module; read-only filesystem, only literal `printf` shell commands, explicit read-evidence MCP tool. Use `--locked` when running this module directly. |
| `production.toml` | Imports base; locks run overrides and adds immutable filesystem-write, shell-command, MCP-tool and trace-content ceilings. Local children inherit those ceilings. |
| `development.toml` | Imports base; locks run overrides, enables filesystem writes and literal shell commands, denies `/bin/rm`. Development shell access can write through other programs. |

Production's read-only guarantee covers Pablo's local filesystem tools and its
permitted local commands. The host must provision read-only MCP and A2A services
and its own OS sandbox. A tool name or Agent Card cannot prove a remote service
has no side effects.

## Configure and inspect

Copy the **whole** `presets/v1` directory to an operator-owned config directory.
Keep its Skill files with it. Replace `https://evidence.example.test/mcp` with
your approved MCP service exposing `read_evidence`, and replace the A2A card/RPC
URLs with your approved review service. These reserved example URLs are not live
services. Update the named tool policy if your service uses another tool name.
The Agent Card must advertise the exact configured RPC endpoint.

Provide credentials outside the TOML:

| Reference | Environment source | Consumer |
| --- | --- | --- |
| `router` | `OPENROUTER_API_KEY` | OpenRouter provider |
| `gateway` | `AI_GATEWAY_API_KEY` | Vercel provider |
| `evidence` | `PABLO_EVIDENCE_TOKEN` | MCP `x-evidence-token` header |
| `collector` | `PABLO_OTLP_HEADERS` | OTel headers, used only with the observed profile |

Configured runs do not automatically read a root `.env`. Named sources can instead
use the deployment contract's private file references. Never put credentials into
shell environment values, instructions, Skill files or rendered output. If A2A
authentication is needed, declare a separate `a2a.bearer` credential and bind it
under the remote's `bearer` record; provider credentials are not forwarded.

Example with operator-owned paths (no network during validation/rendering):

```sh
pablo config validate --config ./config/production.toml --bind workspace=./work --bind preset=./config
pablo config render --config ./config/production.toml --bind workspace=./work --bind preset=./config > ./config/rendered.toml
pablo run 'Review the evidence' --config ./config/rendered.toml --bind workspace=./work --bind preset=./config
pablo acp --stdio --config ./config/rendered.toml --bind workspace=./work --bind preset=./config
```

Keep the `preset` binding pointed at the original Skill assets after moving a
rendered TOML file. ACP `session/new.cwd` must match the workspace binding.
`input` and a narrowed run duration are the only permitted run overrides in the
production/development entries. Interface selection is an invocation choice;
JSON output does not change ACP framing. For TUI use, set `interfaces.cli_output`
to `text` in the operator-owned source before rendering.

`trace-{session_id}.jsonl` is created exclusively in the workspace for each root
run; native traces contain metadata with content capture disabled. The trace is
a host-owned diagnostic write, including in the read-only production preset.
Default OTel export is `none`. To enable it, set the collector URL in the base
file's `observed` profile, supply `PABLO_OTLP_HEADERS` as a comma-separated header
list, and add `--profile observed` when validating/rendering. The declared
endpoint and credential reference survive rendering. Ambient `OTEL_*` settings
cannot override them.

## Embedding and acceptance

`cargo build --locked -p pablo --example preset_host` builds the reference host.
It accepts the same task/config/binding arguments after the example name and
calls `Runtime` directly, installs the configured root owner, captures the native
terminal result, joins child/MCP cleanup and shuts down telemetry. It does not
launch `pablo run` or use an ACP subprocess as its embedding path. The reference
host reuses executable host modules from source; those modules are not a promised
public crate API. Embedders own their `Provider`, tracer, `EventSink`, cancellation
token, root owner and platform resources. The public core contracts remain the
integration boundary.

`tests/presets.test.ts` exercises rendered base, production and development through
CLI, the pinned ACP SDK and direct embedding using independent MCP/A2A SDK peers.
It tests eligible fallback, exact-fact compaction, Skill resources, MCP, local and
remote children, output validation, allowed/denied shell commands, filesystem
write policy, ordered native traces, explicit OTLP flushing and credential privacy.
The test instantiates only the collector address in an operator copy; gateway,
MCP and A2A loopback transports use explicit host-only fixture overrides. No real
provider credentials or live services are used.

## Versioning and migration

Bundle version `1` is distinct from deployment `schema_version = 1`, runtime
contract revision `c3.26`, the executable version and protocol versions. The
manifest records source-file digests and clean locked configuration/input
fingerprints for each entry. A render preserves the effective config fingerprint
but changes the input fingerprint because the source composition changed.
Physical workspace paths, secret values and per-run identities are excluded.

The early `docs/project/fixtures/c3-deployment` files remain historical C3.1
fixtures. They are not upgraded in place. To migrate, copy this bundle, transfer
your approved workspace/policy/endpoint/credential references, choose output
schema and write policy, then inspect `config explain`, run offline `doctor`,
and execute your acceptance task before replacing a deployed preset. Remove old
incidental model/tool/filesystem caps only when that is your intended policy.
There is no automatic migration, remote configuration fetch or environment-key
discovery. Unknown options fail explicitly. Keep a prior rendered preset and its
binary to roll back together.

C3.34 packaging must include this entire versioned directory and migration guide.
C3.35–C3.38 must rerun preset acceptance on macOS arm64/x86_64 and Linux
x86_64/arm64 using their native binary and reference host. Those packaging and
native gates remain separate from the current host's G04 acceptance.
