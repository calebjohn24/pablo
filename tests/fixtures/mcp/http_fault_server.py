"""Independent stdlib HTTP fault peer; all requests/values are synthetic."""
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
from pathlib import Path
import json, sys, time

mode = sys.argv[1]
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def record(self, value):
        assert self.headers.get("x-fixture-token") == "synthetic-http-token"
        with Path("requests.jsonl").open("a") as log:
            log.write(json.dumps({"method": self.command, "path": self.path, **value}) + "\n")
    def reply(self, status, body=b"", content="application/json", extra=None):
        self.send_response(status)
        self.send_header("Content-Type", content)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (extra or {}).items(): self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)
    def do_DELETE(self):
        self.record({})
        assert self.headers.get("mcp-session-id") == "synthetic-session"
        self.reply(405 if mode == "delete_405" else 204)
    def do_POST(self):
        value = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.record(value)
        method = value["method"]
        if mode == "redirect":
            return self.reply(307, extra={"Location": "/stolen"})
        if method.startswith("notifications/"):
            return self.reply(202)
        identifier = value["id"]
        extra = {}
        if method == "initialize":
            result = {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}, "serverInfo": {"name": "fault", "version": "1"}}
            extra["mcp-session-id"] = "s" * 257 if mode == "session_bound" else "synthetic-session"
        elif method == "tools/list":
            assert self.headers.get("mcp-protocol-version") == "2025-11-25"
            assert self.headers.get("mcp-session-id") == "synthetic-session"
            result = {"tools": [{"name": "read", "inputSchema": {"type": "object", "additionalProperties": False}}]}
        else:
            assert method == "tools/call"
            if mode == "session_expired": return self.reply(404)
            if mode == "call_hang":
                time.sleep(4)
                return
            if mode == "disconnect":
                return self.reply(200, b"id: pending\ndata:\nretry: 1\n\n", "text/event-stream")
            if mode == "frame_bound":
                return self.reply(200, b"data: " + b"x" * (2 * 1024 * 1024 + 1), "text/event-stream")
            if mode == "progress_bound":
                progress = b'data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"p","progress":1}}\n\n'
                return self.reply(200, progress * 66, "text/event-stream")
            if mode == "wrong_id": identifier = "unrelated"
            if mode == "session_changed": extra["mcp-session-id"] = "changed"
            result = {"content": [{"type": "text", "text": "synthetic"}]}
        self.reply(200, json.dumps({"jsonrpc": "2.0", "id": identifier, "result": result}).encode(), extra=extra)

http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
Path("endpoint").write_text(f"http://127.0.0.1:{http.server_port}/mcp")
http.serve_forever()
