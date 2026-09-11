/** R03 independent peer span through a real pinned Collector, plus outage parity. */
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {setTimeout as delay} from 'node:timers/promises';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from '../tests/fixtures/telemetry.ts';
import {a2aTelemetry} from '../tests/fixtures/a2a-telemetry.ts';
// @ts-expect-error Dependency-free pinned Collector helper.
import {collectorBinary,lock} from './lib/collector.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const received:any[]=[];
const proof=await server(async(req,res)=>{received.push(JSON.parse((await body(req)).toString()));res.writeHead(200,{'content-type':'application/json'});res.end('{}');});
const reservation=await server((_req,res)=>{res.end();});const listen=`127.0.0.1:${reservation.port}`;await reservation.close();
const collector=spawn(await collectorBinary(),['--config',join(root,'tests/fixtures/collector/config.yaml')],{env:{...cleanEnv(),PABLO_COLLECTOR_LISTEN:listen,PABLO_COLLECTOR_PROOF:proof.url},stdio:['ignore','pipe','pipe']});
let diagnostics='';collector.stdout.on('data',b=>{diagnostics+=b.toString();});collector.stderr.on('data',b=>{diagnostics+=b.toString();});
async function stop(){if(collector.exitCode!==null||collector.signalCode!==null)return;const exited=once(collector,'exit');collector.kill('SIGTERM');const kill=setTimeout(()=>collector.kill('SIGKILL'),3000);try{await exited;}finally{clearTimeout(kill);}}
const endpoint=`http://${listen}/v1/traces`;
const spans=()=>received.flatMap(r=>r.resourceSpans??[]).flatMap(r=>r.scopeSpans.flatMap((s:any)=>s.spans));
try {
  let ready=false;for(let i=0;i<100;i++){assert.equal(collector.exitCode,null,diagnostics);try{await fetch(endpoint,{signal:AbortSignal.timeout(100)});ready=true;break;}catch{await delay(50);}}assert(ready);
  const results=[];
  for(const mode of ['negotiated','generic'] as const){
    const before=spans().length;
    const result=await a2aTelemetry(endpoint,mode,process.argv[2]);results.push(result);
    const beginnings=result.events.filter(e=>['run.started','model.started','tool.started'].includes(e.type));
    for(let i=0;i<100&&spans().length<before+beginnings.length+1;i++)await delay(20);
    const actual=spans().slice(before);assert.equal(actual.length,beginnings.length+1);
    const proxy=result.events.find(e=>e.type==='run.started'&&e.agent.kind==='remote_a2a');assert(proxy);
    for(const event of beginnings){
      const matches=actual.filter(s=>s.traceId===event.trace_id&&s.spanId===event.span_id);assert.equal(matches.length,1);
      const span=matches[0];assert.equal(span.parentSpanId??'',event.parent_span_id??'');
      const own=result.events.filter(e=>e.span_id===event.span_id);
      assert.equal(BigInt(span.startTimeUnixNano),BigInt(own[0].timestamp_unix_micros)*1000n);assert.equal(BigInt(span.endTimeUnixNano),BigInt(own.at(-1).timestamp_unix_micros)*1000n);
      const attrs=Object.fromEntries(span.attributes.map((a:any)=>[a.key,a.value.stringValue??a.value.intValue]));assert.equal(attrs['pablo.agent.id'],event.agent.agent_id);
    }
    const peer=actual.find(s=>s.name==='a2a.remote_task');assert(peer);
    if(mode==='negotiated'){assert.equal(peer.traceId,proxy.trace_id);assert.equal(peer.parentSpanId,proxy.span_id);}else{assert.notEqual(peer.traceId,proxy.trace_id);assert(!peer.parentSpanId);}
    const attrs=Object.fromEntries(peer.attributes.map((a:any)=>[a.key,a.value.stringValue]));assert.equal(attrs['a2a.task.id'],result.snapshot.remote.remote.task_id);assert.equal(attrs['a2a.context.id'],result.snapshot.remote.remote.context_id);
    const serialized=JSON.stringify(received)+JSON.stringify(result.events)+result.diagnostics+diagnostics;
    for(const privateText of ['PRIVATE_ROOT_CONTEXT','PRIVATE_REMOTE_PART'])assert(!serialized.includes(privateText));
  }
  await stop();assert.equal(collector.exitCode,0);
  const outage=await a2aTelemetry(endpoint,'negotiated',process.argv[2]);assert.deepEqual(outage.task.outcome,results[0].task.outcome);assert.deepEqual(outage.task.accounting,results[0].task.accounting);assert.deepEqual(outage.snapshot.outcome,results[0].snapshot.outcome);assert.deepEqual(outage.snapshot.accounting,results[0].snapshot.accounting);assert.deepEqual(outage.snapshot.remote.result,results[0].snapshot.remote.result);
  const outageMetadata=JSON.stringify(outage.events)+outage.diagnostics;for(const privateText of ['PRIVATE_ROOT_CONTEXT','PRIVATE_REMOTE_PART'])assert(!outageMetadata.includes(privateText));
  console.log(JSON.stringify({checkpoint:'C3.28',collector:lock.version,peerOtelSdk:'1.44.0',platform:`${process.platform}/${process.arch}`,spans:spans().length,negotiatedRemoteParent:true,genericIndependentTrace:true,exactNativeIdentitiesAndTimestamps:true,metadataOnly:true,exporterOutageOutcomeAccountingAndRemoteResultUnchanged:true}));
}finally{await stop();await proof.close();}
