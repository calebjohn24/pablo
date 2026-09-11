# Independent A2A reference

`lock.json` pins A2A protocol v1.0.0 at commit
`173695755607e884aa9acf8ce4feed90e32727a1` and the independent official Python SDK
1.0.2 at `eb37091fcd6411b3b01481ea3fd3e001c6fb55c0`. It records the normative
Markdown/proto hashes and published SDK wheel hash. `requirements.txt` freezes the
isolated fixture's resolved package versions; no runtime Python dependency is added.

Install with the existing Python 3.12 interpreter into the ignored
`.pablo/a2a-fixture-venv`, then use `uv pip install --python
.pablo/a2a-fixture-venv/bin/python -r tests/fixtures/a2a/requirements.txt`.
`generate_vectors.py` regenerates `card.json` and `wire.json` using SDK protobuf JSON.
These examples intentionally use v1.0 camelCase fields and protobuf enum names.

`server.py` serves the unmodified SDK's Agent Card routes on a literal loopback
socket. The wrapper records only method/path/version/auth-presence in its temporary
working directory. It has no task executor. The explicit Rust acceptance test
proves two versioned unauthenticated GETs through configured admission, distinct local
proxy IDs, stable remote identity binding and root-ledger kind checks with no task
submission, then joins the peer.
The fixture requires no live provider credentials and must never load root `.env`.

Run:

```
cargo test --locked -p pablo-core --test a2a_cards -- --include-ignored
```

The R01 fixtures prove admission and wire conformance. C3.27 additionally exercises
the configured supervisor through CLI/ACP; see the project evidence for each gate.

`wire_server.py` runs the official SDK JSONRPC/SSE dispatcher with deterministic
synthetic handlers. The explicit wire test sends encoded requests, decodes SDK
Message/Task/status/artifact/cancel replies and verifies unsupported-version
rejection before a handler call. It checks selected synthetic text/data/file/URL input, explicit output modes,
historyLength zero, negotiated trace headers/metadata and generic fallback. The
fixture never follows a file URL or reads a local filename. It is not the production
supervised task transport.

```
cargo test --locked -p pablo-core --test a2a_wire -- --include-ignored
```


`boundary_server.py` drives one selected adverse SDK lifecycle per process. The
CLI/ACP matrix exercises model stop and actual root SIGINT/session cancellation,
assignment boundaries, cancel rejection/completion races, stream loss, deadline,
oversized artifacts, input-required states and nonresponsive cancellation. Call
receipts prove a single submission and at most one cancellation, not rollback.

`telemetry_server.py` creates an actual task span using Python OpenTelemetry SDK
1.44.0 and its pinned OTLP/HTTP exporter. It accepts the explicit optional W3C
profile only when advertised, verifies matching headers/metadata, and exports
through the real pinned Collector. Generic peers start an independent trace.
`node scripts/smoke-a2a-collector.ts` checks remote parenting, native identity and
exact timestamps, content redaction and exporter-outage outcome/accounting parity.
All fixture dependencies remain isolated from the Rust runtime.
