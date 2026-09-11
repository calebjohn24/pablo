# R01–R03 — Configured A2A client

C3.26 supplies configured public-card admission, owned local proxy descriptions
and bounded wire codecs. Production supervised task transport/artifact assembly
belongs to C3.27; adverse cancellation and Collector proof belong to C3.28. The
presence of configuration or a proxy description does not claim task execution.

## Immutable protocol and reference

Pin [A2A v1.0.0](https://github.com/a2aproject/A2A/tree/173695755607e884aa9acf8ce4feed90e32727a1),
including its normative proto, and [Python SDK 1.0.2](https://github.com/a2aproject/a2a-python/tree/eb37091fcd6411b3b01481ea3fd3e001c6fb55c0).
[The fixture lock](../../../tests/fixtures/a2a/lock.json) records commits, source
hashes and the SDK wheel hash. [Wire vectors](../../../tests/fixtures/a2a/wire.json)
are generated through SDK protobuf JSON; the SDK's JSONRPC/SSE dispatcher supplies
independent HTTP conformance. Some pinned Markdown examples retain 0.3 aliases;
the normative v1.0 proto and independent SDK define this profile.

Select protocol `1.0`, binding `JSONRPC`, and header `A2A-Version: 1.0`. Use
`SendMessage`, `SendStreamingMessage`, and `CancelTask`; the first two wrap their
results, while CancelTask returns a Task directly. Parts use protobuf oneof fields
and roles/states use `ROLE_*` / `TASK_STATE_*`, without v0.3 kind discriminators.

## Host selection, credentials and Agent Cards

`options.a2a.remotes.<name>` requires HTTPS `card_url` and `endpoint`; optional
`bearer = { scheme, credential }` selects a declared HTTP Bearer scheme and private
credential reference. `trace_context` defaults false. Defaults contain no remotes;
at most sixteen names of 1–128 ASCII letters/digits/underscore/dot/hyphen are
allowed. URLs are bounded to 4096 bytes, without userinfo, query or fragment.

Every optional `authority[].a2a_remotes` map intersects exact named card/RPC URL
pairs; an empty map denies all remotes. Credential references also intersect
`credential_ids` ceilings. `a2a.bearer` is a distinct consumer: prepared runs resolve
only configured names and expose a credential only to that exact RPC endpoint.
Resolution/rendering/card-byte admission never look up secrets. Public card GETs
never carry credentials or follow redirects, including same-origin redirects.
No OAuth flow, authenticated-card discovery, broad discovery or runtime listener
is introduced. The explicit fixture override validates original host authority
before substituting literal loopback HTTP transport.

Read at most 64 KiB within the earlier of the caller deadline or ten seconds.
Reject an already-expired/cancelled request before network polling. Require one
matching interface with JSONRPC, 1.0, no tenant and the exact host endpoint.
Nonempty input/output mode lists are bounded to sixteen 128-byte media labels;
file-only and structured-data-only peers are supported. Labels and their parameters
remain descriptive data. Card name/description/version are bounded untrusted
metadata. Raw-byte SHA-256 is a content identity, not authentication or JWS proof.
Unknown optional extensions are ignored; unknown required extensions fail before
submission. Cards cannot select credentials, endpoints, tools, Skills, policy or
private local context.

## Local and remote identity

Named retrieval creates a fresh `AgentRef` of kind `remote_a2a` beneath a running
depth-zero root. It retains the selected card/endpoint/protocol identities but
performs no ledger registration or task submission. Remote context/task IDs start
absent, bind separately and cannot change once observed. Failed observations leave
both IDs unchanged. Neither ID becomes a local session or registry key. The root
ledger stores each registered kind and rejects event projections that substitute
a local-child kind or different execution identity.

## Parts, results and limits

Only explicitly selected Parts enter a new user Message; no full RunSpec, trusted
instructions, transcripts, tool/Skill catalogs, policy or credentials transfer
implicitly. Requests explicitly select accepted output modes and historyLength
zero. A returned nonempty history rejects. Text, structured JSON (including null),
bounded inline file bytes and inert HTTP(S) URL references are supported. Base64
accepts standard/URL-safe alphabets with or without padding; encoding uses standard
padded base64. Invalid encoding and excess decoded bytes reject. URL references
are never fetched, and filenames never become local paths. No file is opened or
written by the codec. Media labels remain metadata; the Part oneof determines how
content is represented. Unknown content variants reject.

Send responses contain exactly one Message or Task. SSE result envelopes contain
exactly one Message, Task, statusUpdate or artifactUpdate. Preserve remote IDs,
status dispositions, artifact IDs and append/lastChunk flags. Status-message
context/task identities must agree with their enclosing task. Submitted/working,
completed, failed, canceled, rejected and input/auth-required remain distinct;
execution and interruption handling are C3.27 responsibilities. No automatic
replay, reconnect, continuation or remote file fetching follows from decoding.
Remote-reported usage cannot replace enforceable local accounting.

| Resource | Bound |
| --- | --- |
| Explicit input content | 32 KiB total (decoded file bytes, UTF-8 text/URL, serialized JSON data) |
| One decoded inline file | 32 KiB |
| Inert HTTP(S) file URL | 4096 bytes; no userinfo |
| Encoded request | 256 KiB, allowing input escaping/base64 |
| One JSONRPC response or SSE data envelope | 64 KiB |
| Stream envelope bytes / updates | 1 MiB / 256 |
| Parts per Message/Artifact | 32, at least one |
| Artifacts per Task | 16, distinct IDs |
| Request/message/context/task/artifact IDs | 256 UTF-8 bytes, nonempty, no controls |
| Artifact name / description / Part filename | 256 / 8192 / 256 bytes |
| Accepted output modes / media label | 16 / 128 bytes |
| Remote error message | 1024 bytes, excluded from local diagnostics |
| Task wall time / stream idle | 900 s / 10 s, clamped to caller/root time |
| Cancellation exchange | 2 s cleanup window, or earlier host cleanup deadline |

Typed deserialization rejects duplicate known fields and ambiguous unions.
JSONRPC response ID/version must match the request. Local bound/shape/content
failures remain distinct from numeric remote JSONRPC/A2A errors. Unknown numeric
remote errors retain their code; their untrusted message/data grants no authority.
The codec enforces byte/count/content limits. C3.27 must additionally enforce raw
SSE framing bytes, wall/idle timers, assembled-result limits, root admission and
joined cleanup; constants alone do not prove those execution guarantees.

## Optional trace-context extension

Select `urn:pablo:a2a:tracecontext:v1` only with host opt-in and a matching card
extension declaration with no unknown parameters. Generic peers continue with
reduced correlation: send encoding omits extension metadata/declaration and trace
headers unless negotiation succeeds and an explicit validated local context is
provided. Card-required unsupported extensions still fail admission.

A negotiated send request carries `params.metadata[URI] = { traceparent,
tracestate? }`, `message.extensions = [URI]`, `A2A-Extensions: URI`, and matching
HTTP traceparent/tracestate headers. The same validated header profile can carry
cancel correlation after negotiation; no nonstandard CancelTask parameter is added.
No baggage, private policy or arbitrary metadata is copied into this profile.

This profile accepts W3C version-00 traceparent (55 lowercase ASCII characters,
nonzero trace/span IDs) and optional printable-ASCII tracestate (512 bytes, at most
32 unique valid members). Empty tracestate is omitted. Extension metadata is
bounded to 1024 bytes and its field set is closed. Invalid negotiated metadata
rejects. Unknown optional message extensions and unnegotiated trace metadata are
ignored rather than breaking base messages. Only validated negotiated correlation
is retained in a separate receipt; raw metadata is excluded from typed serialization.
Decoding never activates an OTel context or reparents the local run.

Independent SDK fixtures prove send/stream/cancel shapes, file/data/URL round trips,
negotiated headers/metadata and generic-peer fallback. Required-version rejection
occurs before handler invocation. Actual propagated remote spans, metadata-only
Collector export and exporter-outage parity remain C3.28 acceptance.
