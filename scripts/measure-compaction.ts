/** Local core/CLI/ACP compaction timings; synthetic provider, no live credentials. */
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createHash} from 'node:crypto';
import {mkdir,mkdtemp,readFile,realpath,rm,stat,writeFile} from 'node:fs/promises';
import {cpus,release,tmpdir} from 'node:os';
import {dirname,join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {withPablo,taskOf} from '../examples/acp-client.ts';
import {body,cleanEnv,server} from '../tests/fixtures/telemetry.ts';
import {responsesEvents,responsesWire} from '../tests/fixtures/open-responses.ts';
// @ts-expect-error Dependency-free project helper.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const exec=promisify(execFile), root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const provider=process.env.PABLO_MEASURE_PROVIDER??'vercel', trigger=process.env.PABLO_MEASURE_TRIGGER??'threshold';
assert(['vercel','openrouter','open_responses'].includes(provider));assert(['threshold','overflow'].includes(trigger));
const count=Number(process.argv[2]??30);assert(Number.isInteger(count)&&count>=10&&count<=1000);
const destination=resolve(process.argv[3]??`.pablo/measurements/c3.12a-${provider}-${trigger}.json`);
const binary=resolve(process.env.PABLO_MEASURE_BINARY??'target/release/pablo'),direct=resolve(process.env.PABLO_MEASURE_DIRECT??'target/release/examples/measure');
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-measure-compaction-'))),env=cleanEnv();
const model=provider==='vercel'?'zai/glm-5.3-flash':provider==='openrouter'?'z-ai/glm-5.3-flash':'fixture-text-tools-v1';
const summaryText='Goal: inspect evidence.txt. Keep original constraints and artifact revision. Three reads completed; continue unresolved work without replaying effects.';
const textOf=(m:any)=>typeof m.content==='string'?m.content:(m.content??[]).map((p:any)=>p.text??'').join('');
let steps=0,summaries=0,calls=0,overflow=false,overflowAt=0,summaryAt=0,recoveryAt=0;
const gateway=await server(async(req,res)=>{
  assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');const r=JSON.parse((await body(req)).toString());assert.equal(r.model,model);calls++;
  const messages=r.messages??r.input, summarizing=textOf(messages.at(-1)).startsWith('Create a concise handoff');
  const respond=(output:{chunks?:string[],call?:{id:string,name:string,arguments:string}})=>{
    res.writeHead(200,{'content-type':'text/event-stream'});
    if(provider==='open_responses'){res.end(responsesWire(responsesEvents(r,output)));return;}
    const delta=output.call?{tool_calls:[{index:0,id:output.call.id,type:'function',function:{name:output.call.name,arguments:output.call.arguments}}]}:{content:output.chunks!.join('')};
    res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:output.call?'tool_calls':'stop'}]})}\n\ndata: [DONE]\n\n`);
  };
  if(summarizing){summaryAt=performance.now();summaries++;assert.equal(summaries,1);assert.equal(r.tool_choice,'none');respond({chunks:[summaryText]});return;}
  if(summaries){recoveryAt=performance.now();assert(textOf(messages.find((m:any)=>m.role==='user'&&textOf(m).startsWith('Derived history'))).includes(summaryText));respond({chunks:['done']});return;}
  if(steps<3){steps++;respond({call:{id:`read${steps}`,name:'fs_read',arguments:'{"path":"evidence.txt"}'}});return;}
  assert.equal(trigger,'overflow');assert(!overflow);overflow=true;overflowAt=performance.now();res.writeHead(400,{'content-type':'application/json'});res.end('{"error":{"code":"context_length_exceeded"}}');
});
const metrics:Record<string,number[]>={};
let recording=false;
const record=(key:string,value:number)=>{assert(Number.isFinite(value)&&value>=0,key);if(recording)(metrics[key]??=[]).push(value);};
const reset=()=>{steps=0;summaries=0;calls=0;overflow=false;overflowAt=summaryAt=recoveryAt=0;};
const verify=()=>{assert.equal(steps,3);assert.equal(summaries,1);assert.equal(calls,trigger==='threshold'?5:6);};
const inspect=(events:any[],path:string)=>{
  const start=events.find(e=>e.type==='context.compaction.started'),end=events.find(e=>e.type==='context.compaction.finished');
  assert(start&&end);const meta=end.compaction??end['pablo/v1'].compaction;assert.equal(meta.status,'completed');assert.equal(meta.trigger,trigger);
  const before=Number(meta.before_bytes),after=Number(meta.after_bytes);assert(after<before);
  record(`${path}_before_bytes`,before);record(`${path}_after_bytes`,after);record(`${path}_reduction_percent`,100*(before-after)/before);
  if(path!=='acp')record(`${path}_compaction_ms`,(end.timestamp_unix_micros-start.timestamp_unix_micros)/1000);
  record(`${path}_summary_request_to_generation_ms`,recoveryAt-summaryAt);
  if(trigger==='overflow')record(`${path}_overflow_to_recovery_ms`,recoveryAt-overflowAt);
};
try{
  await writeFile(join(cwd,'evidence.txt'),'e'.repeat(6000));const entry=join(cwd,'deployment.toml');
  await writeFile(entry,`schema_version=1
