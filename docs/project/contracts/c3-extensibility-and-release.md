# C3 scope and contract map

This document accompanies [cycle C3](../cycles/003-extensibility-and-release.md). It records selected requirements and where exact contracts must be frozen before implementation. It does not change the current native `c2.4` or task `c2.3` wire schemas. The [fixture map](../fixtures/c3-extensibility-and-release.md) contains initial synthetic examples; owning checkpoints add executable inputs and exact expected mappings before code changes. The [deployment design](c3-deployment-config.md) covers the user's declarative configuration, ordered fallback and command-policy additions.

## Baseline and gap review

Reviewed the completed C2 state/evidence, current provider/contracts/gateway/policy types and README's interface descriptions against design sections 11.5–11.7, 15–17, 19–20, 26–28, 29.1, 30 and 31.1. Baseline commit: `fb7cbb0141aed5f4a3befc91d4b1fb4dd22e01f1`. Current C2 source and observed evidence remain implementation truth.

| Surface | Observed C2 foundation | Selected C3 work |
| --- | --- | --- |
| Deployment config | CLI/core settings, private key loading and a standalone policy file; no composed deployment resolver | Typed TOML/modules/profiles, deterministic merge, validate/explain/render and locked deployments (C3.1–C3.3, C3.32) |
| Providers | Vercel direct HTTP/SSE, adapter-owned credentials, shared normalized provider trait; no provider-selection surface | Selection and OpenRouter (C3.5–C3.7); native Open Responses (C3.8–C3.9) |
| Model routes | One model; no runtime fallback | Named ordered model profiles, configured fallback and per-attempt accounting (C3.10–C3.12), explicitly promoted by the user |
| Context | Hard context-byte guard with growing task history | One bounded summary pass and one context-overflow recovery, preserving prefix, recent turns, accounting and privacy (C3.12a) |
| Results/schema | Built-in argument validation; task envelope preserves a final string and exact accounting | General final-output schema validation and one same-run repair (C3.13–C3.14) |
| Tools/policy | Shell plus five filesystem tools; exact tool/launcher/root rules; revision-checked atomic replacement | Explicit shell command/argv policy (C3.4); named MCP server/tool admission with stdio and Streamable HTTP transports (C3.15–C3.18) |
| Skills | No discovery, activation or selected-resource loader | Local metadata catalog and explicit progressive activation (C3.19–C3.20) |
| Ownership/accounting | Single run; optional call caps; attested aggregate reservations; unknown usage preserved | Typed in-memory ACP children, narrowed authority, shared atomic root ledger, concurrency and handoffs (C3.21–C3.24) |
| Remote delegation | No A2A client/server | Configured A2A 1.0 client, one HTTP/SSE binding and untrusted proxy (C3.26–C3.28) |
| Interfaces | CLI/shorthand/JSON, Rust embedding, TypeScript ACP; no TUI or doctor | Basic composer/event view, real PTY proof and focused offline diagnosis (C3.29–C3.31) |
| Compatibility/telemetry | Pinned stable ACP v1, negotiated development metadata, bounded native events and real Collector proof | Explicit schema migrations, protocol conformance and native MCP/child/A2A spans (feature checkpoints and C3.33) |
| Delivery | Validated local/CI builds; no published release; C2 native macOS arm64/Linux x86_64 evidence | Archives/install path, four separate native targets, measured baseline and selected prerelease distribution (C3.34–C3.40) |
| Full release | Neither C1 nor C2 claims the full section 31.1 contract | C3 excludes Otto by user direction; retain that release gate and any unsupported live ceiling profiles |

## Shared invariants

