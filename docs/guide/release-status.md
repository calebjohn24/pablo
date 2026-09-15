---
title: Version and support
description: Pablo’s current version, included capabilities, supported platforms and operational limits.
---

# Version and support

Pablo `v0.0.1` is available through the checksum-verifying installer at `runpablo.pages.dev`. Versioned archives support macOS and Linux on arm64 and x86_64, and the repository contains the same documentation, installer and source used by the published site.

## Included in v0.0.1

- A bounded streamed Rust model/tool lifecycle with typed outcomes and cancellation.
- CLI streaming, single-envelope JSON output, a basic TUI and stable ACP v1 process integration.
- Vercel AI Gateway, OpenRouter and explicitly configured Open Responses endpoints.
- Named model profiles, ordered fallback, context compaction and reasoning controls.
- Filesystem read/search/list, revision-checked write/edit and shell execution under static policy.
- Local final-answer JSON Schema validation with one optional bounded repair.
- MCP stdio and Streamable HTTP tools, portable local Agent Skills, supervised local children and remote A2A tasks.
- Composable versioned deployments, locked override policy, offline inspection and diagnostics.
- Bounded native JSONL traces, OpenTelemetry mapping/export and incoming W3C context.

## Supported platforms

| Platform | Architecture | Minimum runtime |
| --- | --- | --- |
| macOS | Apple silicon arm64 | macOS 11.0 |
| macOS | Intel x86_64 | macOS 10.15 |
| Linux | arm64 | glibc 2.35 |
| Linux | x86_64 | glibc 2.35 |

The macOS binaries are unsigned and not notarized. Linux packages target GNU libc; musl is not supported. [Installation and release files](./installation.md) documents archive contents, checksums, platform detection, upgrades and removal.

## Current operational limits

- The Rust crates are consumed from a pinned Git revision and are not published to crates.io.
- Each direct CLI invocation and ACP session is one fresh task; durable chat and job state belongs to the host application.
- The workspace is not an operating-system sandbox. Shell commands execute with the host account’s permissions.
- Live gateway adapters do not attest enforceable hard aggregate token or cost ceilings.
- Remote cancellation cannot prove that a service rolled back work.
- The TUI is a focused terminal client rather than a durable conversation product.
- Pablo acts as an A2A client for delegated tasks; it does not expose an A2A server endpoint.

## Version and compatibility

Pin the exact source revision when embedding `pablo-core`. Use `cargo build --locked`, preserve `Cargo.lock`, validate the native task schema and negotiate Pablo’s ACP extension version. Review constructors, event variants and configuration migrations when moving to another Pablo version.

The release profile uses thin LTO, one codegen unit and stripped symbols. The installer pins an exact version, verifies SHA-256 before extraction and validates the binary’s reported version before changing the installation prefix.

For source, issues and changes, use the [Pablo repository](https://github.com/calebjohn24/pablo).
