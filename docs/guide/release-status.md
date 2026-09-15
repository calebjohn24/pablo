---
title: Release status
description: What is implemented, what remains before release, and which claims are deliberately not made yet.
---

# Release status

Pablo `v0.0.1` is being prepared as the first formal release. The four exact candidate archives are publicly available as an installable preview through `runpablo.pages.dev`, but they have not completed the full native release matrix and no release tag has been cut.

## Implemented today

- A bounded streamed Rust model/tool lifecycle with typed outcomes and cancellation.
- CLI streaming, single-envelope JSON output, a basic TUI and stable ACP v1 process integration.
- Vercel AI Gateway, OpenRouter and explicitly configured Open Responses endpoints.
- Named model profiles, ordered fallback, context compaction and reasoning controls.
- Filesystem read/search/list, revision-checked write/edit and shell execution under static policy.
- Local final-answer JSON Schema validation with one optional bounded repair.
- MCP stdio and Streamable HTTP tools, portable local Agent Skills, supervised local children and remote A2A tasks.
- Composable versioned deployments, locked override policy, offline inspection and diagnostics.
- Bounded native JSONL traces, OpenTelemetry mapping/export and incoming W3C context.
- Compatibility fixtures and a seeded non-coding knowledge-work benchmark.

## Remaining release delivery

The selected release cycle still requires:

1. Pass native acceptance of the exact preview artifacts on all four targets, including CLI, Rust embedding, ACP, TUI, Collector and bounded live-provider proofs.
2. Record matched release measurements across accepted targets.
3. Cut the formal tag and verify clean downloads and installs across every advertised target.
4. Complete the cycle handoff.

Cross-compilation may prepare an archive but does not count as native acceptance. Published artifacts must be the exact artifacts exercised by the platform gates.

## Current limitations

- The checksum-verifying preview archives are public, but they are not yet an accepted tagged release and no package-manager formula is published.
- The source packages are not published to crates.io.
- Each direct CLI invocation and ACP session is one fresh task; durable chat/job state belongs to the host.
- The workspace is not an OS sandbox. Shell commands execute with the host account’s permissions.
- Live gateway adapters do not attest enforceable hard aggregate token or cost ceilings.
- Remote cancellation cannot prove a service rolled back work.
- The TUI is a basic terminal client, not a durable conversation product.
- Current protocol/config revisions are prerelease and may change before 0.1.
- The complete 0.1 product contract still retains a deferred Otto integration proof outside the current C3 cycle.

## Version and compatibility

Pin the exact source revision when evaluating Pablo. Use `cargo build --locked` and preserve `Cargo.lock`. Hosts should validate the native task schema and negotiate the ACP extension version rather than accepting unknown revisions.

The release profile uses thin LTO, one codegen unit and stripped symbols. [Installation and release files](./installation.md) records minimum OS/libc requirements, the unsigned/not-notarized macOS limit, deterministic archive layout, manifests, checksums, replacement and removal policy.

## Follow progress

The repository keeps current task state in [`docs/project/state.json`](https://github.com/calebjohn24/pablo/blob/main/docs/project/state.json), durable decisions in [`docs/project/brain.md`](https://github.com/calebjohn24/pablo/blob/main/docs/project/brain.md), append-only work history in [`docs/project/log.jsonl`](https://github.com/calebjohn24/pablo/blob/main/docs/project/log.jsonl), and the selected plan in the [C3 release cycle](https://github.com/calebjohn24/pablo/blob/main/docs/project/cycles/003-extensibility-and-release.md).

Those project records are development evidence, while the guides in this directory are the user-facing documentation.
