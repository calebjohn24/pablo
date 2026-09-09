# C2 acceptance fixtures

These are frozen acceptance specifications, not executed tests or evidence of implemented behavior. Owning checkpoints implement them with real temporary files, the existing scripted/offline gateway fixture and capture sinks. C2.5 repeats relevant cases on the required platforms and adds live/Collector proof. Expected behavior comes from the [contracts](../contracts/c2-single-agent.md); [the cycle](../cycles/002-single-agent-completion.md) assigns gates.

## Shared synthetic inputs

Each case gets a new temporary workspace W and a sibling outside directory O. No fixture reads the repository `.env` or user task data. Strings below use JSON escapes for exact UTF-8 bytes. `H(text)` means lowercase SHA-256 of those exact bytes, computed by the fixture independently of the tool. Default registered capabilities are reads only unless a case explicitly grants writes/shell. Time is controlled with fixture barriers instead of races or arbitrary sleeps.

| Path under W | Exact bytes / type |
| --- | --- |
| `a.txt` | `"alpha\nbeta alpha\n"` (17 bytes) |
| `unicode.txt` | `"Aé🙂Z\n"` (9 bytes) |
| `nested/b.txt` | `"alpha\r\nnone\n"` |
| `nested/c.txt` | `"ALPHA\n"` |
| `empty.txt` | empty regular file |
| `.hidden` | `"alpha\n"` |
| `binary.bin` | bytes `00 ff 61` |
| `internal-link` | symlink to `a.txt` |
| `outside-link` | symlink to O |
| O/`outside.txt` | `"outside sentinel\n"` |

Tests may add dedicated directories/files as described. On unchanged inputs sort by UTF-8 path bytes. Bound assertions must measure retained work/results, not only inspect a final truncation flag. All hard-limit cases assert no next model call/tool dispatch after termination. Every denied/mutation-failure case checks original bytes and the outside sentinel, not only an error label.

## C2.1 filesystem reads

| ID | Input / setup | Required result |
| --- | --- | --- |
| F01 | `fs.read {path:"a.txt"}` then scripted final response based on returned bytes | Text exactly `"alpha\nbeta alpha\n"`, offset 0, size 17, revision H(original), next offset null, truncated false; same call ID/result in next model request |
| F02 | Read `unicode.txt` with `max_bytes:4`, then offset 3/max_bytes 4, then offset 7/max_bytes 4 | Pages `"Aé"`, `"🙂"`, `"Z\n"`; next offsets 3, 7, null; stable revision and size 9. Offset 2 or offset 3/max_bytes 1 returns recoverable `invalid_range`; offset 9 returns empty EOF; offset 10 invalid |
| F03 | Dedicated `listing` directory with files `b`, `a`, subdir `c`, link `d`; list with max_entries 2 and offsets 0, 2, 4 | Entries a/b then c/d with correct file/directory/symlink types; next offsets 2/null; offset 4 empty; offset 5 invalid. Never resolve d; symlink target absent from result |
| F04 | Search W with query `alpha`, max_matches 2; repeat uncapped at 100 | First two matches `.hidden`:1=`alpha`, `a.txt`:1=`alpha`, truncated true. Full result additionally `a.txt`:2=`beta alpha`, `nested/b.txt`:1=`alpha`, truncated false. No binary, symlink, uppercase or outside matches; literal `a.*` finds none |
| F05 | Missing file, directory-as-read, file-as-list, binary read, invalid UTF-8 name; malformed fields/zero max_bytes/multiline query | Closed recoverable codes `not_found`, `not_file`, `not_directory`, `unsupported_encoding` as applicable; next model receives errors. Invalid schemas terminate with invalid-tool-arguments; no filesystem I/O after validation rejection |
| F06 | Read absolute O path, `../O/outside.txt`, `internal-link`, `outside-link/outside.txt`; swap an intermediate directory for an outside symlink at a barrier before that component is opened | Workspace/no-follow policy denial, no outside data opened or returned. A prefix sibling W-other never matches W. Normal absolute W/a.txt succeeds. A previously opened directory handle may retain its safe original identity; no fallback to canonicalize-then-open |
| F07 | Set file cap 16 bytes then read 17-byte a.txt; set entry cap 2 then list 3 entries; depth cap 1 with nested depth 2; scan cap below total scanned bytes; result cap 1,024 with an oversized matching line/JSON-escaped content | Each hard work limit returns `filesystem_work`; serialized-result overflow returns `tool_output_bytes`. Assert incremental byte/entry/allocation bounds, bounded error metadata and no apparently complete result. A file growing beyond its cap after initial stat also stops |
| F08 | Create FIFO/device-type fixture where supported; deny read subtree `nested`; create unreadable in-policy file; cancel large traversal at fixture barrier | No blocking read of special files. Explicit nested path denied; W search skips its descendants. Unreadable in-policy file returns `io_error` (permission fixture must actually deny access). Cancellation joins all owned work, emits terminal once and starts no next model call |

