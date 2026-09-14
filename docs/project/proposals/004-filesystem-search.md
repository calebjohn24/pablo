# Proposal: more efficient and useful filesystem search

Written 2026-09-14 at the user's request. This is an implementation proposal, not an adopted cycle or completed acceptance claim. [State](../state.json) remains authoritative; C3.34 is active for the existing release-documentation work and packaging remains held. Selecting this work later requires adding its checkpoints to the cycle specification and state, without replacing historical records. D022, D024, D038 and D060 constrain the design.

Improve `fs.search` along two axes: reduce memory and avoid unnecessary filesystem work; help the model retrieve the right evidence with fewer searches and less returned text. Start with an internal scanner improvement, then add explicit search controls. Keep the native tool in the existing Rust runtime, with no required external executable or persistent index.

## Current implementation and constraints

The implementation is in [filesystem.rs](../../../crates/pablo-core/src/filesystem.rs), principally `read_text`, `entries`, `search` and `walk`. The [filesystem contract](../contracts/c2-single-agent.md#shared-filesystem-contract) and [tests](../../../crates/pablo-core/tests/filesystem.rs) define the behavior to preserve.

| Current behavior | Consequence | Proposed change |
| --- | --- | --- |
| Reads each candidate file completely into a `Vec`, validates it, then searches its lines | Memory grows with file size even when nothing matches | Stream bytes and retain only bounded candidate results and scanner state |
| Calls `line.contains(query)` for every line | Repeated matching setup and line traversal are potential CPU costs; speedup is not yet measured | Compile one literal matcher per call and reuse buffers |
| Reopens descendants from the workspace root, traversing their ancestors again | Deep trees can repeat path checks and open/stat work | Profile this separately; reuse a current directory handle where the containment contract is preserved |
| Walks all permitted entries, including hidden and ignored files | Generated trees consume I/O and can crowd out useful results | Explicit include/exclude globs, hidden-entry selection and optional local ignore rules |
| Supports one case-sensitive literal and returns full matching lines | Different spellings require more calls; matches often need a follow-up read | Explicit case/regex controls, file-only results and optional surrounding context |
| Returns the first 100 matching lines by default; no continuation | Broad queries can return a truncated lexical prefix | Make narrowing straightforward; keep truncation honest and defer cursor design |

The current [measurement harness](../../../scripts/measure-filesystem.ts) exercises approximately 1 MiB across 100 files with 100 matches. It measures tool and ACP prompt duration, but does not characterize large-file memory, selective traversal, long lines, or output usefulness. Existing C3 timing observations in the [backlog](../backlog.md) remain historical; they do not establish which search operation is slow.

Preserve default literal/case-sensitive behavior, sorted depth-first traversal, one result per matching line, 1-based lines, LF splitting with one trailing CR removed, and the existing payload shape. Keep no-follow workspace access, policy precedence, binary/invalid-UTF-8 handling, explicit host work quotas, complete serialized-result limits and joined cancellation. D038's unlimited default file/entry/depth/scan quotas must not become incidental restrictions during optimization. `fs.read` and mutation snapshot/revision behavior are outside this change.

## S1 — Freeze behavior and establish a useful baseline

Proposed dependency: explicit adoption of this proposal. Assign actual checkpoint IDs during adoption; S1–S5 are planning labels only.

- Preserve the current scanner as a test oracle or equivalent fixture implementation, isolated from production dispatch. Compare exact payloads, ordering, status and quota outcomes on unchanged synthetic trees.
- Add deterministic corpora for many small files, deep/wide directories, a 256 MiB short-line file, one huge nonmatching line, a huge matching line, dense/sparse/no matches, early/late matches, Unicode and mixed binary/text. Include a generated tree where at least 90% of bytes can be explicitly excluded.
- Exercise UTF-8 characters and queries across chunk boundaries, LF/CRLF and missing final LF, duplicate occurrences on one line, exactly N versus N+1 matches, invalid bytes/NUL at EOF after apparent matches, unreadable permitted files, symlink swaps, denied subtrees and precise quota boundaries.
- Extend the existing offline measurement approach with separate direct-core and ACP runs. Record tool duration, whole prompt duration, CPU, peak process RSS, scanner-retained bytes, files opened, bytes read, entries visited, serialized result bytes and descriptor high-water mark. Keep internal benchmark counters out of default public schemas and content-free traces free of paths/patterns.
- Save a baseline release executable with its source, binary and harness fingerprints. No paid provider calls are needed. Use the current tool in baseline scripts only; new-option workloads need an independent expected-result fixture.

Acceptance: fixtures have reviewed expected outcomes, the harness verifies results before timing them, and saved baseline reports cover every corpus. Do not claim improvements from the existing 1 MiB timing alone.

## S2 — Stream literal search without changing its answers

Proposed dependency: S1.

Extract a private search module under `crates/pablo-core/src/filesystem/`. Leave workspace admission, policy decisions, read/write snapshots, runtime dispatch and tool identity owned by their current modules. Keep one joined blocking worker per call initially.

1. Read a reusable fixed-size buffer. Benchmark 8, 32 and 64 KiB buffers; choose from matched measurements. Incrementally validate UTF-8, retaining an incomplete trailing codepoint across reads. Track line numbers and CRLF boundaries across chunks.
2. Compile the literal query once. Evaluate an explicitly pinned direct dependency on the already-locked `memchr` 2.8.3; its `memmem::Finder` supports reusing one substring searcher across inputs. Measure it against the current standard-library matcher before attributing a speedup. [Upstream API](https://docs.rs/memchr/2.8.3/memchr/memmem/struct.Finder.html).
3. Bound retained line data by the remaining result capacity. An arbitrarily long nonmatching line must not allocate its entire length: retain a query-overlap tail and enough line data to return a result if it fits. Once a line cannot fit, continue matching/validation with bounded state; a matching oversized line becomes a pending output-limit result. Preserve matches crossing chunks and matches near the end of huge lines.
4. Stage each file's results until EOF. Today a NUL or invalid UTF-8 byte anywhere discards every match from that file. Roll back staged matches and their byte accounting if validation fails. Defer a candidate output-limit failure until whole-file validation succeeds. I/O errors and hard work limits still take precedence when the current implementation would encounter them before validation.
5. Preserve the extra-match rule: `truncated=true` requires a validated N+1th matching line. Reaching N, finding an extra provisional match, or recognizing binary data does not permit abandoning the current file's required read/validation and quota checks. After the current file is resolved, avoid visiting later files when truncation is established. This preserves semantics but means streaming alone will not reduce bytes read from an ordinary eligible file.
6. Count escaped result bytes incrementally, including framing and policy metadata, and perform the final complete-result check. Continue honoring cancellation between bounded operations and join the worker before `tool.finish`.

Target retained scanner memory is O(buffer + query + result capacity), independent of total file size and longest nonmatching line. Sorted directory frames still retain directory entries; do not describe total traversal memory as constant. Memory measured through the full runtime also includes result/event/history copies.

After the streaming comparison, profile deep-tree path access. If repeated ancestor opens are material, use parent-relative no-follow opens and type checks through a small bounded handle cache, with safe reopening on eviction. Do not retain one open descriptor per traversal depth or impose a new depth ceiling. Treat directory rename/symlink races as an explicit compatibility review; omit this optimization if preserving containment would require a broader host-isolation contract.

Acceptance: all legacy oracle cases agree; 32–256 MiB file scaling and long-line cases demonstrate bounded scanner retention; quota/error precedence, cancellation and path-race tests pass. Publish CPU, memory and latency independently, including workloads with no improvement.

## S3 — Let callers avoid irrelevant files

Proposed dependency: S2.

Add optional fields with backward-compatible defaults:

| Field | Default | Proposed meaning |
| --- | --- | --- |
| `include` | `[]` | Empty accepts all eligible files; otherwise match any include glob |
| `exclude` | `[]` | Reject files or subtrees matching an exclusion; exclusion wins |
| `include_hidden` | `true` | `false` skips dot-prefixed descendants |
| `respect_gitignore` | `false` | Apply permitted `.gitignore` files from the search directory and its visited descendants |

Globs are case-sensitive, use `/`, and are relative to the requested search directory. Define `*` as within one component and `**` as spanning components; `**/*.rs` includes both direct and nested Rust files. Reject absolute patterns, `..` components and NUL. Propose at most 64 patterns per list and 4,096 bytes per pattern, still subject to the existing argument cap. Compile each pattern set once. Evaluate `globset` with these explicit options instead of inventing a glob parser. [Upstream syntax](https://docs.rs/globset/latest/globset/).

Apply policy before selection. A caller filter can only narrow permitted files. Count enumerated entries under the existing traversal quota even when filtered out. Prune an excluded subtree before opening its children. An include pattern that does not match a directory's own name is not sufficient to prune it: its descendants may match. Validate filenames without lossy conversion. Filtering saves content reads; it does not retroactively remove enumeration work.

Use the `ignore` crate only as a matcher for explicitly supplied lines. Open `.gitignore` through Pablo's permitted no-follow handles, then feed `GitignoreBuilder::add_line`; do not hand filesystem traversal or file opening to the library. It supports parsing supplied lines without requiring its path-based file reader. [Upstream builder](https://docs.rs/ignore/latest/ignore/gitignore/struct.GitignoreBuilder.html#method.add_line).

Freeze nested-rule precedence, comments/escapes and negation in fixtures. Ignore rules start at `path`; do not read ancestors above it, global Git configuration, `.git/info/exclude`, `.ignore`, or files outside the workspace. Do not imply full `git status` equivalence. Load control files independently of content globs/hidden filtering, but only when policy and no-follow checks allow them. Missing/denied/symlinked control files contribute no rules; malformed, unreadable or unsupported-encoding permitted control files produce a bounded error rather than silently using a partial matcher. Charge their bytes and compilation work, with explicit parser resource bounds frozen at adoption. Includes cannot override an ignored directory; callers can disable ignore processing. Preserve ordinary `.git` traversal unless the caller hides or excludes it.

Acceptance: fixtures prove include/exclude precedence, safe subtree pruning, hidden behavior, nested ignore negation and denied/symlinked control-file handling. Identical calls with new options omitted retain old answers. On the generated-tree corpus, at least 90% fewer content bytes are read with the explicit exclusion and all in-scope expected results remain present.

## S4 — Improve matching and evidence returned to the model

Proposed dependency: S3. Ship the additions together only after the contract and compatibility fixtures cover them.

| Field | Default | Proposed meaning |
| --- | --- | --- |
| `case_sensitive` | `true` | Optional Unicode simple case folding; preserve original returned bytes |
| `regex` | `false` | Explicit line-local regular expression; literal remains the normal path |
| `mode` | `"lines"` | `"files"` returns unique paths containing a content match |
| `context_lines` | `0` | 0–10 complete lines before/after each retained matching line; lines mode only |

Evaluate the already-locked `regex` 1.13.1 family with one compilation per query and bounded pattern size, nesting, compiled representation and search state. Its documented syntax omits lookaround and backreferences; case-insensitive matching uses Unicode simple folding. Use these semantics explicitly, with no locale-dependent matching or Unicode normalization. [Upstream behavior](https://docs.rs/regex/1.13.1/regex/).

Keep the existing query-byte and single-line input restrictions. Apply patterns independently to LF-delimited lines after CR removal, including the existing empty final segment. Patterns may match empty strings and still emit each matching line only once. Inline regex flags follow the pinned engine's syntax. Unsupported syntax/compile bounds fail through the documented argument/limit path; no raw pattern goes into default traces. An ordinary literal with `case_sensitive=false` must escape regex metacharacters if it uses the regex backend.

The regex adapter must preserve S2's long-line memory and cooperative cancellation behavior. Evaluate incremental automaton execution with bounded state rather than buffering unlimited lines or calling an uninterruptible matcher on a giant line. Validate Unicode boundaries and anchors across chunks against a reference whole-line matcher. This is a separate go/no-go gate: if the adapter cannot meet those constraints, ship files/context first and keep regex/case folding deferred; do not quietly cap previously accepted line sizes.

Keep the exact existing result when all new options are omitted. In files mode, use a separately specified search-files payload containing `path`, `files` and `truncated`; `max_matches` limits unique files and the extra result must come from another validated file. It reduces returned text and matching work after the first hit, but still validates that file to EOF. It is content search returning filenames, not filename-pattern discovery.

For context requests, retain the usual `matches` and add one deduplicated, sorted `context` array of `{path,line,text,is_match}` for neighboring lines around retained hits that are not already in `matches`. `max_matches` limits match anchors, never context lines. A neighboring hit beyond that limit can appear as context with `is_match:true`; it still establishes `truncated:true` and does not expand the context window. Finalize trailing context before settling the file. Bound all retained context by the complete result budget and fail explicitly if it cannot fit. Full lines remain complete; snippet clipping is deferred. Define `truncated` solely as omitted match anchors, never silently missing requested context.

Example proposed call after S4:

```json
{
  "path": "crates",
  "query": "ToolRegistry",
  "include": ["**/*.rs"],
  "exclude": ["**/generated/**"],
  "include_hidden": false,
  "case_sensitive": false,
  "context_lines": 2,
  "max_matches": 20
}
```

Add model-facing examples explaining when to use file-only discovery, a narrow directory/glob, regex alternatives or context. Evaluate six offline retrieval tasks: find a symbol, find alternate spellings, distinguish source from generated copies, gather a definition with surrounding lines, identify files containing a setting, and narrow a truncated search. Verify required evidence deterministically. Any live-model comparison needs a separately selected bounded run with fixed model/settings and repeated attempts; elapsed model time cannot establish scanner performance.

Acceptance: exact expected evidence for all tasks; unchanged default results; explicit Unicode/regex/chunk-edge semantics; one files-mode call returns the same relevant path set with at least 80% fewer serialized result bytes on a frozen dense-match fixture. Context tasks return the required surrounding lines in one call. Report actual downstream token counts only if measured with a specified tokenizer/provider; bytes are not tokens.

## S5 — Prove compatibility and measured improvement

Proposed dependency: S4, or the explicitly reduced S4 scope recorded at its go/no-go gate.

Update tool input schemas/descriptions, Rust payloads, native contracts, provider catalogs, CLI/TUI projection, ACP fixtures and documentation together. Follow D060 and the [ACP extension registry](../../acp-extensions.json): additive requests do not automatically make new result variants compatible with frozen clients. Keep legacy payload/catalog projection for old negotiations or require a new negotiated capability/revision before admitting new result modes. Choose and test the concrete version mapping at S1 adoption, before exposing new fields. Advance only affected project-owned contracts; do not change upstream protocol pins to fit a local extension.

Use existing deployment fields for work quotas; register any newly necessary host matcher resource controls in typed configuration, defaults, render/explain and fixtures. Keep declared capabilities and policy decisions consistent across all interfaces. Preserve one start/finish pair, closed failures, content redaction and worker ownership.

Run the locked Rust workspace tests and clippy, formatting, TypeScript typecheck, affected CLI/ACP/provider/compatibility and telemetry fixtures, plus project checks. In particular retain `cargo test --locked -p pablo-core --test filesystem`, `cargo test --locked -p pablo-core --lib filesystem` and the existing filesystem measurement workload. Add adversarial tests for the new behavior, rather than tests of documentation wording.

Use release builds on matched native macOS arm64 and Linux x86_64 hosts; list other advertised target checks separately. Follow [measurement methods](../../measurements.md): five warmups and 30 measured samples, reversed/alternating baseline-candidate batches, no overlapping builds/tests, unchanged corpora and saved source/executable/harness fingerprints. Measure warm cache explicitly; label first-run timings separately and do not call them cold-cache results without controlling cache state. Report p50/p95, CPU, peak RSS, scanner retention, syscall/open counts where available and binary-size changes. Retain failures and publish sanitized evidence; raw reports stay ignored.

Proposed review targets, to freeze against the S1 baseline before implementation:

- At least 80% lower incremental peak RSS on the 256 MiB short-line no-match corpus, plus deterministic scanner-memory bounds on huge-line cases. Measure matched isolated processes and subtract an idle baseline consistently.
- At least 25% lower native tool p50 on the generated-tree selective workload, in addition to the 90% content-byte reduction. Compare independently verified results for the intended selected scope; do not label scope reduction a like-for-like engine speedup.
- Default-option p50 and p95 regressions above both 10% and 0.5 ms on any frozen existing workload trigger investigation and repeat alternating comparisons. Unresolved regressions must be explained and explicitly accepted before promotion.
- File-only response size and retrieval correctness meet S4's gates. Query/response schema growth, build size, startup and unrelated filesystem timings are reported as costs, not hidden behind search-only metrics.

These numbers are proposed acceptance targets, not observed results or new CI performance ceilings. Retain an optimization only where its measured benefit justifies its complexity. A missed target is recorded and investigated; do not move the goalposts after inspecting results.

## Scope and handoff

The first useful delivery is S1–S3: measured baseline, streaming literal search and explicit selection. S4 adds richer retrieval once its contracts and scanner behavior are proven. Finish each adopted checkpoint's verification and project handoff before starting the next.

Defer parallel file scanning, persistent indexes/watchers, semantic embeddings, ranking, filename-only discovery, multi-query batches, count-only mode, search cursors, multiline regex and clipped long-line snippets. Parallelism must first justify descriptor/memory costs and preserve deterministic results and quotas; indexing needs an invalidation and policy-isolation contract; cursors need defined behavior under file changes. None is necessary to deliver the first measured improvement.

Planning verification on 2026-09-14 inspected current source, schemas, tests, dependency pins and measurement methods and checked the linked upstream APIs. No search implementation, speed comparison or new feature acceptance has run under this proposal. Next action for this proposal is explicit adoption of S1 with the final contract/fixture matrix; current release work remains governed by state and its existing hold.
