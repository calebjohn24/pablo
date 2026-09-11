"""Independent SDK adverse lifecycle fixture; one explicit scenario per process."""
import asyncio, sys
sys.dont_write_bytecode=True
from wire_server import Handler, selected, record, value, p, card, serve
from a2a.server.routes import create_jsonrpc_routes
from a2a.utils.errors import TaskNotCancelableError
from starlette.applications import Starlette
from starlette.routing import Route
SCENARIO=sys.argv[1]
assert SCENARIO in {"before_assignment","accepted","rejected","late_completion","stream_loss","timeout","oversized","input_required","cancel_hang"}
class Boundary(Handler):
    async def on_message_send_stream(self,params,context):
        assert selected(params)=="PRIVATE_EXPLICIT_PART"
        record("SendStreamingMessage")
        if SCENARIO=="before_assignment": await asyncio.sleep(20)
        yield value("stream_status","statusUpdate",p.TaskStatusUpdateEvent)
        if SCENARIO=="stream_loss": return
        if SCENARIO=="input_required":
            yield p.TaskStatusUpdateEvent(task_id="remote-task",context_id="remote-context",status=p.TaskStatus(state=p.TASK_STATE_INPUT_REQUIRED))
            return
        if SCENARIO=="oversized":
            yield p.TaskArtifactUpdateEvent(task_id="remote-task",context_id="remote-context",artifact=p.Artifact(artifact_id="large",parts=[p.Part(text="x"*70000)]),last_chunk=True)
            return
        await asyncio.sleep(20)
    async def on_cancel_task(self,params,context):
        assert params.id=="remote-task"
        record("CancelTask")
        if SCENARIO=="rejected":raise TaskNotCancelableError()
        if SCENARIO=="cancel_hang":await asyncio.sleep(20)
        task=value("stream_task","task",p.Task)
        task.status.state=p.TASK_STATE_COMPLETED if SCENARIO=="late_completion" else p.TASK_STATE_CANCELED
        return task
sdk=Starlette(routes=[Route("/.well-known/agent-card.json",card),*create_jsonrpc_routes(Boundary(),"/rpc",enable_v0_3_compat=False)])
async def app(scope,receive,send):
    if scope["type"]=="http":
        headers=dict(scope["headers"])
        assert not any(key in headers for key in [b"authorization",b"baggage",b"traceparent",b"tracestate",b"a2a-extensions"])
    await sdk(scope,receive,send)
serve(app)