## C2.2 mutations

Mutations explicitly enable writes and grant required roots. Capture directory contents before and after each failure to find leaked temporary entries.

| ID | Input / setup | Required result |
| --- | --- | --- |
| M01 | Write `new.txt`, text `"new\n"`, expected_revision null; repeat with text `"changed\n"` | First creates mode 0600, size 4, H(new bytes), created/committed true. Second returns conflict and preserves `"new\n"` |
| M02 | Replace a.txt with `"new\n"`, expected_revision H(original), existing mode 0640; repeat stale revision; repeat against missing path with non-null revision | First preserves mode and atomically installs exact bytes, created false. Stale returns conflict; missing returns not_found; no temporary leftovers |
| M03 | Edit a.txt, H(original), old_text `"beta"`, new_text `"gamma"`; separately edit original with old_text `"alpha"` or `"absent"` | Success yields `"alpha\ngamma alpha\n"`, preserving every other byte. Two/zero matches give match_not_unique and preserve original. Empty old_text invalid before execution |
| M04 | Change target to `"external\n"` at the barrier before final revision recheck; separately create target before create-only commit | Conflict; external bytes preserved. State explicitly that a writer racing after replacement recheck is outside the optimistic guarantee; no test claims external-writer compare-and-swap |
| M05 | Cancel before commit; cancel immediately after successful commit; inject write/rename/cleanup failure and oversized content | Precommit cancellation leaves original intact; postcommit tool result says committed true and resulting bytes remain even if run is cancelled. Write/rename failures preserve original and remove temp; injected cleanup failure is explicit. Oversized input never truncates original |
| M06 | Write/edit without write capability; denied root; missing parent; symlink target/parent; replace one name of a hard-linked file | Denial/not_found as applicable with original/outside bytes intact and no implicit mkdir. Hard-link alias retains old bytes after successful replacement; document metadata limits and verify no partially written destination observed by a controlled reader |

## Shared lifecycle and transport

| ID | Input / setup | Required result |
| --- | --- | --- |
| L01 | Model → filesystem tool → model via core and ACP fixture; cover success, recoverable error and denied dispatch | Stable tool catalog and bijective gateway alias; matching call IDs; one start/finish pair for a dispatched tool, no shell.started, run-child tool span with native IDs and exact timestamps; one terminal outcome |
| L02 | Put unique synthetic markers in path/query/content; default native capture, then explicit capture; inspect OTel | Default native trace excludes all content markers and revisions but retains counts/codes/correlation; explicit native capture includes permitted payloads. OTel excludes content in both modes. Compare redacted size accounting with actual serialized bytes |
| L03 | ACP cancellation and stalled/disconnected consumer during filesystem work; two sequential sessions in one process with different workspaces/caps | Bounded transport, physical-write acknowledgement and joined cleanup remain. No events/content/cancellation/accounting leak to session two; first text remains immediate and setup remains reusable. A committed mutation stays visible in its tool result, never silently rolled back |

## C2.3 machine output

| ID | Input / setup | Required result |
| --- | --- | --- |
| J01 | Offline CLI task returns text containing LF, quotes, Unicode and literal JSON; use `run --json`; provider reports input_tokens 9007199254740993 and output_tokens 2 | Exactly one parseable object plus LF, no progress on stdout; outcome output is original string, not parsed model JSON. Correct non-null IDs, null error; accounting counters are `"1"`/`"0"`, input/output `"9007199254740993"`/`"2"` survive normal JSON.parse exactly; unknown cache fields null; exit 0 |
| J02 | Invalid flag/config, unavailable synthetic credential, trace setup failure, all with recognized --json | Exactly one pre-admission envelope, null IDs/outcome/accounting, corresponding closed error code; exit 2; no credential/path/config/task text and no provider request |
| J03 | After a completed model/tool turn trigger cancellation, timeout, policy denial, provider failure, output limit; separately close stdout | Exit 130 only for cancellation, otherwise 1; native outcome and all-outcome accounting match core/ACP. Unknown usage stays null. Closed stdout produces delivery failure with no second outcome attempt |
| J04 | Compare `pablo "TASK"` and `pablo run "TASK"`; generic and envelope-capable ACP peers; large escaping-heavy final output | Equivalent defaults/outcome. Envelope metadata only when negotiated; standard ACP still works. Checked output/frame bounds include escaping and metadata; no accumulated stream transcript or compatibility break hidden by native success |

