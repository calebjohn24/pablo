# C3 declarative deployment configuration

The user requested a NixOS-style file system for preconfiguring every part of Pablo, including an ordered list of models and shell command allow/deny rules. [C3](../cycles/003-extensibility-and-release.md) selects this work early, then requires every later feature to participate. This document fixes design intent; exact syntax and schemas freeze at C3.1 and the owning feature checkpoints. Examples below are proposed TOML, not currently supported configuration.

## Declarative options and composition

Use typed, versioned TOML with local module imports and named profiles, following the brief's section 20. The NixOS inspiration is composable option declarations, defaults, deterministic resolution, assertions and value provenance. Pablo evaluates bounded data; it does not need Nix, evaluate arbitrary code or provision an operating system. Deployment tools may generate the same canonical configuration externally.

A deployment has one explicit entry file, optional local imports, a selected profile and declared environment/secret bindings. Define each option once in the runtime's option inventory with type, default, merge behavior, sensitivity, owner and availability version. Feature modules register their options with this inventory so the schema, validation, effective-value display and runtime adapters share one definition. Unknown keys, conflicting definitions, unsupported feature versions and invalid combinations are errors before model calls, tools or connections start.

Freeze deterministic import order and per-type merge semantics at C3.1: defaults supply missing values; later explicitly selected layers can replace scalar values; maps merge by declared keys; ordered model/rule lists use explicit replace/append/prepend semantics without accidental sorting or set conversion. A list's order must survive round trips. State how a value is cleared and how incompatible definitions fail. Reject import cycles and excessive files/depth/bytes. Imports resolve relative to the file that declares them and stay within approved configuration roots; do not fetch remote imports or execute environment substitutions.

Ordinary local precedence follows section 20: defaults, user/profile config, explicitly trusted workspace config, selected environment values, then CLI/host run overrides. The explicit entry/profile and module ordering must remove ambiguity between those layers. Separately apply a host/deployment authority ceiling after resolution; a higher-precedence convenience flag cannot remove a locked deny, add a credential destination or expand a workspace.

Locked deployment mode reads only the selected entry/imports/profile and declared environment bindings. Ambient home/workspace files and unrelated variables cannot change behavior. Hosts explicitly designate which run fields may vary, such as task input or a narrowed deadline; disallowed overrides fail visibly. Secrets are references to approved environment, private file or host credential sources, resolved privately at use; evaluated/rendered configuration and fingerprints never include secret bytes. Credential-file contents remain outside shell tools and project records.

Snapshot the resolved config at admission. Later file edits apply to subsequent runs; no hot reload changes policy under an active model/tool call. Record schema version, declared input digests, redacted effective-config fingerprint and important value origins. Document whether runtime/environment-dependent paths contribute to identity rather than claiming reproducibility across different declared inputs.

## Coverage and inspection

| Configurable area | Required coverage |
| --- | --- |
| Providers/models/routes | Provider endpoints and credential references; named exact model profiles and options; capability requirements; ordered model route, eligible failures and attempt/deadline policy |
| Run and output | Workspace, host instructions/defaults, model/tool/token/cost/process/byte/time limits, final-output schema reference, one-repair policy and machine-output mode |
| Shell/filesystem | Enablement, launcher and command/argument rules, allow/deny defaults, environment additions, cwd/read/write roots, mutation opt-in and per-tool limits |
| MCP | Named servers, transports, launch/endpoint options, scoped secret references, required/optional startup, catalogs, exact server/tool policy and request/progress/cleanup bounds |
| Skills | Approved roots, explicit activations, resource limits and authority; no automatic installation or activation |
| Local children | Enablement, concurrency/depth/queue/total limits, approved model routes, inherited tools/Skills/MCP, workspace/context/handoff bounds and cancellation |
| A2A | Named remote cards/endpoints and secret references, enabled binding, authority to select a peer, input/artifact/time bounds and optional correlation |
| Interfaces | CLI/TUI defaults, ACP capability/transport limits and permitted per-run host overrides; no second process protocol |
| Trace/OTel | Trace directory/capture/size, exporter enablement/endpoint/scoped credential reference, service/resource/sampling settings and flush limits |
| Deployment/compatibility | Config schema version, profile/imports, lock mode, binary/protocol compatibility requirements and deterministic rendered preset |

“Every part” means every supported behavior-setting option can be supplied by the deployment, with intentional exceptions documented for in-process callbacks, live handles, secret bytes and platform facts. Future features add options only when implemented; reserved sections cannot pretend to activate missing capabilities. The C3.32 audit compares this inventory to actual CLI/ACP/core settings.

`config validate` checks options/references/compatibility offline; `config explain` shows effective values and their sources/overrides with redaction; `config render` emits a canonical portable preset retaining secret references. Equivalent resolved configs feed CLI, ACP and embedding through one resolver. `doctor` adds local diagnostics without performing a task or starting configured MCP servers by default. Ship development and read-only deployment examples and prove them in native acceptance.

## Ordered models and fallback

