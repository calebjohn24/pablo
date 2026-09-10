# C3.1 deployment option inventory

This inventory owns the typed surface of the [deployment contract](c3-deployment-config.md). The [source schema](../schemas/deployment-v1.schema.json) fixes accepted shapes and the [defaults](../schemas/deployment-defaults-v1.json) fix exact baseline values. C3.1 is specification-only: C3.2 validates/resolves baseline options; C3.3 makes them usable through the current runtime. No field is advertised as implemented merely because it appears here. All paths below are under `options` unless stated otherwise.

## Baseline options

Notation: `uint` is an exact nonnegative integer, `positive` is 1–9,007,199,254,740,991 unless narrowed below; these size values fit every selected 64-bit target. `u64str` is a canonical decimal string in 0–18,446,744,073,709,551,615. Optional call counts accept integer 0–4,294,967,295 or `"unlimited"`; optional token/cost ceilings accept `u64str` or `"unlimited"`. Zero disables the corresponding calls/allowance; unknown actual usage remains unknown. `path?`/`name?` accept the typed value or `{unset = true}`. Bounds count UTF-8 bytes, not only schema character lengths.

Each row inherits owner C3.3, availability `c3.1` contract / C3.2 resolver / C3.3 execution, scalar replacement, and operator-only sensitivity unless explicitly changed. Typed tables merge by key. Credentials are references; none of these options accepts secret bytes. Full instruction/policy/environment-derived content is local-inspection-only; runtime summaries use fingerprints and deciding IDs.