- One runtime lifecycle owns root, local-child, model, tool, event, outcome and telemetry state. ACP stays the sole required process protocol. In-memory children use official typed ACP handlers without serializing JSON or creating another proprietary session lifecycle.
- Keep C1/C2's independent one-prompt ACP sessions, client/catalog/SDK reuse, immediate first text, bounded queues, physical-output backpressure, exactly one terminal outcome and joined cleanup. Shared resources must remain safe when child operations overlap.
- Keep existing default call counts unlimited and retain existing byte/time defaults unless an owning checkpoint records a justified change. New catalogs, children, pending work and remote streams require explicit aggregate bounds; each child cannot independently claim the root's remaining allowance.
- Preserve C2 static deny precedence and deciding IDs. Policy is enforced before launch, connection, dispatch or mutation. Extend it with exact MCP server/tool and child capability intersections when those features are introduced. Skills and peer metadata are input, never authorization.
- Root accounting admits work atomically before delivery and settles once. Known actual usage, conservative charges, unknown usage and remote-reported claims stay distinct. A socket close or cancellation acknowledgement is not proof of refunded cost. Providers without enforceable per-call bounds still reject hard aggregate ceilings before sending.
- Root cancellation stops admission, removes queued children and joins owned work. Child-only cancellation cannot implicitly cancel a sibling. Shell/MCP process capacity and filesystem mutation serialization belong to the root; hosts still own OS containment and external-writer isolation.
- Provider/exporter keys never enter model-visible specs, Skill resources, MCP environments, remote messages or serialized records. Credentials resolve privately from approved host sources; fixture servers use synthetic secrets. Scope MCP and A2A credentials separately to their selected server/endpoint.
- Tool catalogs and trusted instruction prefixes stay stable within a run. Newly activated Skill instructions enter at a documented message boundary; child overlays follow shared instructions. Ordered configured fallback owns transport retries; one bounded context-overflow recovery and output repair are separate counted operations. None replays completed tools or restarts the task. Dynamic catalog churn, adaptive model routing and general parallel tool dispatch remain deferred.
- All supported settings enter the typed deployment option inventory with defaults, merge behavior, source provenance and redacted fingerprints. Host/deployment ceilings survive file/profile/environment/CLI precedence. Unknown or unimplemented options fail before side effects; configuration is snapshotted per run.
- Every introduced operation is observable natively when no exporter is attached. Metadata-only OTel remains the default, preserving exact native span IDs and lifecycle timestamps. Use one logical MCP tool span, child/run parentage, remote proxy spans and explicit handoff links; do not manufacture duplicate spans by translating traces after execution.

## Contract ownership

The owner below must check current official sources, record immutable revisions/digests where available, and freeze concrete request/result shapes, numeric bounds, error disposition, policy identity, cancellation ownership, schema/version and fixture expectations before implementing the slice. Live model/profile selection is verified when used; a planning URL is not an immutable protocol pin.

| Owner | Decisions to freeze before implementation |
| --- | --- |
| C3.1 | [Frozen schema/inventory](c3-deployment-config.md): version, typed option ownership, imports/profiles, deterministic merge/precedence, environment/secret refs, bounds, lock/authority, canonical identity/provenance, cross-interface projections and migrations |
| C3.3 | Implement the C3.1 resolved contract across CLI/ACP/embedding, validate/explain/render, canonical fingerprints, authority ceilings and per-run snapshots; prove legacy and explicit-config behavior |
| C3.4 | Literal-command parsing subset, exact executable/argv and supported prefix matching, defaults/deny precedence, ambiguous syntax rejection, stable decision IDs and launcher/descendant distinction |
| C3.5 | Provider-selection/credential precedence for CLI/ACP/Rust, explicit model defaults, shared versus gateway-owned fields, trusted endpoint handling, OpenRouter candidate profile and requested/resolved capability metadata |
| C3.8 | [OR01 pinned contract](c3-open-responses.md): Open Responses HTTP/SSE subset, item/event ordering and continuation retention, configured capability resolution, usage/finish/errors, unsupported content and bounded state |
| C3.10 | Named model profiles/ordered route, sticky selection, capabilities/continuation compatibility, attempt limits/error eligibility/delivery certainty and a single retry owner |
| C3.12a | Provider context capacity/estimation, one summary allowance, complete-turn retention and atomic replacement, overflow classification/recovery, private continuation, shared budgets and redacted compaction events; see [scope](../evidence/c3-compaction-scope.md) |
| C3.13 | Draft 2020-12 keyword/vocabulary and `format` behavior, local-reference/recursion/work limits, schema digest/cache, JSON parsing, output validation envelope and generic ACP projection |
| C3.14 | Exactly one same-run repair, bounded feedback, accounting admission, repair-visible events and typed exhausted/invalid outcomes |
| C3.15 | Official MCP SDK/protocol/convention pins, configuration provenance, exact qualified identity mapping, startup/catalog limits, selected text/structured result support, output-schema validation, progress and transport ownership |
| C3.19 | Agent Skills revision, explicit root discovery, metadata/path/scan bounds, qualified identity and duplicate rules, instruction/resource provenance and digests |
| C3.20 | Explicit activation/resource access shape, prefix-stable context insertion, resource read authority and digest checks, ordinary shell execution for scripts |
| C3.21 | Child identity/action union, typed ACP metadata/handlers, root ledger and child counters, aggregate process/MCP/trace/queue limits, depth one, two active children, authority intersection, shared workspace semantics and cancellation order |
| C3.24 | Inline versus artifact-reference handoff, schema digest/validation state, workspace/revision/byte limits and trace linkage without transcript copies |
| C3.26 | A2A 1.0 immutable pin and reference implementation, one JSON-RPC HTTP/SSE binding, cards/endpoints/Parts/tasks/Artifacts, remote uncertainty and optional W3C metadata extension |
| C3.29 | TUI invocation/composer behavior, bounded presentation state, terminal-safe text, one run per submitted task, cancellation/exit/non-TTY framing |
| C3.31 | Doctor's offline operations, provenance/redaction, selected explicit connectivity probes, version display and stable actionable errors |
| C3.32 | Complete subsystem option inventory, versioned deployable presets, clean/noisy environment equivalence, migration examples and native-platform configuration proof |
| C3.33 | Public compatibility matrix, established ACP namespace ownership, schema/capability versions, migrations, limits/privacy/fallback and minimum parser conformance corpus |
| C3.34 | Four build targets, minimum OS/libc, archive/install/replace/remove behavior, checksums/notices, artifact/source identity and offline CI runner requirements |
| C3.39 | Additional in-memory ACP/A2A timing boundaries, matched C2 baseline and sample method; no speculative performance thresholds |
| C3.40 | Concrete prerelease version/channel/assets and accurate capability claims, selected publication action, download/install verification and remaining release gates |

