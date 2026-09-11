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

S02 adds explicit activation, selected resource loading, stable instruction prefix,
context accounting and joined cancellation. `options.skills.activate` remains an
explicit unsupported C3.20 feature; discovery cannot activate a Skill implicitly.
