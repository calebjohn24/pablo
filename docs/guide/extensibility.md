---
title: MCP, Skills and agents
description: Add configured MCP tools, portable local Skills, supervised children and remote A2A workers.
---

# MCP, Skills and agents

Pablo extends a task through explicit host configuration. Tool descriptions and model instructions can request work, but they cannot add authority, credentials or destinations.

## MCP tools

Deployments can attach Model Context Protocol servers over stdio or Streamable HTTP. Each task receives a fresh MCP catalog and sessions.

### Stdio server

```toml
[credentials.records]
consumer = "mcp.environment"
sources = [{ kind = "environment", name = "RECORDS_TOKEN" }]

[options.mcp.servers.records]
transport = "stdio"
command = "/absolute/path/to/records-server"
args = ["--stdio"]
cwd = { base = "binding", name = "mcp", path = "." }
env = { RECORDS_TOKEN = "records" }
required = true
startup_timeout_ms = 30000
operation_timeout_ms = 60000
```

Stdio uses an absolute launcher, cleared environment and exact credential references. It does not search `PATH` or invoke a shell. Pablo owns the server process group and joins it on cancellation or close.

### Streamable HTTP server

```toml
[credentials.evidence]
consumer = "mcp.headers"
sources = [{ kind = "environment", name = "EVIDENCE_TOKEN" }]

[options.mcp.servers.evidence]
transport = "http"
url = "https://mcp.example.com/mcp"
headers = { x-evidence-token = "evidence" }
required = true
startup_timeout_ms = 30000
operation_timeout_ms = 60000
```

Production endpoints require HTTPS. Requests do not follow redirects or automatically replay tool invocations. Disconnect/cancellation closes local work while preserving remote completion uncertainty.

### MCP policy

Tools receive qualified identities: `mcp/{server}/{tool}`.

```toml
[[options.mcp.policies]]
[options.mcp.policies.servers]
default = "deny"
allow = [{ id = "support.evidence-server", value = "evidence" }]

[options.mcp.policies.tools]
default = "deny"
allow = [{ id = "support.read-evidence", value = "mcp/evidence/read_evidence" }]
```

Policy checks configured servers, tools and stdio launchers. An allow rule cannot create a server that the host did not configure. Tool input and structured output validate against the server’s declared schemas within Pablo’s supported bounded JSON Schema subset.

MCP results support ordered text blocks and optional structured JSON. Unsupported image/audio/resource content fails explicitly rather than being silently flattened.

## Portable Agent Skills

A Skill is a directory containing `SKILL.md` with portable YAML frontmatter and Markdown instructions:

```md
---
name: evidence-review
description: Review cited evidence and retain exact source references.
---

Use only the evidence provided by the task and configured tools.
```

Configure explicit roots and activations:

```toml
[options.skills.roots]
team = { base = "binding", name = "skills", path = "." }

[options.skills]
activate = ["team/evidence-review"]
```

```sh
pablo skills list --config deployment.toml \
  --bind workspace=./work \
  --bind skills=./skills
pablo skills show team/evidence-review --config deployment.toml \
  --bind workspace=./work \
  --bind skills=./skills
```

Discovery scans only configured roots and reads frontmatter metadata. Activation reads complete instructions for the selected Skills and adds them to bounded task context. Skill instructions and `allowed-tools` metadata do not grant capabilities. Resource access still requires configured read policy.

Root/package/SKILL.md symlinks reject. Pablo does not automatically scan a home directory, install dependencies or execute Skill scripts.

## Supervised local children

Configured child tools can dispatch bounded sub-tasks through the same typed ACP handlers used by the process protocol. Root policy narrows every child’s model route, tools, Skills, MCP catalog, workspace and limits.

Children are depth one in the current release line. A root can run two admitted children concurrently when configured capacity allows; excess work queues FIFO under the root deadline. Model/tool/token/cost/event/trace capacity is shared with the root rather than reset per child.

The supervisor retains child results outside ordinary model history. A model promotes one validated result through a typed handoff. Structured output and accounting receipts enter root context; raw child transcripts and authority do not.

For file artifacts, promotion rechecks the path and revision under narrowed read authority immediately before downstream use. A child cannot transfer broader filesystem rights by naming a path.

## Remote A2A tasks

Remote workers use the A2A 1.0 JSON-RPC/SSE subset. Configure an exact Agent Card URL and RPC endpoint:

```toml
[options.a2a.remotes.reviewer]
card_url = "https://agent.example.com/.well-known/agent-card.json"
endpoint = "https://agent.example.com/rpc"
```

Agent Cards are untrusted capability metadata. Fetching a card sends no bearer credential and never follows redirects. A separate `a2a.bearer` credential can be scoped to the exact RPC endpoint.

Pablo maps remote task updates and one Artifact into the local supervised lifecycle. Remote usage remains reported metadata separate from locally enforceable accounting. Cancellation sends a bounded remote request and closes local ownership, but cannot claim the remote service rolled back work.

## Authority model

Across MCP, Skills and children:

- Configuration defines possible capabilities.
- Immutable authority layers set ceilings.
- Ordinary policy narrows the catalog.
- A model or Skill can choose among admitted capabilities.
- Tool output, Agent Cards and child handoffs never grant new authority.
- Credentials resolve only for their named consumer and exact destination.

Use the locked [production preset](https://github.com/calebjohn24/pablo/blob/main/presets/v1/production.toml) as a concrete reference.
