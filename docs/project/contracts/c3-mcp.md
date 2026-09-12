# M01–M04 — Tool-only MCP

## Pins and scope

Use official `rmcp = 3.3.0`, exact release commit
`3e636cab26c013eca5131103c03d20237f12c4df` (tag `rmcp-v3.3.0`). Pin
`2025-11-25` explicitly in initialize; reject a different negotiated version.
The configured protocol is explicit even if the SDK supports newer versions. Do not infer the
release behavior from the moving upstream main README or automatically select a
new discovery lifecycle. Tools-only client capabilities exclude roots, sampling,
elicitation, prompts, resources, durable tasks and subscription/catalog changes.
M02 enables stdio; M03 enables Streamable HTTP; M04 closes host/ACP and Collector
acceptance. M01 performs configuration/policy admission only and advertises neither
transport. M02 promotes the client-only SDK to a runtime dependency. M03 shares
normalized sessions/catalogs between stdio and Streamable HTTP. The Rust
embedding entry point is `PreparedRun::tools_with_mcp`; synchronous `tools()`
requires the async path for admitted MCP. C3.18 (M04) integrates this factory into
CLI and ACP, with a fresh catalog and credential resolution for every task.

## Host configuration and authority

`options.mcp` defaults to an empty server map and exact policy layers. At most 16
host-defined servers. Names are case-sensitive ASCII letters/digits/underscore/dot/
hyphen, 1–32 bytes. Each server is a closed typed stdio or HTTP record, with required
startup (default true), startup timeout 30 seconds (1–120 seconds), and operation
timeout 60 seconds (1–900 seconds). A root deadline always wins.

Stdio uses an absolute launcher, up to 64 arguments (4096 bytes each, 65536 aggregate), a host-selected cwd,
and at most 32 environment-name → credential-reference bindings. There is no PATH
search, inherited environment or command interpolation. HTTP uses a single HTTPS
endpoint without userinfo, query or fragment, and at most 16 lower-case header-name
→ credential-reference bindings. Host-selected loopback HTTP is fixture-only.
Protocol/routing headers cannot be overridden. Secrets are never literal fields;
MCP environment and header consumers are distinct from provider/exporter consumers
and are scoped to the server, destination and binding name. Resolve them only at
use, after admission. Inspection/rendering retain references and provenance.

Policy has exact `servers`, qualified `tools`, and absolute `launchers` dimensions.
Within every layer, deny wins over allow; nonempty allow lists restrict the set.
All layers intersect, including immutable `authority[].mcp` ceilings, existing tool/
executable policy and `authority[].tool_names`. Ordinary `options.mcp.policies`
remain separate when rendering. At most 16 combined MCP policy layers and 1024
unique rule IDs are admitted. Server records replace as a unit during composition;
old arguments or credential bindings cannot survive a replacement.
Only the host server registry defines possible destinations/launchers; an allow
rule cannot manufacture an unconfigured server. Required admission/startup failure
fails the run before model dispatch; optional failure omits the server/catalog and
records a bounded reason. Never retry or silently replace an optional server.

ACP requests may select exact host-approved server definitions and narrow tool
policy. Unknown names, duplicate names, changed command/args/endpoint, nonempty
client environment/headers, or a denied host definition reject before I/O. Host
credential references remain authoritative. Session cwd cannot install a launcher
or configuration file. Model, Skill, tool and workspace data cannot add servers,
credentials or authority. ACP advertises HTTP capability and accepts stdio; legacy
SSE transport remains unadvertised. Session creation performs offline admission;
prompt admission revalidates the selection before launch. Empty selections use
host defaults; nonempty selections narrow the configured server set.

## Qualified catalog and bounds

Canonical identity is `mcp/{server}/{tool}`. Raw tool names use the same ASCII
alphabet, 1–128 bytes. Names are case-sensitive and cannot contain separators.
Provider aliases are `mcp_` plus the first 24 SHA-256 bytes in lowercase hex (52
characters total). Store both directions and reject every duplicate identity or
alias collision, including collisions with already registered aliases. This is an
admitted-catalog bijection, not a mathematical claim that a truncated hash cannot
collide. Freeze the catalog for the admitted run; no tool-list updates mid-run.

Hard host bounds: 8 discovery pages per server; 64 tools per server and 256 total;
1 MiB catalog serialization; 4096 description bytes; 65536 bytes per schema using
J01's documented Draft 2020-12 subset, no external retrieval, annotation-only format.
Validate arguments before dispatch and declared structured output before admitting
results. Unsupported schemas or duplicate/colliding tools fail catalog admission.

At most one MCP request per server and 16 total outstanding; 64 progress messages
per operation, 1024 progress-message bytes and 8192 aggregate progress bytes.
Progress never extends the operation/root deadline. Bound each JSON-RPC frame to
2 MiB, request/result payload to 1 MiB, stderr metadata counting to 65536 bytes and session ID
/cursor to 256/1024 bytes. Bound all queues before reading more peer data. Runtime
context, tool-result, event and trace allowances can narrow these limits further.

