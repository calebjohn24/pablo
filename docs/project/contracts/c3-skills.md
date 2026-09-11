# S01–S02 — Local Agent Skills

## Portable metadata pin

Pin the [Agent Skills specification and reference validator](https://github.com/agentskills/agentskills/tree/69ef37e9424c0a7ea9dd2293b559e43ec8176379)
at `69ef37e9424c0a7ea9dd2293b559e43ec8176379`. A package is a directory with
`SKILL.md`: YAML frontmatter followed by Markdown. Required name and description
and optional license, compatibility, metadata and allowed-tools retain their
portable meanings. Names use the reference validator's Unicode NFKC normalization,
lowercase alphanumeric/hyphen rules, 64-character limit and parent-directory match;
consecutive or leading/trailing hyphens reject. Descriptions allow 1–1024 characters;
provided compatibility is nonempty and at most 500 characters. Arbitrary additional
fields belong in the metadata string map. Allowed-tools is descriptive and grants
no authority. No proprietary manifest is required.

Pablo parses YAML with pinned yaml-rust2 0.11.0, encoding features disabled, and
unicode-normalization 0.1.25. Scalars remain strings as in the reference's StrictYAML
loader. Quoted, multiline literal/folded and Unicode text are accepted. Reject
anchors, aliases, tags, sequences, nested metadata values, duplicate keys and
multiple documents. Unknown top-level fields reject. Delimiters must be complete
`---` lines, with LF or CRLF; only the YAML prefix is decoded and hashed. These are
explicit host parser restrictions, not a claim to support every YAML feature.

## Roots, identity and authority

C3.19 adds `options.skills.roots`, a map of explicit host source IDs to portable
Path references. Defaults are empty. `.agents/skills/` works when explicitly
selected; do not inspect user/home/workspace roots by convention alone. Root IDs
are 1–32 ASCII letters/digits/underscore/dot/hyphen. Resolution remains offline and
never scans roots or resolves credentials. `authority[].skill_roots` optionally
bounds physical root containment; every present layer intersects, and an empty
list denies all roots. Task-workspace rebasing must recheck these ceilings.

Core `skills::discover` accepts only host-selected roots, cancellation and a
monotonic deadline. It returns roots, sorted qualified identities `ROOT/NAME`,
source-relative SKILL.md paths, parsed metadata and frontmatter SHA-256 identities.
The digest identifies metadata only; S02 owns instruction/resource hashes. Physical
root aliases are diagnosed rather than scanned twice. Same-root normalized name
collisions remove all conflicting entries. Cross-root short-name collisions retain
qualified entries, report ambiguity and require qualification. Never pick one by
scan order. Diagnostics carry fixed codes/root/relative location, not raw YAML or
OS error text.

Open root components and descendants without following symlinks, using directory
handles, pre-open type checks and post-open file-descriptor checks. Reject traversal,
symlinked roots/packages/files and nonregular metadata files. One level of package
directories is scanned; scripts, nested roots and resource directories are not
walked. No install/download, model call, resource loading or execution occurs.
Concurrent hostile filesystem mutation still requires host sandbox/isolation;
these operations are not a general filesystem sandbox.

## Resource limits

Fixed discovery limits: 16 roots, 4096 directory entries total, 256 admitted Skills,
16 KiB frontmatter including delimiters, 1 MiB aggregate consumed metadata and
64 diagnostics. YAML permits at most 256 parser events and 64 metadata pairs;
metadata keys/values are bounded at 256/4096 bytes. Root paths are at most 4096
bytes. Diagnostic relative locations are omitted above 512 bytes. Aggregate limits
fail explicitly without returning a truncated catalog. Invalid individual packages
produce bounded diagnostics. Enumeration is capped before sorting; cancellation
and the deadline are checked during enumeration and metadata reads. No detached
worker is created. Local synchronous filesystem calls remain host-owned.

`pablo skills list --config PATH` prints the complete metadata catalog as JSON;
`pablo skills show ROOT/NAME --config PATH` prints one metadata record. Unique short
names also resolve. Both are offline and require explicit configuration. Discovery
has a five-second deadline; invalid package diagnostics yield exit 1, while setup,
lookup and aggregate failures yield exit 2 before output. Ambiguous short names
can be inspected by qualified name. A small bounded input buffer may prefetch bytes
past the frontmatter delimiter, but body bytes are neither parsed, retained in the
catalog nor added to model context. A body edit leaves discovery identity unchanged.

## Explicit activation and selected resources (S02)

`options.skills.activate` selects at most eight names from configured roots; its
default is empty. CLI run and ACP process configuration accept repeatable
`--skill NAME` with `--config`. This replaces the configured selection and remains
subject to locked deployment override policy. Unique short names resolve as in
discovery; qualified identities sort activation deterministically. Repeated aliases
for the same identity reject. Discovery alone never activates instructions.

The asynchronous capability factory rechecks roots against the actual task
workspace, discovers metadata, then reads only selected SKILL.md files. Each file
is bounded to 256 KiB, with 1 MiB aggregate across selected files. Frontmatter must
still match its discovery digest. Activation records identify metadata and the
complete SKILL.md bytes with separate SHA-256 digests. Instruction byte counts
measure the Markdown body; catalog byte counts measure the qualified name plus
description actually included in context. These counts are source bytes, not
token estimates or complete serialized message sizes.

Bodies are ordinary user context after the original task, with an explicit reminder
that they grant no authority. Shared system instructions stay unchanged. Compaction
preserves this original task/activation prefix and summarizes subsequent history;
the ordinary context estimator includes the full serialized messages and tool
catalog, including wrappers. Resource context enters only through requested tool
results and is eligible for ordinary task-relevant compaction. Fixed instructions
cannot be silently discarded to make an oversized context fit.

An activated set enables `skill.read` (provider alias `skill_read`). The model must
select an exact activated `ROOT/NAME`, relative resource path and optional
`max_bytes`. The reader retains the admitted package directory handle, opens each
component without following symlinks, and accepts regular UTF-8 files only. Absolute
paths, parent traversal, backslashes, symlinks and special files reject. Reads are
bounded to 1 MiB, further narrowed by the requested byte limit, ordinary tool output
capacity and deadline. Results contain actual text, source path, SHA-256 and byte
count; repeated calls read current bytes without a cache. JSON escaping may make a
result exceed the output budget even when raw bytes fit; that returns output limit.

`authority[].skill_roots` bounds this separate read capability; filesystem tool
`read_roots` governs filesystem tools. Configured tool authority must admit
`skill.read`, and ordinary runtime tool policy can deny each call. Instructions and
allowed-tools metadata cannot install tools or MCP servers, change write roots, or
widen any authority. Bundled scripts execute only when the model explicitly calls
the existing permitted `shell.run` lifecycle, with its usual workspace, launcher,
environment, timeout and cleanup policy.

Activation and resource reads each await an owned blocking worker, including after
cancellation. Chunk reads check cancellation/deadlines before and after I/O. There
is no detached file operation; a host filesystem call must return before joining
can finish. Immutable package handles live with the activated registry. CLI and ACP
prepare a fresh set for each task rather than retaining instructions in a shared
provider cache. Activation failure/cancellation during admission returns a safe
setup error before `run.started`; resource cancellation uses the admitted run's
ordinary terminal lifecycle.

Native `skill.activated` carries the activation record. Instruction content is
absent unless native capture is explicitly enabled. CLI text mode reports the
qualified identity on stderr; JSON mode retains one terminal envelope. ACP clients
opting into both `pablo/v1` and `pablo/skills-v1` receive `_pablo/skill` with native
correlation and capture-gated instructions. Metadata-only resource events omit
text and the requested resource path. OTel records activation count/catalog/body
bytes on the run and resource digest/bytes on the tool span, never bodies or
resource content. Native instruction digests and tool resource digests distinguish
the loaded sources without inventing token usage.
