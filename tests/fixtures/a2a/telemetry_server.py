"""Real peer SDK span and OTLP export from negotiated W3C context; no local policy."""
import sys,json,contextvars
sys.dont_write_bytecode=True
from pathlib import Path
from importlib.metadata import version
from wire_server import record,value,p,serve,TRACE
from a2a.server.routes import create_jsonrpc_routes
from google.protobuf.json_format import MessageToDict
from starlette.applications import Starlette
from starlette.routing import Route
from starlette.responses import JSONResponse
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.resources import Resource
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator
assert version('opentelemetry-sdk')=='1.44.0'
endpoint,mode=sys.argv[1:]
assert endpoint.startswith('http://127.0.0.1:') and mode in ('negotiated','generic')
provider=TracerProvider(resource=Resource.create({'service.name':'pablo-independent-a2a-peer'}))
provider.add_span_processor(SimpleSpanProcessor(OTLPSpanExporter(endpoint=endpoint,timeout=1)))
tracer=provider.get_tracer('a2a-r03-peer')
headers=contextvars.ContextVar('request_headers')
class Handler:
    async def on_message_send_stream(self,params,context):
        record('SendStreamingMessage')
        assert params.configuration.history_length==0 and len(params.message.parts)==1
        assert params.message.parts[0].text=='PRIVATE_REMOTE_PART'
        assert not params.message.metadata
        metadata=MessageToDict(params.metadata)
        incoming=headers.get()
        if mode=='negotiated':
            assert set(metadata)=={TRACE}
            carrier=metadata[TRACE];assert set(carrier)<= {'traceparent','tracestate'}
            assert list(params.message.extensions)==[TRACE]
            assert incoming['a2a-extensions']==TRACE
            assert incoming['traceparent']==carrier['traceparent']
            assert incoming.get('tracestate')==carrier.get('tracestate')
        else:
            assert not metadata and not params.message.extensions
            assert not any(key in incoming for key in ('traceparent','tracestate','a2a-extensions'))
            carrier={}
        parent=TraceContextTextMapPropagator().extract(carrier)
        with tracer.start_as_current_span('a2a.remote_task',context=parent,attributes={'a2a.task.id':'remote-task','a2a.context.id':'remote-context'}):
            yield value('stream_status','statusUpdate',p.TaskStatusUpdateEvent)
            yield value('stream_artifact','artifactUpdate',p.TaskArtifactUpdateEvent)
        yield p.TaskStatusUpdateEvent(task_id='remote-task',context_id='remote-context',status=p.TaskStatus(state=p.TASK_STATE_COMPLETED),metadata={'urn:pablo:a2a:reported-usage:v1':{'totalTokens':'20','costMicrousd':'42'}})
async def card(request):
    record('GetAgentCard')
    data=json.loads(Path(__file__).with_name('card.json').read_text())
    if mode=='negotiated':data['capabilities']['extensions']=[{'uri':TRACE}]
    return JSONResponse(data)
sdk=Starlette(routes=[Route('/.well-known/agent-card.json',card),*create_jsonrpc_routes(Handler(),'/rpc',enable_v0_3_compat=False)])
async def app(scope,receive,send):
    if scope['type']=='http':
        incoming={k.decode():v.decode() for k,v in scope['headers']}
        assert not any(key in incoming for key in ('authorization','baggage'))
        token=headers.set(incoming)
        try:await sdk(scope,receive,send)
        finally:headers.reset(token)
    else:await sdk(scope,receive,send)
try:serve(app)
finally:provider.shutdown()
