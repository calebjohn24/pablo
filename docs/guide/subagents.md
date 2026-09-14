---
title: Subagents
description: Delegate bounded work to supervised local children or configured remote A2A peers without widening application-owned authority.
---

# Subagents

Pablo can split a root task across supervised subagents. Each subagent is an independently identified child run with its own task, streamed events, cancellation state and typed outcome. The root application remains the owner of the workspace, credentials, policy and final product state.

Pablo calls local subagents **supervised children** in configuration and runtime records. Remote subagents use the same supervisor boundary while their work is carried by [A2A](./extensibility.md#remote-a2a-tasks).

## When to use a subagent

Subagents work well when parts of a task can run independently or need different context and capabilities:

| Work shape | Example |
| --- | --- |
| Parallel investigation | Ask one child to inspect customer records while another checks policy documents. |
| Specialized review | Give a reviewer child one Skill and a read-only MCP server. |
| Bounded fan-in | Run two children concurrently, validate their structured results, then pass selected results to a final child. |
| Remote delegation | Send a narrowly described task to a host-configured A2A peer and retain its Artifact as an untrusted remote result. |

Keep a task in the root when it is short, sequential or depends on one continuously changing context. Every child adds another model loop and consumes the root task's shared capacity.

## Enable local subagents

Subagents are disabled unless a deployment enables them:

```toml
[options.children]
enabled = true
```

Run that deployment through the CLI or ACP as usual:

```sh
pablo acp --stdio --config deployment.toml
```

The root model then receives one built-in `subagent` tool. Your application does not need to create another scheduler or protocol connection. It observes the complete tree through Pablo's normal ACP updates, native events and OpenTelemetry spans.

## Child lifecycle

The `subagent` tool exposes five operations:

| Action | Behavior |
| --- | --- |
| `spawn` | Admit a temporary local child and immediately return its handle. |
| `spawn_remote` | Admit a task for one configured A2A peer and return its handle. |
| `inspect` | Read a bounded snapshot of one owned child's state and terminal result, if settled. |
| `wait` | Wait for any or all selected children without cancelling them on timeout. |
| `stop` | Cancel one child and await its owned cleanup without stopping its siblings or parent. |

A local spawn request can narrow its input, model route, tools, Skills, MCP servers and ceilings:

```json
{
  "action": "spawn",
  "request": {
    "input": "Read the workspace evidence and return the three strongest findings.",
    "capabilities": {
      "tools": ["fs.read", "fs.list", "fs.search"],
      "skills": ["team/evidence-review"],
      "mcp_servers": ["records"],
      "model_route": ["primary"]
    },
    "ceilings": {
      "max_model_calls": 3,
      "max_tool_calls": 8,
      "max_duration_ms": 120000
    },
    "output_schema": {
      "type": "object",
      "properties": {
        "findings": {
          "type": "array",
          "items": { "type": "string" },
          "maxItems": 3
        }
      },
      "required": ["findings"],
      "additionalProperties": false
    }
  }
}
```

Capability names must already exist in the root deployment and policy. A child request can remove authority or lower a ceiling; it cannot add a tool, credential, route, path or limit that the root did not have.

Spawn returns before the child finishes. Use `wait` to join independent work:

```json
{
  "action": "wait",
  "agent_ids": [
    "2a85e7a4-38c2-4f73-8f8e-1d53c8f80d77",
    "804af113-c549-4853-b25b-87c499845e07"
  ],
  "mode": "all",
  "timeout_ms": 120000
}
```

Treat the identifiers above as placeholders; Pablo assigns every root and child identity.

## Shared authority and budgets

Local children inherit the root's trusted instruction prefix and receive only their selected task overlay and attachments. They do not receive the parent's raw transcript. They share:

- the application-owned workspace;
- root deadlines and model/tool/token/cost ceilings;
- aggregate context, output, event and trace capacity;
- process and MCP-session capacity; and
- immutable host and root policy ceilings.

The current release admits at most two active children and sixteen total children per root. Additional admitted work queues FIFO under the original root deadline. Children are depth one and cannot spawn grandchildren. Completion, failure or cancellation of one child does not rewrite a sibling's result.

The shared workspace is not a transaction or operating-system sandbox. Pablo serializes supported filesystem mutations across the tree and preserves revision checks, but shell commands and external processes can still race. Put the root process in your own container or sandbox when tasks must be isolated.

## Validated handoffs

Pablo retains child results outside the root model's ordinary message history. A later child receives a prior result only through an explicit handoff containing the source agent ID, immutable result ID and one of these kinds:

- `inline` passes a schema-valid structured result.
- `artifact` passes a workspace-relative path plus SHA-256 revision after Pablo rechecks read authority, file type and current bytes.

Failed, cancelled, oversized or schema-invalid results cannot become successful handoffs. Raw child transcripts never become trusted instructions.

This lets an application build a bounded fan-out/fan-in flow: spawn two focused readers, wait for both, select their validated results and admit one final synthesis child.

## Remote subagents through A2A

Configure every remote name, Agent Card URL and RPC endpoint before a task starts:

```toml
[options.a2a.remotes.reviewer]
card_url = "https://agent.example.com/.well-known/agent-card.json"
endpoint = "https://agent.example.com/rpc"
```

`spawn_remote` can select only a configured remote. Agent Cards, progress updates and Artifacts are untrusted remote data. Remote usage is reported metadata rather than locally enforceable accounting, and cancellation confirms only the local request lifecycle; it cannot prove that a remote service rolled back work.

Use [MCP, Skills and agents](./extensibility.md) for complete MCP, Skill and A2A configuration details.

## What your application should observe

For every tree, retain:

- root, parent and child agent IDs;
- each child state and typed terminal outcome;
- the model and tool accounting attributed to each agent and aggregated at the root;
- validated handoff source/result IDs;
- native `root_seq` ordering alongside each agent's own event sequence; and
- cancellation and cleanup completion before accepting the root terminal result.

The root result is final only after Pablo joins active and queued children, owned processes and MCP sessions. Through ACP, continue reading updates until the terminal Pablo task object arrives. Direct Rust hosts should use the child contracts and root ledger from the same pinned `pablo-core` revision as the runtime.

## Current release limits

- Temporary depth-one children only.
- Two concurrently active children per root.
- No named persistent children or post-root background work.
- No automatic retry, graph scheduler or transcript copying.
- Local children use typed in-memory ACP handlers; configured external ACP child processes are outside this release slice.
- Pablo acts as an A2A client for remote delegation and does not expose an A2A server.

These constraints keep delegation observable, cancellable and bounded inside the same lifecycle as an ordinary single-agent run.
