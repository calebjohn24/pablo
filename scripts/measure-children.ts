/** Local configured child overhead; run after all builds and tests finish. */
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,writeFile,readFile,realpath,rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from '../tests/fixtures/telemetry.ts';
import {withPablo,taskOf} from '../examples/acp-client.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(root,'target/release/pablo'), exec=promisify(execFile);
const childCount=Number(process.env.PABLO_MEASURE_CHILDREN??'1');
assert(childCount===1||childCount===2);
const waiting:Array<()=>void>=[];
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-child-measure-')));
const wire=(delta:object,finish='stop')=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\ndata: [DONE]\n\n`;
const gateway=await server(async(req,res)=>{
  const r=JSON.parse((await body(req)).toString());
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(r.model==='zai/glm-5.3-flash') {
    waiting.push(()=>res.end(wire({content:'done'})));
    // Two-child measurements require both model requests to arrive before either
    // completes. The one-child mode retains its original immediate response.
    if(waiting.length===childCount)for(const finish of waiting.splice(0))finish();
    return;
  }
  if(!r.tools.some((t:any)=>t.function.name==='subagent')) {res.end(wire({content:'done'}));return;}
  const results=r.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
  if(results.length===childCount+1) {
    assert.equal(results[childCount].subagent.settled.length,childCount);
    for(const child of results[childCount].subagent.settled)assert.equal(child.outcome.output,'done');
    res.end(wire({content:'done'}));return;
  }
  const args=results.length===childCount?{action:'wait',agent_ids:results.map((r:any)=>r.subagent.agent.agent_id),mode:'all',timeout_ms:5000}:{action:'spawn',request:{input:'small child',capabilities:{model_route:['secondary'],tools:[]}}};
  res.end(wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]},'tool_calls'));
});
try{
  const base=(await readFile(join(root,'docs/project/fixtures/c3-model-routes/three-providers.toml'),'utf8')).replace('max_model_calls=2','max_model_calls=10').replace('max_tool_calls=1','max_tool_calls=4');
  const stats:Record<string,unknown>={};
  for(const surface of ['cli','fresh_acp']) for(const enabled of [false,true]){
    const entry=join(cwd,'entry.toml');await writeFile(entry,base+`\n[options.children]\nenabled=${enabled}\n`);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const samples:number[]=[];
    for(let i=0;i<35;i++){
      const start=performance.now();
      const result=surface==='cli'?JSON.parse((await exec(binary,['run','small root',...args,'--json'],{env:cleanEnv(),timeout:10000})).stdout):await withPablo({binary,args,env:cleanEnv()},async cx=>{
        await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
        return taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'small root'}]}));
      });
      const elapsed=performance.now()-start;
      assert.equal(result.outcome.output,'done');
      assert.equal(result.accounting.model_calls,enabled?String(2*childCount+2):'1');assert.equal(result.accounting.tool_calls,enabled?String(childCount+1):'0');
      if(i>=5)samples.push(elapsed);
    }
    samples.sort((a,b)=>a-b);stats[`${surface}_${enabled?'child':'disabled'}_ms`]={min:samples[0],p50:samples[14],p95:samples[28],max:samples[29]};
  }
  const report={checkpoint:childCount===2?'C3.23':'C3.22',source_sha256:await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),platform:`${process.platform}/${process.arch}`,node:process.version,method:{samples:30,warmup:5,timing:'Complete CLI or fresh ACP process including startup, configured setup, task and joined shutdown; sequential surfaces/modes, local fixture, no trace.',comparison:`Same final binary: disabled root makes one model call; enabled root spawns/waits for ${childCount} children and makes ${2*childCount+2} aggregate model calls and ${childCount+1} subagent tool calls. Difference includes extra work, not isolated scheduler overhead.`,child_count:childCount,overlap_barrier:childCount===2},stats};
  await writeFile(join(root,`.pablo/measurements/c3.${childCount===2?'23':'22'}-children.json`),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
