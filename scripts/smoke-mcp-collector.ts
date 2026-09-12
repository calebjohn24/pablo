/** M04 real Collector proof, explicitly run with isolated independent MCP fixtures. */
import assert from 'node:assert/strict';
import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { promisify } from 'node:util';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { body, cleanEnv, server, traceId, parentId, traceparent } from '../tests/fixtures/telemetry.ts';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';
// @ts-expect-error Dependency-free local JS helper.
import { collectorBinary, lock } from './lib/collector.mjs';

const exec=promisify(execFile);
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(root,'target/debug/pablo');
const python=join(root,'.pablo/mcp-fixture-venv/bin/python');
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-collector-')));
const received:any[]=[];
const proof=await server(async(req,res)=>{received.push(JSON.parse((await body(req)).toString()));res.writeHead(200,{'content-type':'application/json'});res.end('{}');});
const reservation=await server((_req,res)=>{res.end();});const listen=`127.0.0.1:${reservation.port}`;await reservation.close();
const collector=spawn(await collectorBinary(),['--config',join(root,'tests/fixtures/collector/config.yaml')],{env:{...cleanEnv(),PABLO_COLLECTOR_LISTEN:listen,PABLO_COLLECTOR_PROOF:proof.url},stdio:['ignore','pipe','pipe']});
let diagnostics='';collector.stdout.on('data',b=>{diagnostics+=b.toString();});collector.stderr.on('data',b=>{diagnostics+=b.toString();});
async function stop(child:ChildProcess){if(child.exitCode!==null||child.signalCode!==null)return;const exited=once(child,'exit');child.kill('SIGTERM');const timer=setTimeout(()=>child.kill('SIGKILL'),3000);try{await exited;}finally{clearTimeout(timer);}}
const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
const secret='synthetic-mcp-evidence-private';
const gateway=await server(async(req,res)=>{
  const value=JSON.parse((await body(req)).toString());assert.equal(value.tools.length,1);
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(value.messages.at(-1).role==='tool'){
    const result=JSON.parse(value.messages.at(-1).content);assert.equal(result.mcp.content.structured.text,secret);
    res.end(frame({content:'Verified.'})+frame({},'stop')+'data: [DONE]\n\n');
  }else res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:value.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');
});
const events:any[]=[];const contexts:any[]=[];const sessionTokens:string[]=[];
try{
  let ready=false;for(let i=0;i<100;i++){assert.equal(collector.exitCode,null,diagnostics);try{await fetch(`http://${listen}/v1/traces`,{signal:AbortSignal.timeout(100)});ready=true;break;}catch{await delay(50);}}assert(ready);
  for(const mode of ['stdio','json','sse']){
    const workspace=join(cwd,mode);await mkdir(workspace);await writeFile(join(workspace,'evidence.txt'),secret);
    let peer:ChildProcess|undefined;let endpoint='';
    if(mode!=='stdio'){
      peer=spawn(python,[join(root,'tests/fixtures/mcp/http_server.py'),mode],{cwd:workspace,env:{},stdio:['ignore','ignore','ignore']});
      for(let i=0;i<500;i++){try{endpoint=await readFile(join(workspace,'endpoint'),'utf8');if(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/.test(endpoint))break;}catch{}assert.equal(peer.exitCode,null);await delay(10);}assert(endpoint);
    }
    try{
      const entry=join(workspace,'entry.toml');await writeFile(entry,`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="UNREAD_PROVIDER"}]
[credentials.mcp]
consumer="${mode==='stdio'?'mcp.env':'mcp.headers'}"
sources=[{kind="environment",name="MCP_TOKEN"}]
[credentials.exporter]
consumer="otel.headers"
sources=[{kind="environment",name="EXPORTER_TOKEN"}]
[options.shell]
enabled=false
[options.filesystem]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
[options.otel]
exporter="otlp"
endpoint="http://${listen}/v1/traces"
headers="exporter"
sampler="always_on"
schedule_delay_ms=60000
[options.mcp.servers.local]
${mode==='stdio'?`transport="stdio"\ncommand=${JSON.stringify(python)}\nargs=[${JSON.stringify(join(root,'tests/fixtures/mcp/server.py'))}]\nenv={FIXTURE_TOKEN="mcp"}`:'transport="http"\nurl="https://synthetic.example/mcp"\nheaders={x-fixture-token="mcp"}'}
`);
      const args=['--config',entry,'--bind',`workspace=${workspace}`,'--fixture-endpoint',gateway.url,...(mode==='stdio'?[]:['--fixture-mcp-endpoint',`local=${endpoint}`])];
      const env={...cleanEnv(),MCP_TOKEN:mode==='stdio'?'synthetic-mcp-token':'synthetic-http-token',EXPORTER_TOKEN:'x-fixture=synthetic-exporter-private',AI_GATEWAY_API_KEY:'synthetic-provider-private'};
      const cli=await exec(binary,['run','Read evidence','--traceparent',traceparent,...args],{cwd:workspace,env,timeout:10000});assert.equal(cli.stdout.trim(),'Verified.');contexts.push(JSON.parse(await readFile(join(workspace,'context.json'),'utf8')));
      await withPablo({binary,args,env},async cx=>{
        await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true}}});
        const {sessionId}=await cx.request('session/new',{cwd:workspace,mcpServers:[]});
        const result=outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read evidence'}],_meta:{'pablo/v2':{traceparent}}}));assert.equal(result.status,'completed');
        contexts.push(JSON.parse(await readFile(join(workspace,'context.json'),'utf8')));
      });
      for(const name of await readdir(workspace))if(/^[0-9a-f-]+\.jsonl$/.test(name))events.push(...(await readFile(join(workspace,name),'utf8')).trim().split('\n').map(line=>JSON.parse(line)));
      if(mode!=='stdio')sessionTokens.push(...(await readFile(join(workspace,'requests.jsonl'),'utf8')).trim().split('\n').map(line=>JSON.parse(line).session).filter(Boolean));
    }finally{if(peer)await stop(peer);}
  }
  for(let i=0;i<100&&received.flatMap(r=>r.resourceSpans??[]).flatMap(r=>r.scopeSpans.flatMap((s:any)=>s.spans)).length<24;i++)await delay(20);
  const spans=received.flatMap(r=>r.resourceSpans).flatMap(r=>r.scopeSpans.flatMap((s:any)=>s.spans));assert.equal(spans.length,24);
  const roots=spans.filter(s=>s.name==='invoke_agent pablo');assert.equal(roots.length,6);
  const tools=spans.filter(s=>s.name==='execute_tool mcp/local/read_evidence');assert.equal(tools.length,6);
  for(const span of spans){
    assert.equal(span.traceId,traceId);assert.equal(span.parentSpanId,span.name==='invoke_agent pablo'?parentId:events.find(e=>e.span_id===span.spanId).parent_span_id);
    const native=events.filter(e=>e.span_id===span.spanId);assert(native.length>=2);
    assert.equal(BigInt(span.startTimeUnixNano),BigInt(native[0].timestamp_unix_micros)*1000n);assert.equal(BigInt(span.endTimeUnixNano),BigInt(native.at(-1).timestamp_unix_micros)*1000n);
  }
  for(const span of tools){
    assert.equal(span.kind,3);const attrs=Object.fromEntries(span.attributes.map((a:any)=>[a.key,a.value.stringValue??a.value.intValue]));assert.equal(attrs['mcp.method.name'],'tools/call');assert.equal(attrs['mcp.protocol.version'],'2025-11-25');
    const context=contexts.find(c=>c.traceparent===`00-${traceId}-${span.spanId}-01`);assert(context);assert.equal(String(context.request_id),String(attrs['jsonrpc.request.id']));
  }
  const serialized=JSON.stringify(received)+diagnostics;
  for(const value of [secret,'synthetic-mcp-token','synthetic-http-token','synthetic-provider-private','synthetic-exporter-private',...sessionTokens])assert(!serialized.includes(value));
  console.log(JSON.stringify({collector:lock.version,platform:`${process.platform}/${process.arch}`,surfaces:['cli','acp'],transports:['stdio','http-json','http-sse'],spans:spans.length,logicalMcpSpans:tools.length,exactNativeTimestamps:true,propagatedToolContext:true,metadataOnly:true}));
}finally{await stop(collector);await gateway.close();await proof.close();await rm(cwd,{recursive:true,force:true});}
assert.equal(collector.exitCode,0,'Collector must shut down cleanly');
