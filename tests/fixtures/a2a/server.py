"""Independent pinned SDK Agent Card routes; this R01 peer submits no tasks."""
from importlib.metadata import version
from pathlib import Path
import json, socket, os
from google.protobuf.json_format import Parse
from a2a.types.a2a_pb2 import AgentCard
from a2a.server.routes import create_agent_card_routes
from starlette.applications import Starlette
import uvicorn

assert version("a2a-sdk") == "1.0.2"
card = Parse(Path(__file__).with_name("card.json").read_text(), AgentCard())
sdk_app = Starlette(routes=create_agent_card_routes(card))

async def app(scope, receive, send):
    if scope["type"] == "http":
        headers = dict(scope["headers"])
        with Path("requests.jsonl").open("a") as out:
            out.write(json.dumps({"method": scope["method"], "path": scope["path"],
                                  "version": headers.get(b"a2a-version", b"").decode(),
                                  "authenticated": b"authorization" in headers}) + "\n")
    await sdk_app(scope, receive, send)

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
sock.listen(128)
print(json.dumps({"url": f"http://127.0.0.1:{sock.getsockname()[1]}/.well-known/agent-card.json", "pid": os.getpid()}), flush=True)
uvicorn.Server(uvicorn.Config(app, log_level="error", lifespan="off")).run(sockets=[sock])
