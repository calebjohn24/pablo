# Independent M02 fixture

`server.py` uses the official Python SDK `mcp==2.2.0`, independently of Pablo's
Rust SDK. PyPI wheel SHA-256:
`bde982589473a060ae145e3406e9a5333fe538c97229ba841f5a7f92be004f81`.
Source/distribution metadata: <https://pypi.org/project/mcp/2.2.0/>.
`requirements.txt` pins the resolved fixture dependencies. Install into the ignored
`.pablo/mcp-fixture-venv` with the existing Python installation. No live provider
or credential file is used. The caller creates a fresh synthetic working directory
and supplies only `FIXTURE_TOKEN=synthetic-mcp-token`.