A model profile names its gateway/endpoint, exact model, credential reference, capabilities and supported model options. A route is an ordered list of profile references, never a dynamic ranking heuristic. Select the first compatible allowed entry and report why any entry is rejected; unknown profiles, repeated/cyclic route references or impossible tool/output requirements fail before a run. Children inherit an approved route or ordered subset and cannot append new models or credential destinations.

At C3.10 freeze whether a successfully selected entry remains preferred for later calls in the run; the default design is sticky selection, proceeding to later entries only on an eligible failure. Each route has explicit attempt limits, deadline behavior, eligible error classes and retry owner. The default is one model with no fallback. When configured, Pablo owns ordered model fallback and sends one model per gateway attempt; do not also ask the gateway to run its own ordered model list. Opaque gateway upstream routing remains separately reported and cannot support a fabricated spending guarantee.

Eligible default triggers are narrowly mapped transient service/rate-limit failures and errors proven not sent. Cancellation, policy refusal, schema/config errors, missing credentials and provider content refusals do not trigger fallback by default. Transport errors with uncertain delivery require explicit route opt-in and conservative retained charges. Stop if text or tool-call output from the failing attempt has already escaped, or if provider-specific continuation cannot be represented faithfully by the next adapter. A fallback never restarts the run, reruns completed tools or silently drops continuation items.

Each attempted request counts against the existing root model-call/time/token/cost ledger and gets its own model/span identity. Preserve missing actual usage as unknown. Prevalidate enforceable hard ceilings across every eligible entry; reserve before each attempt, settle once and retain uncertain reservations. Exhausted routes return one typed failure with bounded attempt metadata. Output repair remains one separate same-run validation operation, with any configured fallback attempts still consuming this same budget.

## Shell command policy

C2 controls the `/bin/sh` launcher; it does not inspect the script supplied to `sh -c`. C3.4 adds an independent command/argument policy. Define allow-only, deny-only and combined policies with explicit defaults and stable rule IDs; an explicit deny wins and a nonempty allowlist requires a match. Evaluate the host/deployment ceiling as well as child restrictions.

Freeze a small literal-argument command subset with exact executable and argument matching plus documented argument-prefix rules if included. Resolve the executable through the fixed allowed path or explicit absolute identity and ensure the dispatched argv is the one evaluated. Quoting must preserve argument boundaries. Under command restrictions, reject unsupported substitutions, operators, redirections, pipelines, variable expansion and compound scripts before launch. Do not use raw substring searches or imply that a launcher allowlist constrains nested commands. Configured trusted script support would require its own explicit identity/digest contract; it is not an implicit escape hatch.

Unconfigured command rules retain C2's ordinary shell semantics. Configured rules govern admitted commands, not what an allowed program, script or remote MCP service may do internally; hosts own OS/process/network containment. Native traces record deciding rule IDs without echoing private commands. Invalid or denied commands keep the current terminal policy outcome until policy recovery is separately selected.

## Proposed deployment example

The example illustrates option relationships; C3.1/C3.10/C3.4 freeze the final spelling and matching semantics. `primary-model` and `secondary-model` are placeholder IDs, not claims of gateway availability. A base module can hold shared bounds and telemetry defaults; the production entry narrows authority.

```toml
schema_version = 1
imports = ["modules/base.toml"]
profile = "production"

[deployment]
locked = true
allowed_run_overrides = ["input"]

[credentials.vercel]
environment = "AI_GATEWAY_API_KEY"

[credentials.openrouter]
environment = "OPENROUTER_API_KEY"

[models.primary]
provider = "vercel"
model = "primary-model"
credential = "vercel"

[models.secondary]
provider = "openrouter"
model = "secondary-model"
credential = "openrouter"

[routes.analysis]
models = ["primary", "secondary"]
fallback_on = ["rate_limited", "unavailable", "not_sent"]
max_attempts = 2
retry_owner = "runtime"
allow_uncertain_delivery = false

[profiles.production]
model_route = "analysis"

[filesystem]
roots = ["."]
write = false

[shell.commands]
default = "deny"

[[shell.commands.allow]]
id = "inspect.git-status"
executable = "/usr/bin/git"
arguments = ["status", "--short"]
match = "exact_argv"

[[shell.commands.deny]]
id = "deny.git-push"
executable = "/usr/bin/git"
arguments = ["push"]
match = "argv_prefix"
```

## Planning sources and acceptance

The [NixOS module manual](https://nixos.org/manual/nixos/stable/#sec-writing-modules) describes option declarations, composition, priorities and source-aware conflicts. C3 adopts those design ideas in a bounded TOML system; it does not claim Nix language compatibility. The [OpenRouter model fallback documentation](https://openrouter.ai/docs/guides/routing/model-fallbacks) describes gateway-owned ordered model lists; C3 deliberately assigns one retry owner and verifies its runtime-owned route across gateways. Both sources were inspected 2026-09-09; the unchanged brief's sections 19–20 remain the existing project design context.

G01–G04, Q01 and F01–F03 in the [fixture map](../fixtures/c3-extensibility-and-release.md) cover configuration contracts/composition/inspection, shell restrictions, routes/fallback and full deployment proof. Each later feature also proves file-config equivalence when it ships. These are C3 requirements, with no config implementation or execution evidence claimed by C3.0.
