# A01–A04 — Temporary local children

## Scope and activation

C3.21 freezes the contract and extracts shared stable ACP v1 handlers; it does not
install a model tool or advertise working delegation. C3.22 admits one active child,
C3.23 admits two, and C3.24 adds validated handoffs. Persistent agents, descendants,
external ACP processes, automatic retry, graph scheduling and A2A are outside this
slice. `options.children.enabled` must be explicit before any child can execute;
absence means disabled. Unsupported enabling configuration must fail before work.

One root owns temporary depth-one local ACP children. They use the existing runtime,
provider fallback, tools, accounting and cancellation path. In-memory dispatch passes
official ACP request/response/update/cancel structs directly to the same handlers
used by stdio; the SDK's JSON-valued Channel is not this path. Stdio alone owns wire
framing, JSON-RPC IDs and physical-write acknowledgements. Native trace serialization
and JSON-valued extension fields are not a serialization of the ACP dispatch envelope.

## Identity and ownership

`AgentRef` records globally unique `agent_id`, immutable `root_run_id`,
`root_session_id`, optional immutable `parent_agent_id`, kind (`root` or
`local_acp_temporary`), depth, lifecycle state and optional ACP `session_id`.
A root has depth zero and no parent; a local child has depth one and exactly the
root agent as parent. The runtime assigns IDs; model arguments cannot choose,
replace or reparent them. ACP session identity differs from agent and model IDs.
An admitted handle is returned immediately, including for queued work; its session
ID may remain absent until ACP new-session admission completes.

States are queued, starting, running, stopping and settled. Settled retains a typed
`RunOutcome`, accounting, validation metadata and trace identity. Stop is idempotent;
settled state never rewrites a completed result as rollback. A root-owned handle
lookup rejects foreign/unknown IDs before inspection, wait or stop. No child has
access to another root's registry. Child state expires with the root; no resume,
reparent, named persistence or post-root background execution is implied.

## Operations and results

The model-facing built-in is `subagent`, one action union:

- `spawn`: one bounded task, optional task overlay, explicitly selected context,
  narrowed capabilities/model route/budgets and optional output contract. Return
  an `AgentRef` immediately after atomic admission, or a typed admission error.
- `wait`: a nonempty unique list of owned handles, mode `any` or `all`, and a bounded
  timeout. Return settled snapshots and remaining handles. `any` includes all
  already-settled selections at observation; `all` waits until every selection
  settles. Timeout returns current state and does not cancel work. Wait holds no
  model, process or active-child permit. Ordering follows requested handle order.
- `inspect`: one owned handle, returning a bounded snapshot: state, identifiers,
  counters, safe error and terminal result if available. No transcript is injected.
- `stop`: one owned handle; signal cancellation, remove queued work if applicable,
  and await its owned shutdown before returning settled state. It does not stop a
  sibling or root. A bounded wait timeout is available through `wait`, not by dropping
  a live stop/join operation.

Equivalent host operations call the same supervisor admission/state functions.
They do not manufacture tool calls or spend model tool-call allowances; the child
work they admit spends the same root budgets and capacity. A model `subagent` call
is itself an ordinary policy-checked, accounted tool call. Children cannot call
`subagent` at depth one. Tool calls remain sequential within each agent; returned
handles allow two child loops to overlap without general parallel tool dispatch.

## Bounds and root ledger

Initial fixed ceilings (hosts may narrow; zero active/total capacity disables):

| Resource | Ceiling |
| --- | --- |
| Tree depth | 1 |
| Active children | 2 at C3.23; 1 at C3.22 |
| Total admitted children per root | 16, including settled children |
| Pending queue | 14, FIFO and cancellation-aware |
| Selected child task + overlay + attachments | 1 MiB per child, 4 MiB queued total |
| Child working context | 8 MiB, also within the root's aggregate context bound |
| Aggregate resident model context | Root `max_context_bytes`, 32 MiB by default |
| Returned child result | 64 KiB per child, 1 MiB retained per root |
| Wait selection | 16 distinct owned handles |
| Wait timeout | 0–900,000 ms, further bounded by remaining root deadline |
| Owned shell + stdio MCP processes | 16 across root and children |
| Live MCP sessions | 16 across root and children, including HTTP |
| In-flight model operations | Root plus two children, maximum 3 |
| Pending multiplexed native events | 8, each within existing 32 MiB event bound |
| Events and trace bytes | Existing root limits shared across the whole tree |

These are additions for enabled supervision, not new limits on existing disabled
single-agent runs. Counts of model/tool calls and filesystem work retain unlimited
defaults. Existing byte/time ceilings still apply. Child limits may only narrow
root policy. Every operation deadline is the minimum of root, agent and operation
allowances; the root deadline starts once, not anew per child or fallback attempt.

Use one mutex-protected root ledger with per-agent counters and owned reservation
IDs. Check-and-reserve is atomic across root and siblings: model/tool admission,
active/total/queue/context capacity, process and MCP permits, event/trace capacity,
and attested token/cost upper bounds cannot be independent reads of remaining
allowance. A multi-resource admission succeeds wholly or changes nothing. Never
hold its lock during provider, process, filesystem, event sink or ACP work.

Native event counts reserve the root terminal when its ledger is created. Each
executed agent claims a terminal slot before its run span; model/tool/compaction
operations reserve start and finish together before opening. Already reserved
closings remain deliverable after admission closes. Failed delivery attempts stay
spent; only unused reservation slots are released. Root multiplex delivery order
is separate from this capacity counter.