[options.model]
provider="${provider}"
id="${model}"
credential="key"
context_window_tokens=${trigger==='threshold'?'8000':'{unset=true}'}
${provider==='open_responses'?'endpoint="https://responses.example.test/v1/responses"\ncapability_profile="open-responses-text-tools-v1"':''}
[credentials.key]
consumer="provider.${provider}"
sources=[{kind="environment",name="UNREAD_COMPACTION_KEY"}]
[options.context]
max_summary_tokens=128
[options.limits]
max_output_tokens=2048
max_model_calls=8
[options.shell]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
`);
  const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
  // One warm ACP process, fresh task/session each time; rotate paths per sample.
  let notifications:any[]=[],compactionStarted=0,compactionEnded=0;
  await withPablo({binary,args,env,onCompaction:n=>{const e=n as any;notifications.push(e);if(e.type==='context.compaction.started')compactionStarted=performance.now();else compactionEnded=performance.now();}},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true,'pablo/compaction-v1':true}}});
    for(let sample=0;sample<count+5;sample++){
      recording=sample>=5;
      for(let i=0;i<3;i++){
        const path=['core','cli','acp'][(sample+i)%3];reset();notifications=[];const task='Inspect evidence in three steps.';
        if(path==='acp'){
          const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});const start=performance.now();
          const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:task}]});record('acp_warm_prompt_ms',performance.now()-start);assert.equal(taskOf(response).outcome.status,'completed');
          inspect(notifications,path);record('acp_compaction_notification_ms',compactionEnded-compactionStarted);await rm(join(cwd,`${sessionId}.jsonl`));
        }else{
          const trace=join(cwd,`${path}.jsonl`);const start=performance.now();
          const result=JSON.parse((await exec(path==='core'?direct:binary,path==='core'?['http',gateway.url,task,...args,'--trace',trace]:['run',task,...args,'--json','--trace',trace],{env,timeout:15000})).stdout);
          record(`${path}_process_ms`,performance.now()-start);assert.equal(result.outcome.output,'done');if(path==='core')record('core_run_ms',result.run_ms);
          inspect((await readFile(trace,'utf8')).trim().split('\n').map(s=>JSON.parse(s)),path);await rm(trace);
        }
        verify();
      }
    }
  });
  const stats=Object.fromEntries(Object.entries(metrics).map(([k,v])=>{const s=v.toSorted((a,b)=>a-b);const p=(n:number)=>s[Math.ceil(n*s.length)-1];return[k,{n:s.length,min:s[0],p50:p(.5),p95:p(.95),p99:p(.99),max:s.at(-1)}];}));
  const report={schema_version:1,checkpoint:process.env.PABLO_MEASURE_CHECKPOINT??'C3.12b',timestamp:new Date().toISOString(),source_sha256:process.env.PABLO_MEASURE_SOURCE_SHA256??await sourceFingerprint(root),harness_sha256:createHash('sha256').update(await readFile(fileURLToPath(import.meta.url))).digest('hex'),platform:{os:process.platform,arch:process.arch,kernel:release(),cpu:cpus()[0].model,node:process.version},build:{binary_bytes:(await stat(binary)).size,binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex')},method:{provider,model,trigger,samples:count,warmup:5,workload:'three actual 6000-byte file reads, one bounded summary, final continuation; overflow adds one HTTP 400',timing:'core/CLI buffered redacted JSONL; fresh processes; ACP fresh sessions in one warm process with negotiated notifications and redacted trace; rotated paths, monotonic host/request clocks, native UTC span timestamps',limits:'loopback synthetic latency only; no live model throughput or summary quality claim; no private reasoning fixture in this performance workload'},stats};
  await mkdir(dirname(destination),{recursive:true});await writeFile(destination,JSON.stringify(report,null,2)+'\n',{mode:0o600});console.log(JSON.stringify({file:destination,stats}));
}finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
