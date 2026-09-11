# Independent M02 fixture

`server.py` uses the official Python SDK `mcp==2.2.0`, independently of Pablo's
Rust SDK. PyPI wheel SHA-256:
`bde982589473a060ae145e3406e9a5333fe538c97229ba841f5a7f92be004f81`.
Source/distribution metadata: <https://pypi.org/project/mcp/2.2.0/>.
`requirements.txt` pins the resolved fixture dependencies. Install into the ignored
`.pablo/mcp-fixture-venv` with the existing Python installation. No live provider
or credential file is used. The caller creates a fresh synthetic working directory
and supplies only `FIXTURE_TOKEN=synthetic-mcp-token`.

M03's separate `http_server.py` exposes stateful JSON and SSE responses through
the same independent pinned SDK, using a literal loopback socket and synthetic
`x-fixture-token` header. It records only synthetic request IDs/session headers in
the test's temporary directory. `http_fault_server.py` is an independent stdlib
fault peer for disconnects, redirects, framing/progress bounds and cancellation.
Run acceptance tests with `cargo test --locked -p pablo-core --lib mcp:: -- --ignored --skip measure_http`.
The separately ignored `mcp::http_tests::measure_http` test is a release measurement;
run it only after other tests/builds finish and retain its optimized-build flag.