## C2.4 policy

Policy examples below use ordered `{id,value}` rule arrays and the contract's defaults. IDs refer to configuration decisions, not human-readable task data.

| ID | Input / setup | Required result |
| --- | --- | --- |
| P01 | tools allow `{id:"allow.read",value:"fs.read"}` and deny `{id:"deny.read",value:"fs.read"}`; then remove deny and dispatch fs.list | Deny wins with deny.read. Nonempty allowlist rejects unmatched fs.list with built-in allowlist-miss ID. No side effect; absence from registry is still denied even if a rule allows it |
| P02 | read_roots allow `nested`, deny `nested/private`; add `nested/private` and `nested-other` directories; test matching paths | nested/b.txt allowed with configured ID; private denied with deny ID; nested-other never matches nested. First matching deny wins when two denies apply. Traversal hides denied descendants |
| P03 | executables deny `/bin/sh`; then allow only `/bin/sh`; request shell.run | Deny prevents process creation. Allow evaluates only launcher; no assertion that command contents/descendants are constrained. Sanitized environment and shell cleanup remain unchanged |
| P04 | CLI and ACP use same policy; duplicate IDs, unknown fields, >1 MiB file, outside root; exact tool rule `fs.*`; write flag plus explicit write deny | Invalid config rejected pre-delivery; `fs.*` never matches `fs.read`. Valid configurations produce equivalent rule IDs across surfaces. Session arguments/write flag cannot override explicit host denial |

## C2.4 accounting

The attested scripted profile has per-call token bound 10 and cost bound 8 micro-USD unless a case overrides it. These are synthetic test values, not live model pricing. Token actual is input+output; cache counters are not extra tokens.

| ID | Input / setup | Required result |
| --- | --- | --- |
| A01 | Omit count/aggregate caps and script 6 model calls with 5 tools; separately set model_calls=0 or tool_calls=0 | No hidden four/two-call ceiling. Zero model cap dispatches none and reports zero actual usage. Zero tool cap executes no tool and retains existing provider final-answer hint behavior |
| A02 | Total token cap 15, per-call bound 10; first actual input 3/output 3/cache-read 2; next request needed | Reserve 10 then settle charged tokens 6, remaining 9; deny second dispatch with total_tokens limit since 10 exceeds 9. Actual input/output/cache totals remain 3/3/2; model_calls 1 |
| A03 | Cost cap 10, per-call bound 8; first reported cost 3; next request needed | Reserve 8 then settle charge 3, remaining 7; deny second dispatch with cost limit. No rounding, price lookup or automatic retry |
| A04 | Both ceilings enabled; first response reports unknown usage/cost, or cancellation/failure occurs after possible delivery | Retain reservations 10/8; actual unknown fields null; no conversion to zero. Definitely-not-sent failure instead releases charge and reports known zero actual, while model_calls stays 1. Completed earlier-call usage survives a later failure |
| A05 | No attested adapter bound with a requested ceiling; zero ceiling; actual 11 tokens against bound 10; checked-add overflow fixture | Unsupported profile rejects pre-delivery. Zero admits no positive reserve. Violated bound returns accounting_bound_violated and no further dispatch. Arithmetic never wraps; unrepresentable actual fields become unknown without releasing reservations |
| A06 | Task one consumes/reserves budget; task two in same ACP process starts independently; cancellation at pre-dispatch barrier | Fresh counters/actuals/reservations per task, no stale budget/cancellation. Pre-dispatch cancellation consumes no count/reservation. Every terminal envelope and native metadata agree; ledger never changes immutable settled outcomes |

## C2.5 integration gates

Run the applicable fixtures on macOS arm64 and native Linux x86_64, with OS-specific cases explicitly identified. Use real Collector export to verify filesystem native/span correlation and default content privacy. A small opt-in live Vercel ACP task must read synthetic files and return a result derived from them, with explicit call/time/output caps and private credentials/traces. Live usage without attested bounds cannot count as hard-ceiling proof. Record C1.7-compatible fingerprints/methods and new filesystem workload boundaries. No runtime fixture in this document is marked passed by C2.0.
