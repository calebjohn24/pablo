---
title: Deployments
description: Compose and inspect versioned TOML deployments for providers, policy, tools, output and telemetry.
---

# Deployments

A deployment is a versioned TOML document that turns Pablo’s host settings into a reviewable artifact. It can select providers and routes, bind a workspace, constrain tools, activate Skills, attach MCP/A2A peers, validate output and configure traces.

Use deployments for repeatable application behavior. The direct CLI flags remain convenient for local tasks.

## Minimal deployment

```toml
schema_version = 1

[credentials.gateway]
consumer = "provider.vercel"
sources = [{ kind = "environment", name = "AI_GATEWAY_API_KEY" }]

[options.model]
provider = "vercel"
id = "zai/glm-5.3-flash"
credential = "gateway"

[options.run]
workspace = { base = "binding", name = "workspace", path = "." }

[options.shell]
enabled = false

[options.filesystem]
enabled = true
write = false
```

Save this as `deployment.toml`, then validate and run it:

```sh
pablo config validate --config deployment.toml --bind workspace=./work
pablo run "Summarize the files in this workspace." \
  --config deployment.toml \
  --bind workspace=./work
```

A [copy-ready version](https://runpablo.pages.dev/examples/vercel.toml) is kept beside this guide.

The `workspace` binding is supplied by the host. Moving the deployment does not silently move its authority to an unrelated directory.

## Document shape

Every input requires `schema_version = 1`. Supported top-level fields are:

| Field | Purpose |
| --- | --- |
| `imports` | Ordered local modules, resolved within an approved config root |
| `profile` | Default named profile for an entry |
| `deployment` | Lock mode and allowed per-run overrides |
| `options` | Typed runtime, model, tool, output and telemetry settings |
| `credentials` | Named private source references |
| `environment` | Explicit non-secret environment-to-option bindings |
| `profiles` | Named option and authority layers |
| `authority` | Immutable constraints accumulated across composition |

Unknown fields fail closed. The parser accepts TOML booleans, strings, integers, arrays and tables in declared shapes. It rejects dates, floats, interpolation, code evaluation and inline secret values.

## Composition order

From lowest to highest precedence, Pablo applies:

1. Built-in defaults.
2. An explicitly selected user file and profile.
3. An explicitly trusted workspace file and profile.
4. The selected entry file and profile.
5. Declared non-secret environment bindings.
6. Typed host or CLI overrides.

There is no automatic user or workspace config search. Imports are visited depth-first in written order, then the importing file applies. Scalar values replace. Typed option tables merge recursively. Arrays replace unless an explicit `replace`, `append` or `prepend` operation is used. Authority layers only accumulate; ordinary options cannot clear them.

```toml
# development.toml
schema_version = 1
imports = ["base.toml"]

[deployment]
locked = true
allowed_run_overrides = ["input", "limits.max_run_duration_ms"]

[options.filesystem]
write = true
```

Imports must be relative local TOML files contained by `--config-root`. Symlinked inputs, repeated imports, cycles and root escapes reject.

## Profiles

Profiles add a named option/authority layer without duplicating a deployment:

```toml
schema_version = 1
profile = "local"

[profiles.local.options.otel]
exporter = "none"

[profiles.observed.options.otel]
exporter = "otlp"
endpoint = "http://localhost:4318/v1/traces"
headers = "collector"
```

Select another profile explicitly:

```sh
pablo config validate --config deployment.toml --profile observed \
  --bind workspace=./work
```

Profiles may extend other profiles. Cycles, missing parents and repeated ancestors reject.

## Path references and bindings

Config paths are typed objects rather than interpolated strings:

- `source` starts at the declaring file and normalizes relative to the approved config root.
- `config` starts at the approved config root.
- `workspace` starts at the admitted workspace.
- `binding` starts at an explicit named host root.

Example Skill root:

```toml
[options.skills.roots]
team = { base = "binding", name = "skills", path = "." }
```

Invocation:

```sh
pablo skills list --config deployment.toml \
  --bind workspace=./work \
  --bind skills=./approved-skills
```

Paths do not perform shell expansion, globbing or URL fetches.

## Credentials

TOML stores references, never secret values:

```toml
[credentials.router]
consumer = "provider.openrouter"
sources = [
  { kind = "environment", name = "OPENROUTER_API_KEY" }
]
```

Consumers are scoped. A provider credential cannot automatically become an MCP header, A2A bearer token, OTel header or shell environment value. Config inspection preserves source names while omitting resolved values.

Configured runs resolve only the credentials they use. They do not read legacy gateway keys from an ambient `.env` unless you explicitly model a supported private file source.

## Locking and authority

`deployment.locked = true` restricts per-run overrides to `allowed_run_overrides`. This is useful for an application that lets a caller supply task input or narrow a timeout while retaining the operator’s model, tool and trace choices.

Authority layers are permanent ceilings:

```toml
[[authority]]
id = "production.read_only"
capture_content = false

[authority.policy.write_roots]
default = "deny"
```

Normal options can narrow or select behavior inside those ceilings. They cannot widen authority past them. Each deciding configured policy rule has a stable ID for results and traces.

## Validate, explain, render

```sh
pablo config validate --config deployment.toml --bind workspace=./work
pablo config explain --config deployment.toml --bind workspace=./work > effective.json
pablo config render --config deployment.toml --bind workspace=./work > rendered.toml
```

- `validate` resolves structure and prints the effective configuration fingerprint.
- `explain` emits the normalized configuration and provenance.
- `render` writes a self-contained TOML entry with imports/profiles resolved.

All three are offline. They do not resolve credentials, scan the workspace, start MCP servers, create traces or contact providers.

The rendered file keeps credential references. Its effective fingerprint remains stable, while its input fingerprint changes because the source composition is now one file.

## Start from the bundled presets

`presets/v1` contains a reusable base plus locked production and development entries. The bundle exercises model fallback, compaction, tool policy, output validation, Skills, MCP, children, A2A, native traces and optional OTel.

Copy the whole directory so its Skill assets and imports remain together. Replace reserved example MCP/A2A URLs before executing it. See [`presets/v1/README.md`](https://github.com/calebjohn24/pablo/blob/main/presets/v1/README.md) for the complete operator workflow.