Results preserve ordered text blocks and optional structured JSON, with explicit
`isError`. JSON-RPC errors are protocol failures; `isError=true` is a recoverable
tool failure. Image/audio/resource content and task-required execution are rejected
explicitly, never flattened or silently discarded. JSON schema output failure is
a typed invalid-result error; it is not final-model-answer repair. Structured
content must be an object. Success requires declared structured output; a tool
error may omit the success payload, but supplied structured content still validates.

## Transport ownership

Use the official SDK for initialization, messages and request correlation. The
pinned SDK's default async reader uses unbounded `read_until`, and its codec default
has no finite maximum. Supply a bounded transport/codec adapter before receiving
untrusted bytes; do not fork the protocol state machine. Review service pending
requests, progress timeouts and transport close paths at M02/M03. M02 sends one
typed request directly, bypassing the SDK's catalog cache and automatic multi-round
tool helper. It admits no server requests and never replays a tool invocation.

Stdio: one process group per owned server, cleared environment, bounded stderr and
stdout framing. Cancellation notifies the peer where possible, then closes, kills
and joins the process group/pipes under a 2-second cleanup allowance. Dropping a
service alone is not evidence that descendants or pipe readers have joined. Stderr
text is drained and discarded with fixed buffers; only a saturated byte counter is
retained. Cancellation interrupts transport reads/writes before joining the SDK
service, child and stderr task. The notification shares the original cleanup
deadline. Callers must drive cancellation/close to completion; Drop is a fallback,
not a joined-cleanup guarantee.

Each admitted MCP registry is single-use and rejects concurrent/repeated runs.
Startup is charged to the root deadline. Required startup failure closes all prior
servers; optional failures record fixed omission codes, and cleanup failure is
always fatal. An unused prepared catalog must be explicitly closed by its host.

HTTP: JSON and SSE responses, negotiated protocol/session headers, optional GET
and DELETE per the pinned transport specification. Disable redirects and automatic
invocation replay. Scope credentials to the exact endpoint. No stream resumption
in this cut; disconnect is an explicit failure and remote completion may remain
uncertain. Send bounded protocol cancellation and close local work without claiming
the remote effect was rolled back. Own clients/catalogs/credentials per admitted
run; any future process-level reuse must prove isolation at M04.

M03 uses one POST response stream per outstanding request and no standalone GET
stream: unsolicited server requests are outside the admitted tool-only subset.
Accept JSON and SSE, including bounded priming/heartbeat events. A frame is at most
2 MiB and an entire POST stream at most 4 MiB; progress retains the shared per-call
bounds. Every response ID must match its originating POST. Incomplete streams,
unrelated IDs, session changes and redirects fail explicitly. A 404 ends the current
session; callers must start a fresh admitted run, never replay the interrupted call.

The initialized session header is visible ASCII, 1–256 bytes; subsequent requests
carry it and the pinned protocol version. Peer headers never change the configured
endpoint. Private `mcp.headers` credentials are scoped to the exact server definition
and binding. HTTP clients disable proxies, redirects and automatic retries.
Loopback HTTP is an explicit host-only synthetic fixture override; ordinary host
configuration requires HTTPS. The CLI/ACP bootstrap requires configuration and a
synthetic provider endpoint, validates mappings before credentials/launch, and
cannot install servers. See the deployment contract for the exact flags.

Cancellation or disconnected uncertain calls attempt an explicit notification
directly on the same HTTP endpoint, because the SDK sender may be occupied by a
stalled POST. This best-effort notification uses at most 50 ms of the original
2-second cleanup allowance. Local transport cancellation joins the SDK and drops
response readers; an assigned session receives bounded DELETE. DELETE 404 means
already expired and 405 means teardown is unsupported. Notification/DELETE success
does not prove remote effects were rolled back. Tool results and metadata-only
telemetry retain `remote_completion_uncertain` when a dispatched call lacks a
correlated response. A valid correlated result clears that uncertainty; peer error
text remains private. HTTP session tokens are sensitive and excluded from telemetry.

## Telemetry

Use existing GenAI convention commit
`fee465db333bdd6a7d2faa320edab5cf3101a4f4`, including its MCP mapping. Propagate allowed
W3C context in `params._meta`. MCP client spans use method/target naming and CLIENT
kind, protocol/session/request IDs, status, transport and server address/port.
Decorate the ordinary logical tool span with MCP attributes; do not add a duplicate
logical execution span. Keep raw peer errors, credentials, args/results, headers,
stderr and progress text out of metadata-only telemetry; safe error codes/counts
replace peer messages. Native records retain source, deciding policy IDs, phase,
counts and uncertainty. M04 proves exact native/Collector correlation across CLI
and ACP for stdio, HTTP JSON and HTTP SSE using the independent pinned peer.

## Audited sources

- [Pinned Rust SDK](https://github.com/modelcontextprotocol/rust-sdk/tree/3e636cab26c013eca5131103c03d20237f12c4df): model version constants, client feature boundary, async framing, service cancellation/close and progress timeout paths. This is an integration-surface audit, not a blanket security certification.
- [Lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle), [transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports), and [tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) define the pinned protocol. Numeric host limits and narrower supported features above are Pablo choices.
- [Pinned MCP OTel mapping](https://github.com/open-telemetry/semantic-conventions-genai/blob/fee465db333bdd6a7d2faa320edab5cf3101a4f4/docs/gen-ai/mcp.md).
