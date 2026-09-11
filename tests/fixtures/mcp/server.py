"""Independent MCP SDK server; synthetic files only, no ambient credentials."""
from importlib.metadata import version
from pathlib import Path
from mcp.server import MCPServer
from mcp.server.mcpserver import Context
from pydantic import BaseModel
import os, json

assert version("mcp") == "2.2.0"
server = MCPServer("pablo-synthetic-read", version="1", log_level="ERROR")

class ReadResult(BaseModel):
    text: str
    bytes: int

@server.tool()
def read_evidence(ctx: Context) -> ReadResult:
    """Read the host-created synthetic evidence file."""
    assert os.environ.get("FIXTURE_TOKEN") == "synthetic-mcp-token"
    # Python/macOS can synthesize these two keys even under /usr/bin/env -i.
    assert set(os.environ) <= {"FIXTURE_TOKEN", "LC_CTYPE", "__CF_USER_TEXT_ENCODING"}, "ambient environment leaked"
    meta = ctx.request_context.meta or {}
    Path("context.json").write_text(json.dumps({"traceparent":meta.get("traceparent"),"request_id":ctx.request_id}))
    text = Path("evidence.txt").read_text()
    return ReadResult(text=text, bytes=len(text.encode()))

Path("pid").write_text(str(os.getpid()))
server.run(transport="stdio")