## Release boundary

C3 promotes extensibility, interoperability apart from Otto, and release hardening from the backlog. The initial distribution matrix follows design section 27.1: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`. Each has its own native acceptance checkpoint; available C2 evidence covers only the first and third. Missing native access is a future concrete acceptance blocker, not a reason to pass an emulated or cross-compiled artifact. Host architecture virtualization is disclosed separately from cross-architecture emulation.

The section 29.1/31.1 Otto proof remains required for the complete 0.1 contract and is intentionally absent from this cycle. A synthetic integration fixture validates C3 composition but cannot be relabelled Otto. Hard live token/cost guarantees also remain unsupported for profiles without actual enforceable bounds; neither a second gateway nor children repair that evidence gap. Prerelease notes must state these limits. Stable beta.1/0.1 completion cannot be inferred from a completed C3 or a successful publication.

Packaging, checksums, installer testing and reviewable release assets are authorized future development work under this plan. A tag/publication action is selected separately after accepted artifacts and release notes exist, preserving C1/C2 practice. C3.40's publication gate cannot pass on draft preparation alone. This planning session performs no build, live call, integration or publication.

## Upstream planning references

Inspected 2026-09-09 UTC. The brief and this cycle choose scope; these official sources inform contract details. Implementation owners pin audited revisions before depending on them.

- [OpenRouter streaming](https://openrouter.ai/docs/api_reference/streaming): keepalive comments, an accounting frame that can repeat the finish reason, mid-stream failures, and provider-dependent cancellation. P02 must exercise these distinctions; cancellation does not establish a universal billing bound.
- [Open Responses specification](https://www.openresponses.org/specification): items, semantic events and state transitions inform OR01/OR02. C3 selects HTTP/SSE text/function tools; optional WebSocket and broader content remain outside the selected subset.
- [MCP transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) and [MCP tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools): stdio and Streamable HTTP are separate transport gates; HTTP can return JSON or SSE, and disconnect differs from protocol cancellation. The dated revision is planning input, subject to audited SDK compatibility at C3.15.
- [Agent Skills specification](https://agentskills.io/specification): portable YAML-frontmatter/Markdown packages and progressive disclosure inform discovery/activation. Optional author metadata cannot grant host capabilities.
- [A2A specification](https://a2a-protocol.org/latest/specification/): select and pin the 1.0 binding/card/task/artifact subset at C3.26; the mutable `latest` URL is not a build pin.
- [ACP v1 extensibility](https://agentclientprotocol.com/protocol/v1/extensibility): use negotiated metadata/extensions for missing semantics while preserving generic peers. Existing ACP locks remain authoritative until a deliberate compatibility change.
