# R01–R03 — Configured A2A client

C3.26 is in progress. The implemented core currently validates cards and retrieves
public cards through bounded, no-redirect HTTP. Deployment definitions and RPC
credential scoping are implemented; configured fetch/proxy identity integration
and full R01 acceptance remain pending. Task execution
and cancellation belong to C3.27/C3.28; no A2A capability is advertised yet.

## Immutable protocol and reference

Use [A2A v1.0.0](https://github.com/a2aproject/A2A/tree/173695755607e884aa9acf8ce4feed90e32727a1),
including its normative `specification/a2a.proto` and `docs/specification.md`, and
[Python SDK 1.0.2](https://github.com/a2aproject/a2a-python/tree/eb37091fcd6411b3b01481ea3fd3e001c6fb55c0)
as the independent reference. [Fixture lock](../../../tests/fixtures/a2a/lock.json)
records commits/content hashes; mutable `latest` is not a build pin. Generated
[wire examples](../../../tests/fixtures/a2a/wire.json) come from SDK protobuf JSON.

Select protocol `1.0` and binding `JSONRPC`. Send `A2A-Version: 1.0`; method names
are `SendMessage`, `SendStreamingMessage` and `CancelTask`. Message requests wrap
`message` plus optional `configuration`/`metadata`. Send results wrap exactly one
`message` or `task`; SSE data frames wrap JSON-RPC envelopes whose results contain
exactly one `task`, `message`, `statusUpdate` or `artifactUpdate`. Use v1.0 Part
oneof fields and protobuf enum strings, not v0.3 `kind` discriminators or lower-case
role/state aliases. Some examples in the pinned specification still mention 0.3;
the normative v1.0 types and independent SDK determine this selected wire profile.

## Host authority and card admission

The host selects an exact HTTPS RPC endpoint and explicit card URL. Cards cannot
redirect either selection, choose credential sources, widen input, activate local
Skills/tools or modify local policy. Public-card GETs carry no inferred credentials;
HTTP redirects fail, including same-origin redirects. The isolated fixture hook
can replace transport only with literal loopback HTTP after validating original
host URLs. No broad discovery, authenticated extended-card discovery, server
listener, OAuth flow or remote file URL fetching is introduced.

Validate at most 64 KiB of card bytes within the earlier of the caller deadline or
ten seconds. Require a unique matching `supportedInterfaces` entry with `JSONRPC`,
`1.0`, no tenant and the exact host endpoint. Remote name/description/version are
bounded display data. Card digest is SHA-256 of received bytes, not a claim of JWS
verification or agent identity. Unknown optional metadata grants no authority.
Malformed/ambiguous cards, required unknown extensions and unsupported security
requirements reject before any task submission. A configured bearer scheme name
must match the card's HTTP Bearer declaration; scoped credentials themselves are
not card data. `options.a2a.remotes.<name>` contains required `card_url` and
`endpoint`, optional `bearer = { scheme, credential }`, and `trace_context`
(default false). Remote names use 1–128 ASCII letters/digits/underscore/dot/hyphen;
at most sixteen definitions are admitted. Defaults contain no remote definitions.
Each optional `authority[].a2a_remotes` map restricts exact named card/RPC URL
pairs; all layers intersect and an empty map denies every remote. Ordinary
configuration cannot widen a host layer. Credential declarations use the separate
`a2a.bearer` consumer and remain subject to `credential_ids` ceilings. Resolution,
rendering and card-byte admission do not look up secrets. Prepared runs resolve a
credential only by configured remote name and expose it only for that RPC endpoint
and consumer, never for the card URL or another provider. Public-card-only
retrieval intentionally excludes authenticated card discovery.

The optional selected trace extension is `urn:pablo:a2a:tracecontext:v1`. It is
understood only with explicit host opt-in, matching card declaration and no unknown
parameters. Unknown optional extensions are ignored; required unsupported ones
fail. The later task binding will carry only validated W3C traceparent/tracestate,
without baggage or local policy, in negotiated extension metadata. Remote metadata
must never reparent the local run. Task/stream implementation must prove this before
R03 acceptance.

## Mappings selected for execution checkpoints

- A local `remote_a2a` proxy gets an owned local agent ID. Remote context, task,
  message and artifact IDs remain distinct opaque bounded strings. Card digest,
  selected protocol/binding and endpoint identify the admitted transport. Remote
  IDs never become a local registry lookup or authority source.
- Only explicitly delegated task/context becomes a user Message. Local trusted
  instructions, full transcripts, tool catalogs, Skill bodies, credentials and
  policy do not transfer implicitly. Parts support bounded text/structured data;
  raw bytes and URLs are unsupported and never fetched automatically.
- Immediate agent Message results require their remote context and message IDs.
  Task results and subsequent status/artifact events must retain the same context
  and task identity. Submitted/working are nonterminal; completed requires a valid
  bounded result. Failed/rejected/canceled remain distinct terminal dispositions.
  Input/auth-required remain explicit interruptions, without automatic resubmission.
- Preserve artifact ID and append/last-chunk semantics within the selected result
  bound. No file-store or concurrent-write-isolation claim follows from an artifact.
- Cancel a known remote task with `CancelTask`; local transport cleanup and remote
  cancellation acknowledgement are different observations. Disconnection never
  proves rollback or stops remote work by itself. No automatic reconnect/replay.
- Keep standard JSON-RPC errors and local transport/bounds failures separate.
  A2A -32001 through -32009 have their pinned standard meanings; untrusted error
  messages/details must not enter private diagnostics or change local authority.

Remaining R01 work must freeze request/stream/result/identifier/time/error and
trace-extension bounds alongside configuration and reference-server rejection
fixtures before C3.27 starts. Current card tests alone do not satisfy R01.