| Option | Type and default | Runtime owner / additional constraints |
| --- | --- | --- |
| `run.workspace` | Path; explicit deployments default to binding `workspace`, path `.` | `RunSpec.workspace`; cannot be relative to itself; legacy CLI resolves invocation cwd, ACP uses `session/new.cwd`, Rust supplies its workspace |
| `run.instructions` | String, exact current CLI instruction in defaults JSON | `RunSpec.instructions`; at most 1 MiB; local inspection only. A configured embedded run uses this same value; raw `RunSpec::new` retains its empty instruction default |
| `model.provider` | Enum `vercel` (default), `openrouter` | C3.5 selection/inspection; OpenRouter admission fails until C3.6. [Provider contract](c3-provider-selection.md) defines adapter availability and capabilities |
| `model.id` | Nonempty string; Vercel `zai/glm-5.3-flash`, OpenRouter `z-ai/glm-5.3-flash` | C3.5 follows the user-selected defaults for runs and provider testing; explicit IDs override defaults; at most 256 UTF-8 bytes without whitespace/control. Catalog presence does not claim live acceptance |
| `model.endpoint` | Provider-owned literal chat-completions URL | Vercel `https://ai-gateway.vercel.sh/v1/chat/completions`; OpenRouter `https://openrouter.ai/api/v1/chat/completions`. Omission follows selection; explicit cross-provider/arbitrary endpoints reject |
| `model.credential` | Name, `gateway` | Must refer to one matching `provider.vercel` or `provider.openrouter` record; explicit presets declare it; compatibility mode synthesizes legacy references privately |
| `limits.max_model_calls`, `limits.max_tool_calls` | Optional u32 counts, `unlimited` each | `RunLimits`; never replace unlimited defaults with incidental config/parser limits |
| `limits.max_total_tokens`, `limits.max_cost_microusd` | Optional `u64str`, `unlimited` each | C2 attested admission/reservations; live Vercel still rejects unsupported hard ceilings before delivery |
| `limits.max_run_duration_ms`, `limits.max_tool_duration_ms` | Integer 1–86,400,000; 3,600,000 / 900,000 | Existing CLI range, expressed in ms; task-level shell timeout may narrow tool duration. Direct legacy Rust retains its existing representable-duration behavior |
| `limits.max_tool_input_bytes`, `limits.max_tool_output_bytes` | `positive`; 1,048,576 / 8,388,608 | Serialized arguments/results; output minimum 1,024 |
| `limits.max_context_bytes`, `limits.max_input_bytes`, `limits.max_output_bytes` | `positive`; 33,554,432 / 1,048,576 / 4,194,304 | Existing core byte accounting; config capacity is a separate bound |
| `limits.max_output_tokens` | Integer 1–4,294,967,295; 65,536 | Provider request bound, not an invented token count or enforceability attestation |
| `limits.max_events` | `u64str`, `"1000000"`, minimum 4 | Includes terminal/lifecycle slots |
| `limits.filesystem.max_file_bytes`, `max_scan_bytes` | `positive` or `"unlimited"`; `"unlimited"` each | Optional `FilesystemLimits` host quotas; UTF-8 reads/mutations/search |
| `limits.filesystem.max_entries`, `max_depth` | `positive` or `"unlimited"`; `"unlimited"` each | Optional traversal quotas; not import-parser limits |
| `shell.enabled` | Boolean, true | Tool registry enables `shell.run`; C3.4 command policy below is independently optional |
| `filesystem.enabled`, `filesystem.write` | Boolean; true / false | Tool registry enables read/list/search and explicit write/edit opt-in; write with disabled filesystem rejects |
| `policy.tools`, `executables`, `read_roots`, `write_roots` | Optional rule dimension, omitted by default | Exact C2 `Policy`; each supplied dimension requires `default = "allow"` or `"deny"`, with ordered `allow`/`deny` lists default `[]`; ordinary list replace/append/prepend supported |
| Policy list item | `{id, value}` | IDs are unique across effective and authority rules; 128 total allow+deny per dimension; executable values absolute, roots workspace-relative without traversal, tool names exact. Deny precedes allowlist precedes default |
| `trace.path` | `path?`, unset | CLI native JSONL file / ACP template; must be new at use. Render preserves `{session_id}`; task identity is outside config |
| `trace.capture_content`, `trace.max_bytes` | Boolean false; `positive` 268,435,456 | `TraceSettings`; capture requires destination; OTel never captures task content |
| `interfaces.cli_output` | Enum `text` or `json`, default `text` | One-task stdout framing; `--json` maps here. ACP stdio framing is selected by invocation, never this option |
| `otel.exporter`, `otel.sdk_disabled` | Enum `none`/`otlp`, `none`; boolean false | Standalone SDK owner; disabled SDK suppresses recording/export without losing native IDs |
| `otel.endpoint`, `otel.protocol` | Absolute URL `http://localhost:4318/v1/traces`; literal `http/protobuf` | Endpoint is a complete traces URL; general environment endpoint translation appends `/v1/traces` once |
| `otel.headers` | Credential `name?`, unset | One `otel.headers` record; header bytes never enter config or metadata |
| `otel.compression`, `otel.timeout_ms` | Enum `none`/`gzip`, `none`; `positive` 10,000 | Existing OTLP HTTP owner and whole export bound |
| `otel.service_name`, `otel.resource_attributes` | String `pablo`; map of string values `{}` | Name 1–256 bytes; ≤64 attributes, keys 1–256 bytes and values ≤4,096 bytes; service name overrides resource `service.name`, executable fixes `service.version`; no credentials/task content |
| `otel.sampler`, `otel.sampler_arg` | Enum below, `parentbased_always_on`; decimal string `"1"` | `always_on`, `always_off`, `traceidratio`, `parentbased_always_on`, `parentbased_always_off`, `parentbased_traceidratio`; ratio 0–1 with ≤17 fractional digits, consumed only by ratio samplers; retain exact source string in identity |
| `otel.propagators` | Ordered list, `["tracecontext"]` | Only `tracecontext` or `[]`; empty disables extraction. No baggage forwarding |
| `otel.max_queue_size`, `max_export_batch_size` | `positive`; 2,048 / 512 | Batch size cannot exceed queue size |
| `otel.schedule_delay_ms`, `export_timeout_ms` | `positive`; 5,000 / 30,000 | Earlier OTLP/batch deadline wins; sequential export; shutdown remains the existing fixed two seconds |

Policy omission preserves C2: tools/launcher/read roots default allow; write-root fallback is deny without explicit write opt-in, allow with opt-in. Enabling writes does not defeat an explicit root/tool deny. File config does not grant a broader shell environment, change the fixed launcher or attest provider spending bounds.

Top-level ownership: `schema_version`, `imports`, `profile`, `profiles`, `deployment`, `environment`, credential **references**, `authority` and path binding names belong to C3.2 resolution and C3.3 admission. The shared schema and defaults are the single specification of those values. The eventual Rust inventory must drive validation, defaults, explain/render and adapters; do not build independent CLI/ACP default tables.

## C3.4 shell options

Available at contract revision `c3.4`, owned by C3.4/Q01. The [literal shell contract](c3-shell-commands.md) defines parser/dispatch, byte bounds, authority intersections and metadata privacy. `commands` and `cwd_roots` are absent by default; `environment.values` defaults to `{}`. Legacy invocation remains unrestricted by command rules.

