"""Fault injector, deliberately independent of Pablo's Rust serializer."""
import json, os, sys, time, subprocess
from pathlib import Path
mode=sys.argv[1]
Path('pid').write_text(str(os.getpid()))
if mode=='startup_hang': time.sleep(60)
def emit(value):
    sys.stdout.write(json.dumps(value,separators=(',',':'))+'\n');sys.stdout.flush()
def tool(name='read'):
    return {'name':name,'description':'Synthetic tool','inputSchema':{'type':'object','additionalProperties':False},'outputSchema':{'type':'object','properties':{'text':{'type':'string'}},'required':['text'],'additionalProperties':False}}
page=0
for line in sys.stdin:
    message=json.loads(line); method=message.get('method'); request_id=message.get('id')
    if request_id is None: continue
    if method=='initialize':
        if mode=='frame_overflow':
            sys.stdout.write('x'*(2*1024*1024+1));sys.stdout.flush();time.sleep(60)
        result={'protocolVersion':'2020-01-01' if mode=='wrong_version' else '2025-11-25','capabilities':{} if mode=='no_tools' else {'tools':{}},'serverInfo':{'name':'synthetic','version':'1'}}
    elif method=='tools/list':
        page+=1
        tools=[tool(str(page) if mode=='cursor' else 'read')]
        if mode=='duplicate':tools.append(tool())
        if mode=='catalog_bound':tools=[tool(str(i)) for i in range(65)]
        if mode=='schema':tools[0]['inputSchema']={'type':'object','$ref':'https://example.invalid/private-schema'}
        result={'tools':tools}
        if mode=='cursor':result['nextCursor']='repeat'
    elif method=='tools/call':
        Path('called').write_text('once')
        if mode=='call_hang':continue
        if mode=='descendant':
            child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])
            Path('descendant').write_text(str(child.pid))
        if mode=='stderr':os.write(2,b'x'*(1024*1024))
        if mode in ('progress','progress_overflow'):
            token=message['params']['_meta']['progressToken']
            for i in range(65 if mode=='progress_overflow' else 8):emit({'jsonrpc':'2.0','method':'notifications/progress','params':{'progressToken':token,'progress':i}})
        if mode=='rpc_error':
            emit({'jsonrpc':'2.0','id':request_id,'error':{'code':-32000,'message':'PRIVATE_PEER_ERROR'}});continue
        result={'content':[{'type':'text','text':'synthetic result'}],'structuredContent':{'text':42 if mode=='invalid_result' else 'synthetic result'},'isError':mode=='tool_error'}
        if mode=='tool_error':result.pop('structuredContent')
        if mode=='unsupported':result['content']=[{'type':'image','data':'AA==','mimeType':'image/png'}]
    else:raise AssertionError(method)
    emit({'jsonrpc':'2.0','id':request_id,'result':result})
