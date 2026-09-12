import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createHash} from 'node:crypto';
import {mkdtemp,readFile,realpath,rm,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
import {withPablo,taskOf} from '../examples/acp-client.ts';
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const base=(await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml',import.meta.url),'utf8')).replace('max_model_calls=2','max_model_calls=20').replace('max_tool_calls=1','max_tool_calls=10');
const wire=(delta:object,finish='stop')=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\ndata: [DONE]\n\n`;
for(const surface of ['cli','acp'] as const)test(`A04 ${surface}: explicit fan-in consumes inline data and a verified artifact with source provenance`,{timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-a04-')));
  const artifact='{"delta":1}',reference={path:'artifact.json',revision:createHash('sha256').update(artifact).digest('hex')};
  await writeFile(join(cwd,reference.path),artifact);
  const waiting:Array<()=>void>=[];
  let sources:any[]=[];
  const gateway=await server(async(req,res)=>{
    const r=JSON.parse((await body(req)).toString());
    res.writeHead(200,{'content-type':'text/event-stream'});
    const finish=(value:unknown)=>res.end(wire({content:JSON.stringify(value)}));
    const tool=(name:string,args:object)=>res.end(wire({tool_calls:[{index:0,id:`call-${r.messages.length}`,type:'function',function:{name,arguments:JSON.stringify(args)}}]},'tool_calls'));
    const results=r.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
    if(r.model==='zai/glm-5.3-flash'){
      const text=JSON.stringify(r.messages);
      if(text.includes('PRIVATE_A')||text.includes('PRIVATE_B')){
        waiting.push(()=>finish(text.includes('PRIVATE_A')?{answer:41}:reference));
        if(waiting.length===2)for(const complete of waiting.splice(0))complete();
        return;
      }
      assert(!text.includes('ROOT_PRIVATE'));
      const input=JSON.parse(r.messages.find((m:any)=>m.role==='user').content);
      assert.equal(input.task,'combine selected outputs');
      const handoffs=input.context.map((s:string)=>JSON.parse(s).handoff);
      assert.equal(handoffs.length,2);
      for(let i=0;i<2;i++){
        assert.equal(handoffs[i].selection.source_agent_id,sources[i].agent.agent_id);
        assert.equal(handoffs[i].selection.result_id,sources[i].result_id);
        assert.deepEqual(handoffs[i].validation,sources[i].validation);
        assert.deepEqual(handoffs[i].accounting,sources[i].accounting);
        assert.deepEqual(handoffs[i].trace,sources[i].trace);
      }
      assert.deepEqual(handoffs[1].value,reference);
      if(results.length===0){tool('fs_read',{path:handoffs[1].value.path});return;}
      assert.equal(results.length,1);
      assert.equal(results[0].filesystem.revision,reference.revision);
      finish({answer:handoffs[0].value.answer+JSON.parse(results[0].filesystem.text).delta});return;
    }
    assert.equal(r.model,'z-ai/glm-5.3-flash');
    if(results.length<2){
      tool('subagent',{action:'spawn',request:{input:results.length===0?'PRIVATE_A':'PRIVATE_B',capabilities:{model_route:['secondary'],tools:[]},output_schema:{type:'object'}}});return;
    }
    if(results.length===2){tool('subagent',{action:'wait',agent_ids:results.map((r:any)=>r.subagent.agent.agent_id),mode:'all',timeout_ms:5000});return;}
    if(results.length===3){
      sources=results[2].subagent.settled;
      assert.equal(sources.length,2);assert.deepEqual(results[2].subagent.remaining,[]);
      for(const source of sources){assert.equal(source.outcome.status,'completed');assert.equal(source.validation.status,'valid');assert(source.result_id);assert(source.trace);}
      tool('subagent',{action:'spawn',request:{input:'combine selected outputs',capabilities:{model_route:['secondary'],tools:['fs.read']},output_schema:{type:'object',properties:{answer:{const:42}},required:['answer'],additionalProperties:false},handoffs:sources.map((s,i)=>({source_agent_id:s.agent.agent_id,result_id:s.result_id,kind:i===0?'inline':'artifact'}))}});return;
    }
    if(results.length===4){tool('subagent',{action:'wait',agent_ids:[results[3].subagent.agent.agent_id],mode:'all',timeout_ms:5000});return;}
    assert.equal(results.length,5);
    const combined=results[4].subagent.settled[0];
    assert.equal(combined.validation.status,'valid',JSON.stringify(combined));assert.deepEqual(JSON.parse(combined.outcome.output),{answer:42});
    assert.deepEqual(combined.handoffs.map((s:any)=>s.result_id),sources.map(s=>s.result_id));
    finish({answer:42});
  });
  try{
    const entry=join(cwd,'entry.toml');await writeFile(entry,base+'\n[options.children]\nenabled=true\n[options.trace]\npath={base="workspace",path="trace.jsonl"}\n');
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const task=surface==='cli'?JSON.parse((await promisify(execFile)(binary,['run','ROOT_PRIVATE',...args,'--json'],{env:cleanEnv(),timeout:12000})).stdout):await withPablo({binary,args,env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      return taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'ROOT_PRIVATE'}]}));
    });
    assert.equal(task.outcome.status,'completed');assert.deepEqual(JSON.parse(task.outcome.output),{answer:42});
    assert.equal(task.accounting.model_calls,'10');assert.equal(task.accounting.tool_calls,'6');
    const events=(await readFile(join(cwd,'trace.jsonl'),'utf8')).trim().split('\n').map(s=>JSON.parse(s));
    assert.deepEqual(events.map(e=>e.root_seq),events.map((_,i)=>i+1));
    for(const source of sources){
      const terminal=events.find(e=>e.type==='run.finished'&&e.agent.agent_id===source.agent.agent_id);
      assert(terminal);assert.equal(terminal.trace_id,source.trace.trace_id);assert.equal(terminal.run_id,source.trace.run_id);assert.deepEqual(terminal.output_validation,source.validation);
    }
    assert.equal(events.filter(e=>e.type==='run.finished'&&e.agent.depth===1).length,3);
    assert.equal(events.at(-1).agent.depth,0);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
