import assert from 'node:assert/strict';
import {test} from 'node:test';
import {mkdtemp,readFile,realpath,rm,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
import {withPablo,taskOf} from '../examples/acp-client.ts';
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const base=(await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml',import.meta.url),'utf8')).replace('max_model_calls=2','max_model_calls=20').replace('max_tool_calls=1','max_tool_calls=10');
const wire=(delta:object,finish='stop')=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\ndata: [DONE]\n\n`;
for(const mode of ['stop','fail','cancel'] as const)test(mode==='cancel'?'A03 root cancellation joins two overlapping children and drains queued work':`A03 overlapping children preserve a runnable sibling after ${mode} and admit queued work`,{timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-a03-')));
  const seen=new Set<string>();
  let overlap!:()=>void;const both=new Promise<void>(resolve=>{overlap=resolve;});
  let failA!:()=>void,finishB!:()=>void;
  let cancelRoot!:()=>Promise<void>;
  const gateway=await server(async(req,res)=>{
    const r=JSON.parse((await body(req)).toString());
    if(r.model==='zai/glm-5.3-flash'){
      const text=JSON.stringify(r.messages);
      const id=['child-A','child-B','child-C'].find(id=>text.includes(id))!;
      assert(id);assert(!seen.has(id),'no replay');seen.add(id);
      if(id==='child-A'&&mode==='fail')failA=()=>{res.writeHead(503);res.end('synthetic child failure');};
      else{
        res.writeHead(200,{'content-type':'text/event-stream'});
        if(id==='child-C')res.end(wire({content:'C done'}));
        else{
          res.write(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'working'},finish_reason:null}]})}\n\n`);
          if(id==='child-B')finishB=()=>res.end(wire({content:' done'}));
        }
      }
      if(seen.has('child-A')&&seen.has('child-B'))overlap();
      return;
    }
    assert.equal(r.model,'z-ai/glm-5.3-flash');
    const results=r.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
    const ids=results.slice(0,3).map((r:any)=>r.subagent?.agent?.agent_id);
    let args:object;
    if(results.length<3)args={action:'spawn',request:{input:['child-A','child-B','child-C'][results.length],capabilities:{model_route:['secondary'],tools:[]}}};
    else if(results.length===3){
      // Both calls reached the provider without either completing; C cannot
      // start until one slot is released. This is a barrier, not a timing guess.
      await both;assert(!seen.has('child-C'));
      if(mode==='cancel'){
        // Notification write completion is not server-side cancellation receipt.
        // Keep this model operation pending so a final answer cannot win that race.
        await cancelRoot();return;
      }
      if(mode==='fail'){failA();args={action:'wait',agent_ids:[ids[0]],mode:'all',timeout_ms:5000};}
      else args={action:'stop',agent_id:ids[0]};
    }else if(results.length===4){
      const a=mode==='stop'?results[3].subagent.snapshot:results[3].subagent.settled[0];
      assert.equal(a.outcome.status,mode==='stop'?'cancelled':'failed');
      args={action:'wait',agent_ids:[ids[1],ids[2]],mode:'any',timeout_ms:5000};
    }else if(results.length===5){
      assert.equal(results[4].subagent.settled.length,1);
      assert.equal(results[4].subagent.settled[0].agent.agent_id,ids[2]);
      assert.equal(results[4].subagent.remaining[0].agent_id,ids[1]);
      finishB();args={action:'wait',agent_ids:ids,mode:'all',timeout_ms:5000};
    }else{
      assert.deepEqual(results[5].subagent.remaining,[]);
      assert.equal(results[5].subagent.settled[1].outcome.output,'working done');
      assert.equal(results[5].subagent.settled[2].outcome.output,'C done');
      res.writeHead(200,{'content-type':'text/event-stream'});res.end(wire({content:'all joined'}));return;
    }
    res.writeHead(200,{'content-type':'text/event-stream'});
    res.end(wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]},'tool_calls'));
  });
  try{
    const entry=join(cwd,'entry.toml');await writeFile(entry,base+'\n[options.children]\nenabled=true\n[options.trace]\npath={base="workspace",path="trace-{session_id}.jsonl"}\n');
    await withPablo({binary,args:['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url],env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      cancelRoot=()=>cx.notify('session/cancel',{sessionId});
      const task=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'exercise siblings'}]}));
      if(mode==='cancel'){
        assert.equal(task.outcome.status,'cancelled');assert(!seen.has('child-C'));
        assert.equal(task.accounting.model_calls,'6');assert.equal(task.accounting.tool_calls,'3');
      }else{
        assert.equal(task.outcome.status,'completed');assert(task.outcome.status==='completed');assert.equal(task.outcome.output,'all joined');
        assert.equal(task.accounting.model_calls,'10');assert.equal(task.accounting.tool_calls,'6');
      }
      const events=(await readFile(join(cwd,`trace-${sessionId}.jsonl`),'utf8')).trim().split('\n').map(s=>JSON.parse(s));
      assert.deepEqual(events.map(e=>e.root_seq),events.map((_,i)=>i+1));
      assert.equal(events.filter(e=>e.type==='run.finished'&&e.agent.depth===1).length,mode==='cancel'?2:3);
      if(mode==='cancel')assert(events.filter(e=>e.type==='run.finished'&&e.agent.depth===1).every(e=>e.outcome.status==='cancelled'));
      assert.equal(events.at(-1).agent.depth,0);assert.equal(events.at(-1).type,'run.finished');
    });
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
