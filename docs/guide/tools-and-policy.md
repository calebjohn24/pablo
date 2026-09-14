---
title: Tools and policy
description: Safely configure filesystem tools, shell execution, limits and static host policy.
---

# Tools and policy

Pablo has no ambient tool authority in the core. A host constructs a catalog. The CLI enables shell and filesystem reads for convenience, while explicit flags and deployments can remove or constrain them.

## Built-in catalog

| Native name | Provider name | Capability |
| --- | --- | --- |
| `fs.read` | `fs_read` | Read a bounded UTF-8 page and revision |
| `fs.list` | `fs_list` | List one directory deterministically |
| `fs.search` | `fs_search` | Literal recursive UTF-8 search |
| `fs.write` | `fs_write` | Create or replace with a revision precondition |
| `fs.edit` | `fs_edit` | Replace one exact text match at a known revision |
| `shell.run` | `shell_run` | Run a noninteractive `/bin/sh -c` command |

Tool calls execute sequentially. Arguments validate against a closed JSON Schema before dispatch. Tool results are bounded again after JSON serialization so escaping cannot bypass the output limit.

## Filesystem reads

The runtime opens a canonical workspace handle at admission. Built-in filesystem operations use no-follow directory-relative access and reject traversal, symlinked targets, special-file reads and paths outside the workspace.

`fs.read` returns UTF-8 text, pagination metadata and a SHA-256 revision of the same bytes. Pages never split a UTF-8 codepoint. `fs.list` returns direct children in deterministic order. `fs.search` is recursive, literal and case-sensitive; it skips symlinks and binary files.

Read/list/search are enabled in CLI and ACP by default. Disable them with `--no-filesystem`.

## Revision-checked mutation

Writes require explicit authority:

```sh
pablo run "Update CHANGELOG.md from the release notes." --allow-write --no-shell
```

`fs.write` accepts `expected_revision: null` only for create-if-absent. Replacing an existing file requires its SHA-256 revision. `fs.edit` requires a revision and replaces exactly one literal occurrence.

Pablo prepares the complete result before touching the target, writes a private temporary file in the same directory, rechecks policy/revision/cancellation and installs atomically. A pre-commit failure leaves the original intact. Once committed, the result keeps `committed: true` even if cancellation arrives during cleanup.

This is optimistic conflict detection, not isolation from another process. Hosts that need a stable snapshot must isolate concurrent writers.

## Shell execution

The shell tool runs `/bin/sh -c` with closed stdin and a small clean environment:

```json
{
  "command": "wc -l README.md",
  "cwd": ".",
  "timeout_ms": 5000,
  "max_output_bytes": 8192,
  "env": { "PABLO_TASK_LABEL": "docs" }
}
```

Only environment additions beginning with `PABLO_TASK_` are accepted. Provider credentials, `HOME`, shell startup settings and trace context are not inherited. `cwd` must resolve inside the workspace.

The launcher receives a new process group. On completion, timeout or cancellation, Pablo kills remaining group members, reaps the leader and drains output before settling. Cleanup gets a separate bounded allowance. If cleanup cannot be proven, the outcome is an explicit tool-cleanup failure.

A permitted command still has the operating-system rights of the Pablo process. Static command policy is not a sandbox.

## Command rules in deployments

```toml
[options.shell]
enabled = true

[options.shell.commands]
default = "deny"
allow = [
  { id = "docs.git-status", executable = "/usr/bin/git", args = ["status"], match = "prefix" },
  { id = "docs.wc", executable = "/usr/bin/wc", args = [], match = "prefix" }
]
deny = [
  { id = "docs.no-force", executable = "/usr/bin/git", args = ["push", "--force"], match = "exact" }
]
```

Command parsing is literal rather than a general shell parser. Rules match the first simple command form supported by the contract. Dynamic shell constructs that cannot be proven against the rule are denied. Deny wins over allow, and a nonempty allow list denies unmatched commands.

Executable policy and command policy are separate. The shell launcher is `/bin/sh`; a command rule can constrain the literal command invoked inside it, but neither mechanism contains descendants once a permitted program starts.

## Static JSON policy

Legacy CLI tasks can use `--policy PATH` for exact tool, launcher and filesystem-root rules. Each rule carries a stable configured ID. Deny wins within every dimension and all host/authority layers intersect.

Policy can narrow a catalog but cannot create a tool that the host did not register. Likewise, `--allow-write` cannot override a denied write root.

## Limits

Default host limits include:

| Resource | Default |
| --- | ---: |
| Run deadline | 1 hour |
| Shell deadline | 15 minutes |
| Tool arguments | 1 MiB |
| Serialized tool result | 8 MiB |
| Request context | 32 MiB |
| Model output | 4 MiB |
| Native events | 1,000,000 |

Filesystem file bytes, entry count, search depth and scan bytes are unlimited by default in the CLI, but hosts/deployments can set explicit quotas. Response pagination and serialized result bounds still apply.

Model and tool call counts default to unlimited. A zero count disables that kind of call. Pablo never starts a tool when no model-call budget remains to consume its result.

## Cancellation and privacy

Cancel the shared token and await the task. Dropping the future triggers fallbacks but cannot promise joined cleanup or terminal event delivery.

Native traces omit tool arguments, paths and content by default while retaining operational counts, policy decisions and truncation metadata. OpenTelemetry never records task or tool content, even when native content capture is enabled.
