---
title: Introduction
description: Understand what Pablo owns, what the host owns, and how a task moves through the runtime.
---

# Introduction

Pablo is a small headless Rust runtime for applications that use agents for non-coding knowledge work. It supplies the model-and-tool lifecycle. The application around it supplies product state, isolation, approval decisions and the user experience.

That split is the central design choice. A desktop editor can host Pablo through ACP, a terminal can run one task directly, and a Rust service can call the core API. All three use the same outcome, accounting, cancellation and telemetry model.

## Start from your application

Pablo is infrastructure inside your product rather than a complete end-user agent application. Choose the boundary that matches how your application is built:

| Need | Integration |
| --- | --- |
| A language-neutral, independently upgradeable child process | [ACP over stdio](./acp.md) |
| Direct in-process control from Rust | [`pablo-core`](./embedding.md) |
| A one-task command for evaluation, jobs or scripts | [CLI JSON output](./cli.md#json-mode) |

For most applications, install the native executable and begin with ACP. Your host creates the workspace, passes the selected credentials, consumes updates, handles approvals and commits validated results into product state. The [getting-started guide](./getting-started.md) walks through that path.

## Ownership boundary

| Pablo owns | Your host owns |
| --- | --- |
| Sequential model/tool execution | OS/container sandboxing |
| Typed run, model and tool events | User identity and business state |
| Cancellation propagation and owned cleanup | Interactive approvals and escalation |
| Payload, context, event and time bounds | Which capabilities a user may request |
| Static policy evaluation | Credential provisioning and rotation |
| Native outcome and accounting | Durable conversation or job storage |
| Optional JSONL and OTel instrumentation | Trace retention and access controls |

The workspace establishes a contained working directory and filesystem root for Pablo’s built-in tools. It is not an operating-system sandbox. A permitted shell command runs with the account’s ordinary permissions and may access resources outside that workspace unless the host isolates the process.

## One task lifecycle

An admitted task follows one root lifecycle:

1. The host validates configuration, provider capabilities, limits, policy and private credential references.
2. Pablo emits `run.started` and opens a streamed model operation.
3. The provider yields text or a tool call. Pablo validates tool identity and arguments before dispatch.
4. The tool result returns to the same model conversation. The loop continues sequentially.
5. Optional output validation checks the final answer. A configured repair gets at most one additional attempt.
6. Pablo settles accounting, emits `run.finished`, closes child/tool resources and returns one typed outcome.

Model calls can fall back across an ordered route. Local children and remote A2A tasks remain attributed to the root, with their own identity and bounded handoff. Native events preserve source order across the supervised tree.

## Outcomes, not exceptions alone

Normal task results use one of six states:

- `completed` contains final output, a stop reason and reported usage.
- `cancelled` means the local cancellation path completed its owned cleanup.
- `timed_out` means the root deadline won.
- `policy_denied` identifies a built-in or configured deciding rule.
- `limit_exceeded` identifies the exhausted input, output, context, event, call, tool or accounting limit.
- `failed` carries a closed failure code and delivery certainty.

Setup errors happen before admission and use separate CLI/ACP error framing. Once a task is admitted, its terminal event is the source of truth.

## Interfaces

### CLI and TUI

`pablo run` executes one task and streams human-readable output. `--json` returns one machine envelope. `pablo tui` presents the same event stream in an interactive terminal. Each submission has fresh model context.

### ACP

`pablo acp --stdio` lets an editor or another process create sessions, send a prompt, consume typed updates and cancel work. The process can remain warm across independent sessions.

### Rust embedding

`pablo-core` accepts a typed `RunSpec`, `Provider`, `EventSink` and cancellation token. It does not install global telemetry, discover credentials or create an ambient tool catalog.

## Design posture

Pablo favors explicit configuration and closed failures. Redirects and automatic invocation replays are disabled in provider and remote-tool transports. Credentials stay outside model-visible configuration. Unknown usage remains unknown instead of being inferred as zero. Cross-compilation may prepare a binary, but native execution is required before a release target is accepted.

Continue with [Getting started](./getting-started.md), or jump to [embedding](./embedding.md) and [ACP integration](./acp.md).