| Surface | Shape, composition and authority |
| --- | --- |
| `shell.commands` | Explicit default plus allow/deny `{id, executable, args, match}` rules; exact or whole-argument prefix; lists replace. Any supplied layer enables literal parsing. Immutable authority command layers intersect. |
| `shell.environment` | Non-secret `values` map merges by key; task values override defaults. Optional `allowed_names` lists replace and intersect with each authority name restriction. Authority values are forbidden. Only `PABLO_TASK_` additions, at most 32 effective names and 8,192 bytes/value. |
| `shell.cwd_roots` | Existing relative-root rules against canonical cwd/workspace; allow/deny lists accept existing replace/append/prepend operations. All authority cwd dimensions intersect. |

These dimensions do not accept `unset`: omission inherits, rule/name arrays replace (empty arrays clear), and a supplied command object continues to require literal parsing even with no rules. Ordinary replacement never removes an immutable authority dimension. They are not added to the locked per-run override allowlist. Provider/exporter credentials remain unavailable to shell. Old presets need no source conversion; render/fingerprints identify the new revision and empty environment default.

## Selected extension ownership

These namespaces are reserved and rejected by the baseline schema. The listed owner must add exact typed keys, numeric defaults/bounds, merge/clear behavior, redaction, authority intersections, availability revision, resolved schema and file/direct equivalence fixtures **before** making the feature usable. Later checkpoints own their detailed protocol semantics; C3.1 does not guess them. The inventory reservation includes all selected subsystem settings, not just an `enabled` flag.

| Reserved surface | Owner and availability gate | Required options and authority coverage |
| --- | --- | --- |
| OpenRouter execution / Open Responses selection and execution | OpenRouter C3.6; Open Responses C3.8–C3.9 | C3.5 provider selection and shared transport are implemented; each new adapter owns its verified mapping/capabilities, endpoint/credential scope and protocol fields |
| `models`, `routes`, `model_route` | C3.10 contracts, C3.11 execution / F01–F03 | Named exact profiles and ordered route, attempt/deadline policy, transient classes/uncertain-delivery opt-in, one retry owner, compatible continuation, child subset rules and accounting |
| `output` | C3.13 validation; C3.14 repair | Local schema reference/digest, supported Draft 2020-12 subset and work bounds, output mode, one-repair choice/feedback budget; envelope stdout remains separate |
| `mcp` | C3.15 contracts; C3.16 stdio, C3.17 HTTP, C3.18 integration | Named servers; executable/argv/cwd/cleared env or endpoint; scoped credential refs; required/optional startup; exact server/tool catalog/policy; request/result/progress/process/concurrency/startup/cleanup bounds |
| `skills` | C3.19 discovery; C3.20 activation | Explicit approved roots/activations, metadata/resource/instruction scan and byte bounds, identity/digest/duplicate handling, resource/tool/child authority; no install/auto-match |
| `children` | C3.21 contracts; C3.22 one, C3.23 two; C3.24 handoffs | Explicit enablement; depth one, active/total/queue/context/process/MCP/event limits; allowed routes/tools/Skills/MCP; workspace and output/handoff schemas/byte limits; narrowed budgets and joined cancellation |
| `a2a` | C3.26 cards; C3.27 delegation; C3.28 cancellation | Named approved card/endpoint and one binding, scoped auth, peer selection permission; input/card/task/artifact/update/time/cleanup limits, optional trace propagation; remote usage trust distinction |
| `interfaces.tui` | C3.29–C3.30 | TTY selection, composer/display defaults and retained bytes/events, safe control rendering; real cancellation/restoration proof |
| `diagnostics` | C3.31 | Offline doctor defaults and explicit probe selection; never eager MCP/network/model calls |
| `interfaces.acp`, `compatibility`, top-level `requires` | C3.33 | Supported capability/extension versions, narrowing supported transport bounds, binary/protocol/schema compatibility requirements; generic-peer fallback |
| Additional `trace`/`otel` settings | Owning feature; C3.32 coverage | Child/MCP/A2A attribution and fixed cleanup/transport settings only if newly configurable; content/privacy invariants persist |
| Packaging and rendered presets | C3.32, C3.34–C3.40 | Complete option audit, explicit migration, source/config/build identity, four native target presets, version/channel and install destination. Build/installer inputs remain outside a run config |

## Compatibility and intentional host-only inputs

Inventory audit sources are current [CLI configuration](../../../crates/pablo/src/config.rs), [RunSpec/RunLimits](../../../crates/pablo-core/src/contracts.rs), [filesystem limits](../../../crates/pablo-core/src/filesystem.rs), [policy](../../../crates/pablo-core/src/policy.rs), [gateway](../../../crates/pablo-core/src/gateway.rs), [ACP](../../../crates/pablo/src/acp.rs) and [telemetry](../../telemetry.md). No code behavior changes at C3.1.

