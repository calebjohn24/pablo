/** E01 real Collector identity/privacy proof and exporter-outage control. */
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {setTimeout as delay} from 'node:timers/promises';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from '../tests/fixtures/telemetry.ts';
import {extensibilityFixture} from '../tests/fixtures/extensibility.ts';
// @ts-expect-error Dependency-free pinned Collector helper.
import {collectorBinary,lock} from './lib/collector.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const received:any[]=[];
const proof=await server(async(req,res)=>{received.push(JSON.parse((await body(req)).toString()));res.writeHead(200,{'content-type':'application/json'});res.end('{}');});
const reservation=await server((_req,res)=>{res.end();});
const listen=`127.0.0.1:${reservation.port}`;await reservation.close();
const collector=spawn(await collectorBinary(),['--config',join(root,'tests/fixtures/collector/config.yaml')],{env:{...cleanEnv(),PABLO_COLLECTOR_LISTEN:listen,PABLO_COLLECTOR_PROOF:proof.url},stdio:['ignore','pipe','pipe']});
let diagnostics='';collector.stdout.on('data',b=>{diagnostics+=b.toString();});collector.stderr.on('data',b=>{diagnostics+=b.toString();});
async function stop(){
  if(collector.exitCode!==null||collector.signalCode!==null)return;
  const exited=once(collector,'exit');collector.kill('SIGTERM');
  const kill=setTimeout(()=>collector.kill('SIGKILL'),3000);
  try{await exited;}finally{clearTimeout(kill);}
}
const otel=`[options.otel]\nexporter="otlp"\nendpoint="http://${listen}/v1/traces"\nsampler="always_on"\nschedule_delay_ms=60000\n`;
try{
  let ready=false;
  for(let i=0;i<100;i++){
    assert.equal(collector.exitCode,null,diagnostics);
    try{await fetch(`http://${listen}/v1/traces`,{signal:AbortSignal.timeout(100)});ready=true;break;}catch{await delay(50);}
  }
  assert(ready);
  const fixture=await extensibilityFixture('complete',otel);
  let expectedTask:any,events:any[]=[],context:any;
  try{expectedTask=await fixture.run();({events,context}=await fixture.verify(expectedTask));}finally{await fixture.close();}
  const beginnings=events.filter(e=>['run.started','model.started','tool.started','context.compaction.started'].includes(e.type));
  for(let i=0;i<100&&received.flatMap(r=>r.resourceSpans??[]).flatMap(r=>r.scopeSpans.flatMap((s:any)=>s.spans)).length<beginnings.length;i++)await delay(20);
  const spans=received.flatMap(r=>r.resourceSpans??[]).flatMap(r=>r.scopeSpans.flatMap((s:any)=>s.spans));
  const validations=spans.filter(s=>s.name==='validate_output');
  assert.equal(validations.length,4);assert.equal(spans.length,beginnings.length+validations.length);assert.equal(spans.length,37);
  for(const span of validations){
    const attrs=Object.fromEntries(span.attributes.map((a:any)=>[a.key,a.value.stringValue??a.value.intValue]));
    const terminal=events.find(e=>e.type==='run.finished'&&e.agent.agent_id===attrs['pablo.agent.id']);
    assert(terminal);assert.equal(span.traceId,terminal.trace_id);assert.equal(span.parentSpanId,terminal.span_id);
    assert.equal(attrs['pablo.root.run.id'],terminal.agent.root_run_id);
    assert.equal(attrs['pablo.output.schema_sha256'],terminal.output_validation.schema_sha256);
    assert(BigInt(span.startTimeUnixNano)<=BigInt(span.endTimeUnixNano));
    assert(BigInt(span.endTimeUnixNano)<=BigInt(terminal.timestamp_unix_micros)*1000n);
  }
  for(const event of beginnings){
    const matches=spans.filter(s=>s.traceId===event.trace_id&&s.spanId===event.span_id);assert.equal(matches.length,1);
    const span=matches[0];assert.equal(span.parentSpanId??'',event.parent_span_id??'');
    const attrs=Object.fromEntries(span.attributes.map((a:any)=>[a.key,a.value.stringValue??a.value.intValue]));
    assert.equal(attrs['pablo.agent.id'],event.agent.agent_id);assert.equal(attrs['pablo.root.run.id'],event.agent.root_run_id);
    const native=events.filter(e=>e.span_id===event.span_id);
    assert.equal(BigInt(span.startTimeUnixNano),BigInt(native[0].timestamp_unix_micros)*1000n);
    assert.equal(BigInt(span.endTimeUnixNano),BigInt(native.at(-1).timestamp_unix_micros)*1000n);
  }
  const mcp=spans.filter(s=>s.name==='execute_tool mcp/local/read_evidence');assert.equal(mcp.length,2);
  assert(mcp.some(s=>context.traceparent===`00-${s.traceId}-${s.spanId}-01`));
  assert.equal(spans.filter(s=>s.name==='invoke_agent pablo').length,4);
  assert.equal(spans.filter(s=>s.name==='execute_tool skill.read').length,1);
  assert.equal(spans.filter(s=>s.name==='execute_tool fs.read').length,1);
  assert.equal(spans.filter(s=>s.name==='compact_context').length,1);
  const serialized=JSON.stringify(received)+diagnostics;
  for(const secret of ['PRIVATE_E01_ROOT','PRIVATE_E01_SKILL','PRIVATE_E01_RESOURCE','PRIVATE_E01_EVIDENCE','synthetic-mcp-token'])assert(!serialized.includes(secret));
  await stop();assert.equal(collector.exitCode,0);
  const outage=await extensibilityFixture('complete',otel);
  try{
    const actual=await outage.run();await outage.verify(actual);
    assert.deepEqual(actual.outcome,expectedTask.outcome);assert.deepEqual(actual.accounting,expectedTask.accounting);
  }finally{await outage.close();}
  console.log(JSON.stringify({checkpoint:'C3.25',collector:lock.version,platform:`${process.platform}/${process.arch}`,surface:'acp',spans:spans.length,rootAndChildRuns:4,models:17,tools:11,compactions:1,validations:4,mcpTools:2,exactNativeIdentities:true,exactNativeTimestamps:true,metadataOnly:true,exporterOutageOutcomeAndAccountingUnchanged:true}));
}finally{await stop();await proof.close();}
