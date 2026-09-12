/** R02 end-to-end ACP delegation baseline against the pinned independent A2A SDK. */
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from '../tests/fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
// @ts-expect-error Dependency-free source fingerprint helper.
import { sourceFingerprint } from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(root,'target/release/pablo');
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-a2a-measure-')));
const peer=spawn(join(root,'.pablo/a2a-fixture-venv/bin/python'),[join(root,'tests/fixtures/a2a/wire_server.py')],{cwd,env:{},stdio:['ignore','pipe','pipe']});
const exited=new Promise<void>((resolve,reject)=>{peer.once('error',reject);peer.once('close',()=>resolve());});
peer.stderr.resume();
const ready=new Promise<string>((resolve,reject)=>{
  let text='';const timer=setTimeout(()=>reject(new Error('SDK readiness timeout')),5000);
  peer.stdout.on('data',chunk=>{text+=chunk.toString();if(text.length>4096){clearTimeout(timer);reject(new Error('SDK readiness bound'));}else if(text.includes('\n')){clearTimeout(timer);resolve(JSON.parse(text.split('\n')[0]).url);}});
  exited.then(()=>{clearTimeout(timer);reject(new Error('SDK exited'));},reject);
});
let gateway:Awaited<ReturnType<typeof server>>|undefined;
const checkpoint=process.env.PABLO_MEASURE_CHECKPOINT??'C3.27';
assert(/^C[0-9]+\.[0-9]+$/.test(checkpoint));
const samples:number[]=[];
try {
  const rpc=await ready;
  gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());
    const results=request.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
    let delta:object,finish='tool_calls';
    const call=(args:object)=>({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]});
    if(results.length===0)delta=call({action:'spawn_remote',remote_request:{remote:'peer',parts:[{text:'assembly'}],accepted_output_modes:['text/plain','application/octet-stream'],stream:true}});
    else if(results.length===1)delta=call({action:'wait',agent_ids:[results[0].subagent.agent.agent_id],mode:'all',timeout_ms:5000});
    else {
      assert.equal(results.length,2);assert.deepEqual(results[1].subagent.remaining,[]);
      const child=results[1].subagent.settled[0];assert.equal(child.outcome.status,'completed');
      assert.equal(child.remote.result.artifacts[0].parts[0].text,'first');assert.equal(child.remote.remote_reported_usage.totalTokens,'20');
      assert.equal(child.accounting.model_calls,'0');delta={content:'verified'};finish='stop';
    }
    res.writeHead(200,{'content-type':'text/event-stream'});
    res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\ndata: [DONE]\n\n`);
  });
  const base=(await readFile(join(root,'docs/project/fixtures/c3-model-routes/three-providers.toml'),'utf8')).replace('max_model_calls=2','max_model_calls=20').replace('max_tool_calls=1','max_tool_calls=10');
  const entry=join(cwd,'entry.toml');
  await writeFile(entry,base+'\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=false\npath={base="workspace",path="trace-{session_id}.jsonl"}\n[options.a2a.remotes.peer]\ncard_url="https://agent.example.test/.well-known/agent-card.json"\nendpoint="https://agent.example.test/rpc"\n');
  const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--fixture-a2a-endpoint',`peer=${rpc}`];
  for(let i=0;i<35;i++){
    const started=performance.now();
    await withPablo({binary,args,env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      const result=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'explicit remote work'}]}));
      assert.equal(result.outcome.status,'completed');assert.equal(result.accounting.model_calls,'3');assert.equal(result.accounting.tool_calls,'2');
    });
    if(i>=5)samples.push(performance.now()-started);
  }
} finally {
  try {if(gateway)await gateway.close();} finally {
    if(peer.exitCode===null)peer.kill('SIGTERM');const timer=setTimeout(()=>peer.kill('SIGKILL'),3000);
    try {await exited;} finally {clearTimeout(timer);await rm(cwd,{recursive:true,force:true});}
  }
}
const sorted=samples.toSorted((a,b)=>a-b);
const report={checkpoint,timestamp:new Date().toISOString(),source_sha256:await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),platform:`${process.platform}/${process.arch}`,node:process.version,method:{samples:30,warmup:5,timing:'Fresh ACP process initialize/session/prompt and joined shutdown; three mock model calls, two subagent tool calls, card GET, streamed SDK task/artifact, typed result validation, metadata-only trace. SDK/gateway setup excluded; SDK remains alive between samples.',comparison:'End-to-end supervised workload baseline, not an isolated supervisor overhead delta or live network/model/TLS measurement. Sequential after builds/tests.',cleanup:'SDK process joined and temporary workspace removed before report'},stats:{fresh_acp_ms:{min:sorted[0],p50:sorted[14],p95:sorted[28],max:sorted[29]}},samples_ms:samples};
await writeFile(join(root,`.pablo/measurements/${checkpoint.toLowerCase()}-supervised.json`),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report));