| Existing input | Config mapping or deliberate exception |
| --- | --- |
| Quoted task / prompt / `RunSpec.input`; session ID | Dynamic per-run data, never persisted or hashed in a preset; input permission explicit, identities host/protocol-owned |
| `run`, shorthand, `demo`, `acp --stdio`, help/version | Invocation/interface selection or offline fixture; not a deployment-granted capability. Future TUI selection owned by C3.29 |
| `--workspace`, ACP cwd, Rust workspace | `run.workspace` after typed binding/override admission; locked ACP cwd must match or be an expressly permitted contained workspace |
| `--provider`, `--model`, Rust `ModelProfile` | `model.provider`, `model.id`; configured host resolves the same provider identity/default/endpoint |
| `--no-shell`, `--no-filesystem`, `--allow-write` | `shell.enabled = false`, `filesystem.enabled = false`, `filesystem.write = true` |
| `--policy PATH` | Parse existing bounded C2 JSON policy as one explicit ordinary policy layer; cannot erase accumulated authority. C3.3 retains legacy behavior; rendered output contains typed rules, not a second live policy-file reference |
| Timeout and `--max-*` flags | Corresponding `limits` leaves; seconds convert with checked multiplication by 1,000, u64 ceilings normalize to decimal strings |
| `--trace`, `--capture-content`, `--json` | `trace.path`, `trace.capture_content`, `interfaces.cli_output` |
| `--env-file`; Vercel environment and `.env` | Only compatibility mode retains environment canonical key, environment alias, then selected/default invocation `.env` canonical key/alias priority. Explicit config uses named sources; a locked config cannot trigger this ambient fallback |
| `--traceparent`, `--tracestate`, ACP negotiated metadata, Rust parent context | Dynamic validated incoming context, excluded from config identity; extraction governed by `otel.propagators`; existing 512-byte limits/baggage exclusion stay fixed |
| `OTEL_TRACES_EXPORTER`, `OTEL_SDK_DISABLED`, `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` | Corresponding explicit `otel` options; compatibility adapter retains SDK service-name precedence and diagnostics. Explicit config suppresses ambient SDK detectors |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG`, `OTEL_PROPAGATORS` | `otel.sampler`, `sampler_arg`, `propagators`; legacy parsing/unsupported-value behavior remains in compatibility mode |
| `OTEL_EXPORTER_OTLP[_TRACES]_ENDPOINT`, `_PROTOCOL`, `_HEADERS`, `_COMPRESSION`, `_TIMEOUT` | Endpoint/protocol/credential-ref/compression/timeout options; signal-specific environment wins in compatibility mode, with header sets replaced. Explicit config uses a complete endpoint and declared scoped secret reference |
| `OTEL_BSP_MAX_QUEUE_SIZE`, `_MAX_EXPORT_BATCH_SIZE`, `_SCHEDULE_DELAY`, `_EXPORT_TIMEOUT` | `otel.max_queue_size`, `max_export_batch_size`, `schedule_delay_ms`, `export_timeout_ms` |
| Unsupported OTel CA/client-key/insecure/metrics/log/config-file variables | Preserve current compatibility diagnostics/disabled export; explicit unknown/unsupported config keys fail before startup. No new pipeline/transport implied |
| Tracer/provider/tool trait instances, `EventSink`, cancellation/run handles, attested bounds | In-process host capabilities/callbacks, not serializable config or executable hooks. Host adapter identity/capability must match the resolved configured path; arbitrary embedded tools remain host-installed |
| Model tool arguments, filesystem revision/commit state, shell command/env/cwd | Dynamic task data, not config defaults. Bounds/policy/configured defaults have owners above; fixed `PABLO_TASK_` policy cannot forward provider/exporter variables |
| ACP frame/ingress/queue/write constants; gateway frame/response/request/call-assembly limits | Fixed safety/compatibility bounds in current code, not existing mutable settings. Any future narrowing knobs belong to C3.5/C3.33; configuration cannot raise compiled maxima |
| `/bin/sh`, fixed shell PATH/cleared env, cleanup time, one prompt per ACP session, OTel shutdown/privacy | Fixed behavioral invariants, not hidden flag-only settings. Changes require their own contract/evidence |
| Canonical physical roots, process environment snapshot, OS/architecture, clock, sockets, output FD, SDK `service.version` | Host/platform facts. Bindings and supported environment selections are explicit resolver inputs; secrets/live handles never serialize. Physical-root identity remains host-only |

Schema versioning does not tighten raw legacy Rust inputs or silently change existing CLI/SDK error fallbacks. The new configured path validates the documented portable types; equivalent representable values resolve identically across interfaces. C3.3 must test both the legacy no-config path and the explicit configured path before claiming compatibility.
