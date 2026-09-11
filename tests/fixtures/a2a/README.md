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

These fixtures prove R01 admission and wire conformance, not implemented supervised
remote task execution. Check project state/evidence for the acceptance status.

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
