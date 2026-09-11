"""Pinned SDK JSONRPC/SSE dispatcher with deterministic synthetic handlers."""
from importlib.metadata import version
from pathlib import Path
import json, socket, os, asyncio
from google.protobuf.json_format import ParseDict, MessageToDict
from a2a.types import a2a_pb2 as p
from a2a.server.routes import create_jsonrpc_routes
from a2a.utils.errors import TaskNotFoundError
from starlette.applications import Starlette
from starlette.routing import Route
from starlette.responses import JSONResponse
import uvicorn
assert version("a2a-sdk") == "1.0.2"
vectors=json.loads(Path(__file__).with_name("wire.json").read_text())
assembly=json.loads(Path(__file__).with_name("assembly.json").read_text())
def value(name,field,kind): return ParseDict(vectors[name][field],kind())
def record(method):
    with Path("calls.jsonl").open("a") as out: out.write(json.dumps({"method":method})+"\n")
TRACE="urn:pablo:a2a:tracecontext:v1"
PARENT="00-"+"1"*32+"-"+"2"*16+"-01"
def selected(params):
    assert params.configuration.HasField("history_length") and params.configuration.history_length == 0
    assert not params.message.metadata
    if params.metadata:
        assert MessageToDict(params.metadata)=={TRACE:{"traceparent":PARENT,"tracestate":"vendor=value"}}
        assert list(params.message.extensions)==[TRACE]
    else: assert not params.message.extensions
    assert params.message.role == p.ROLE_USER and len(params.message.parts)==1
    assert params.message.parts[0].WhichOneof("content") in ("text","data","raw","url")
    return params.message.parts[0].text if params.message.parts[0].WhichOneof("content")=="text" else None
class Handler:
    async def on_message_send(self,params,context):
        text=selected(params);record("SendMessage")
        if text=="task": return value("task_result","task",p.Task)
        reply=value("message_result","message",p.Message)
        reply.parts[0].CopyFrom(params.message.parts[0]);reply.metadata.CopyFrom(params.metadata);reply.extensions.extend(params.message.extensions)
        return reply
    async def on_message_send_stream(self,params,context):
        text=selected(params);record("SendStreamingMessage")
        if text=="hold":
            yield value("stream_status","statusUpdate",p.TaskStatusUpdateEvent)
            await asyncio.sleep(20)
            return
        if text=="assembly":
            yield value("stream_status","statusUpdate",p.TaskStatusUpdateEvent)
            for event in assembly["events"]: yield ParseDict(event,p.TaskArtifactUpdateEvent())
            yield p.TaskStatusUpdateEvent(task_id="remote-task",context_id="remote-context",status=p.TaskStatus(state=p.TASK_STATE_COMPLETED),metadata={"urn:pablo:a2a:reported-usage:v1":{"inputTokens":"13","outputTokens":"7","totalTokens":"20","costMicrousd":"42"}})
            return
        yield value("stream_status","statusUpdate",p.TaskStatusUpdateEvent)
        yield value("stream_artifact","artifactUpdate",p.TaskArtifactUpdateEvent)
        yield value("stream_task","task",p.Task)
    async def on_cancel_task(self,params,context):
        record("CancelTask")
        if params.id != "remote-task": raise TaskNotFoundError()
        task=ParseDict(vectors["cancel_result"],p.Task());task.status.state=p.TASK_STATE_CANCELED
        return task
async def card(request):
    record("GetAgentCard")
    assert "authorization" not in request.headers
    return JSONResponse(json.loads(Path(__file__).with_name("card.json").read_text()))
sdk_app=Starlette(routes=[Route("/.well-known/agent-card.json",card),*create_jsonrpc_routes(Handler(),"/rpc",enable_v0_3_compat=False)])
async def app(scope,receive,send):
    if scope["type"]=="http":
        headers=dict(scope["headers"])
        if b"a2a-extensions" in headers:
            assert headers[b"a2a-extensions"].decode()==TRACE
            assert headers[b"traceparent"].decode()==PARENT and headers[b"tracestate"]==b"vendor=value"
        else: assert b"traceparent" not in headers and b"tracestate" not in headers
        assert b"baggage" not in headers and b"authorization" not in headers
    await sdk_app(scope,receive,send)
def serve(application):
    sock=socket.socket();sock.bind(("127.0.0.1",0));sock.listen(128)
    print(json.dumps({"url":f"http://127.0.0.1:{sock.getsockname()[1]}/rpc","pid":os.getpid()}),flush=True)
    uvicorn.Server(uvicorn.Config(application,log_level="error",lifespan="off")).run(sockets=[sock])
if __name__ == "__main__": serve(app)