Traced trees install immutable root capture/byte policy before execution or child
registration. Full or redacted JSONL is counted through the writer's borrowed
projection outside the lock, then event count and byte use are admitted atomically.
Untraced trees skip serialization and byte admission. Root terminal bytes are
prepaid at configuration; executed children reserve their own terminal allowance
before their run span. Settlement replaces that allowance with actual projected
bytes, including after admission closes. Unused terminal allowances are released;
consumed bytes remain spent after failed delivery. Operation payloads still must
fit ordinary trace space. One tree writer remains open through child terminals
and closes on the native root terminal. The eventual multiplex projection must
include any added sequence/envelope bytes in admission before delivery.

Retained model context uses the existing encoded instructions/history/tool catalog
measure, including opaque continuation bytes. Roots reserve retained context and
stream growth atomically; children meter growth within their full admitted context
reservation. Tool output must fit before becoming retained history. Compaction
covers its request while retaining prior history, then reduces the reservation
after accepted replacement. Advisory remaining capacity can guide compaction;
it never substitutes for atomic admission.

Reserve before external delivery or process start. Settle each reservation once
against actual usage; release unused capacity only after owned work joins. Call
counts remain spent for admitted attempts, including fallback and compaction.
Unknown usage remains unknown. C2's attested upper-bound reservations are retained
on uncertain delivery; missing usage cannot become zero and children cannot turn
unsupported live hard ceilings into supported ones. Each event has per-agent
accounting; root totals include own and child work exactly once. Child terminal
records do not spend usage again. No replay on cancellation, lost updates or retry.

An agent exhausted by its own ceiling settles without cancelling a sibling. Root
exhaustion stops new admission; existing owned operations settle under previously
reserved capacity. Root cancellation stops admission, drains queued children,
signals active work, wakes blocked producers and joins all children/processes/MCP
sessions before the root terminal event. Normal root completion also joins or
cancels outstanding children; no child can outlive the root. A stop/failure of one
child must remain independently observable and must not poison sibling tokens.

## Authority, context and shared workspace

Effective authority intersects host/root, parent and child restrictions. Child
requests select only exact inherited tool identities, Skill roots/selections, MCP
server/tool definitions and credential scopes. They cannot install servers, add
ambient roots, substitute same-named profiles, expose credentials or change trusted
instructions. A child can explicitly activate an approved Skill without loading
its body into the parent. Each child owns its admitted immutable registry and
MCP authentication/catalog lifetime; it must not reuse a consumed MCP registry.

Model routes are inherited by default. An allowed override is an ordered exact
subsequence of the parent's approved entries, preserving profile identity,
capabilities, endpoint/credential scope, fallback bounds and uncertain-delivery
rules. Child options may narrow, never erase, command/tool/filesystem policies and
root ceilings. Denied provider/tool/Skill/MCP/path selection fails before dispatch.

Only shared workspace mode is enabled in this slice. Filesystem mutations across
root and children share one cancellation-aware root serialization gate and retain
revision checks. Shell writes and external writers cannot be made transactional by
this gate; hosts own OS isolation. No artifact store or snapshot promise is added.

Children receive the trusted shared prefix, selected bounded task/overlay and
explicit attachments, not the parent transcript. Capability descriptions and Skill
instructions follow the stable-prefix contracts. Parent context receives bounded
snapshots/results only on explicit operations. Compaction preserves ownership,
handles, pending work and completed handoff identities outside summarized text.
A04 later validates schema-constrained output or workspace-relative path/revision
references before downstream delivery; unsuccessful upstream work is never passed
as a successful result automatically.

## Attribution and typed dispatch ownership

Native events carry immutable agent/root/parent IDs plus the source ACP session.
Each child run span is causally parented by its spawn operation, with stable links
for explicit handoffs; model/tool spans remain children of the executing agent's
run. Root multiplexing assigns one monotonic delivery sequence while preserving
per-agent sequence and original span/timestamps. Filtering is a projection, not a
second lifecycle. Task/context/resource content stays absent from OTel.

Scoped native records and ACP correlation metadata expose an `agent` object:
`agent_id`, `root_run_id`, `root_session_id`, nullable `parent_agent_id`, `kind`,
`depth` and executing `session_id`. The root ledger binds a registered agent once
before its run span starts. Incoming trace metadata and decoded event projections
cannot register or reparent agents. Unscoped runs omit the object. Metadata-only
JSONL retains it. Span-start attributes use `pablo.agent.id`, `pablo.root.run.id`,
`pablo.root.session.id`, `pablo.agent.session.id`, `pablo.agent.kind`,
`pablo.agent.depth` and, for children, `pablo.parent.agent.id`.

Typed dispatch must retain one-prompt sessions, setup errors, update ordering,
request cancellation, safe fallback and exactly one terminal response after joined
work. A bounded typed update receiver acknowledges consumption before another
notification is admitted; direct dispatch cannot inherit unbounded SDK queues.
Dropping/closing a host receiver cancels and joins owned work. Lifecycle code is
shared; direct mode adds no socket, JSON envelope, provider loop or independently
implemented session state machine. C3.21 A01 tests must exercise full prompt,
updates and cancellation, not merely initialize/new-session calls.
