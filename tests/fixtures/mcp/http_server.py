"""Independent stateful JSON/SSE MCP HTTP fixture; synthetic credentials only."""
from importlib.metadata import version
from pathlib import Path
import json, socket, sys
import uvicorn
from mcp.server import MCPServer
from mcp.server.mcpserver import Context
from pydantic import BaseModel

assert version("mcp") == "2.2.0"
server = MCPServer("pablo-http-read", version="1", log_level="ERROR")

class ReadResult(BaseModel):
    text: str

@server.tool()
def read_evidence(ctx: Context) -> ReadResult:
    """Read a fresh synthetic file through the independent HTTP server."""
    meta = ctx.request_context.meta or {}
    Path("context.json").write_text(json.dumps({"traceparent":meta.get("traceparent"),"request_id":ctx.request_id}))
    return ReadResult(text=Path("evidence.txt").read_text())

app = server.streamable_http_app(json_response=sys.argv[1] == "json")

class Recorded:
    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            return await app(scope, receive, send)
        headers = dict(scope["headers"])
        expected = Path("expected-token").read_bytes() if Path("expected-token").exists() else b"synthetic-http-token"
        token_header = sys.argv[2].encode() if len(sys.argv) > 2 else b"x-fixture-token"
        assert headers.get(token_header) == expected
        assert b"authorization" not in headers, "ambient provider authentication leaked"
        body = b""
        while True:
            message = await receive()
            assert message["type"] == "http.request"
            body += message.get("body", b"")
            if not message.get("more_body", False):
                break
        value = json.loads(body) if body else {}
        record = {"method": scope["method"], "rpc": value.get("method"), "id": value.get("id"),
                  "protocol": headers.get(b"mcp-protocol-version", b"").decode(),
                  "session": headers.get(b"mcp-session-id", b"").decode()}
        if value.get("method") != "initialize":
            assert record["protocol"] == "2025-11-25"
            assert record["session"]
        with Path("requests.jsonl").open("a") as log:
            log.write(json.dumps(record) + "\n")
        delivered = False
        async def replay():
            nonlocal delivered
            if not delivered:
                delivered = True
                return {"type": "http.request", "body": body}
            return await receive()
        await app(scope, replay, send)

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
sock.listen(128)
Path("endpoint").write_text(f"http://127.0.0.1:{sock.getsockname()[1]}/mcp")
uvicorn.Server(uvicorn.Config(Recorded(), log_level="error", access_log=False)).run(sockets=[sock])
