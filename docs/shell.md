# C1.2 shell execution

The Rust core supports a sequential model/tool loop with one built-in capability, `shell.run`. An empty `ToolRegistry` grants no tools. Hosts enable shell explicitly with `ToolRegistry::with_shell()` and pass it to `Runtime::run_with_tools` with a `CancellationToken`. The existing `Runtime::run` convenience method continues to grant no tools.

The offline integration fixture exercises real command execution and verifies its output on the second model request:

```sh
cargo test --locked -p pablo-core --test tool_loop actual_shell_evidence_round_trip
```

## Request and policy

The catalog carries a built-in Draft 2020-12 schema compiled once per registry. `jsonschema` 0.53.0 is pinned with default features disabled; a rejecting retriever also prevents HTTP and file reference resolution. These options follow the [validator's configuration API](https://docs.rs/jsonschema/0.53.0/jsonschema/struct.ValidationOptions.html).

Example arguments:

```json
{
  "command": "cat evidence.txt",
  "cwd": ".",
  "timeout_ms": 5000,
  "max_output_bytes": 8192,
  "env": {"PABLO_TASK_LABEL": "fixture"}
}
```

`command` and `cwd` are required nonempty strings. `env`, `timeout_ms`, and `max_output_bytes` are optional. Unknown fields, invalid types, zero requested limits, and NUL characters are rejected. Command, cwd, environment name/value, property-count, and total argument-byte bounds apply before spawning.

The shell is noninteractive `/bin/sh -c` with closed stdin. It receives a fresh environment containing only `PATH=/usr/bin:/bin`, `LANG=C`, and explicit additions whose names begin with `PABLO_TASK_`. It does not inherit provider/exporter credentials, `HOME`, shell startup variables, or tracing context. C1.2 has no environment override that can replace PATH or inject a startup file. Environment values are supplied by the caller/model; the runtime does not load `.env`.

Relative cwd values resolve beneath the canonical workspace; absolute cwd values must also resolve inside it. Nonexistent directories, parents outside the workspace, and escaping symlinks produce a typed policy denial. Cwd validation is a static policy check. Commands can access resources available to the host process; hosts provide filesystem, credential, network, and process containment for untrusted work. Commands that deliberately leave their process group require host containment.

Supported execution platforms are macOS and Linux. Other platforms explicitly deny shell execution. This checkpoint was verified on macOS; the cross-platform acceptance gate remains in C1.6.

## Results and bounds

Tools run in request order, one at a time per run. The next model request contains the prior assistant message and each actual tool result, preserving call IDs and the unchanged instruction/tool prefix. The internal qualified name remains `shell.run`; a future HTTP adapter must map it to a provider-valid function name.

Model and tool call counts default to unlimited, with optional explicit caps. Other defaults are one hour per run, 15 minutes per tool, 1 MiB for tool arguments and 8 MiB for the complete serialized tool result. CLI and ACP hosts can set `--timeout` and `--tool-timeout` in seconds, each from 1 to 86400. Requested tool limits can narrow the host limits. A separate 32 MiB request-context bound covers serialized instructions, messages, and tool definitions. No tool runs without model-call budget left to consume its result.

`ToolResult` carries a typed status, optional policy rule, and optional `ShellResult`. The shell result separates stdout/stderr, exit code/signal, truncation flags, and flags identifying lossy replacement of non-UTF-8 bytes. Output retention is bounded across both streams. JSON escaping or replacement characters can expand retained output; the complete serialized result is bounded again, with additional truncation made explicit. At least 1,024 bytes of host tool-result capacity are required to retain metadata.

A nonzero exit is returned to the next model call as an ordinary tool result with its exit status. Output-capacity exhaustion stops the command and run with a typed limit outcome. Invalid arguments, policy denial, timeouts, execution errors, and cleanup failure stop the run explicitly. There is no automatic retry or argument repair.

Native events report tool start, process start, and tool finish. Stdout/stderr are delivered in the final tool result in this checkpoint; there is no interactive process handle or live byte-stream API. The existing model text stream remains incremental.

## Cancellation and cleanup

Cancel the supplied token and continue awaiting the run. Provider opening/streaming futures are dropped on cancellation. The runtime awaits the executing tool's cleanup instead of dropping its future to enforce the deadline.

Each shell leader starts a new process group using [Tokio's process-group option](https://docs.rs/tokio/1.53.1/tokio/process/struct.Command.html#method.process_group). Exit observation uses [`waitid` with `NOWAIT`](https://docs.rs/rustix/1.1.4/rustix/process/fn.waitid.html), preserving the leader's PID until group signaling is finished. C1.2 uses immediate SIGKILL for owned group cleanup, including normal shell exit, so background children do not outlive a completed tool call.

Cleanup then reaps the leader, drains the pipes within the output bound, and probes until the group disappears. Only signal-zero probes occur after reaping; no real signal is sent to a potentially recycled group ID. macOS can report EPERM when a group contains only exited members. That result is treated provisionally: cleanup must still observe the group disappear, so a live inaccessible member cannot be reported as cleaned up.

Cleanup has a two-second allowance beyond the execution deadline. Failure to reap, drain, or observe disappearance returns `ToolCleanup` failure, which takes precedence over a cancellation claim. Descendant zombie reaping depends on the host's process reaper. The runtime does not install a global subreaper or take ownership of unrelated children.

Dropping a run future invokes process-group and `kill_on_drop` fallbacks, but cannot await cleanup or deliver a terminal event. Hosts should request cancellation and await settlement. Similarly, synchronous event sinks must return promptly; their blocking I/O cannot be interrupted by an async deadline. ACP disconnect and queue behavior are C1.3 work.

## Telemetry

Run, model, and tool spans originate from the same lifecycle. Tool spans are children of the run span and share exact timestamps and IDs with native tool events. Shell cleanup finishes before the tool-finished event and span end. Intentional cancellation leaves OTel status unset; a nonzero shell exit marks its tool span as an error while allowing the model to handle the result.

Native capture defaults off: arguments are null, stdout/stderr are null with byte counts, and operational metadata remains. Explicit native capture can include commands, cwd, environment additions, and results. OTel never receives this content. The schema and internal telemetry mapping are versioned `c1.2`; the pinned upstream GenAI conventions remain unchanged. See [the runtime mapping](runtime.md#otel-mapping).
