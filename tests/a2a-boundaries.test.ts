import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile,spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,readFile,realpath,rm,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
import {withPablo,taskOf} from '../examples/acp-client.ts';
const root=fileURLToPath(new URL('../',import.meta.url)),python=join(root,'.pablo/a2a-fixture-venv/bin/python');
const exec=promisify(execFile),binary=join(root,'target/debug/pablo');
const scenarios=['before_assignment','accepted','rejected','late_completion','stream_loss','timeout','oversized','input_required','cancel_hang'];
for(const surface of ['cli','acp','cli_cancel','acp_cancel'])for(const scenario of (surface.endsWith('_cancel')?['before_assignment','accepted']:scenarios))test(`R03 ${surface}: ${scenario}`,{timeout:15000,skip:!existsSync(python)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-a2a-boundary-')));
  const peer=spawn(python,[join(root,'tests/fixtures/a2a/boundary_server.py'),scenario],{cwd,env:{},stdio:['ignore','pipe','pipe']});
  const exited=new Promise<void>((resolve,reject)=>{peer.once('error',reject);peer.once('close',()=>resolve());});
  let diagnostics='';peer.stderr.on('data',b=>{diagnostics+=b.toString();});
  const ready=new Promise<string>((resolve,reject)=>{
    let text='';const timer=setTimeout(()=>reject(new Error('SDK readiness timeout')),5000);
    peer.stdout.on('data',b=>{text+=b.toString();if(text.length>4096){clearTimeout(timer);reject(new Error('SDK readiness bound'));}else if(text.includes('\n')){clearTimeout(timer);resolve(JSON.parse(text.split('\n')[0]).url);}});
    exited.then(()=>{clearTimeout(timer);reject(new Error(diagnostics));},reject);
  });
  const rootCancel=surface.endsWith('_cancel');
  let cancelReady!:()=>void;const cancellable=new Promise<void>(resolve=>{cancelReady=resolve;});
  let gateway:Awaited<ReturnType<typeof server>>|undefined,snapshot:any;
  const calls=async()=>{try{return (await readFile(join(cwd,'calls.jsonl'),'utf8')).trim().split('\n').map(x=>JSON.parse(x));}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return [];throw error;}};
  try {
    const rpc=await ready;
    const stopping=['before_assignment','accepted','rejected','late_completion','cancel_hang'].includes(scenario);
    gateway=await server(async(req,res)=>{
      const request=JSON.parse((await body(req)).toString());
      const results=request.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content).subagent);
      assert(results.length<20,'bounded polling');let args:object|undefined;
      if(results.length===0)args={action:'spawn_remote',remote_request:{remote:'peer',parts:[{text:'PRIVATE_EXPLICIT_PART'}],accepted_output_modes:['text/plain'],stream:true,max_duration_ms:scenario==='timeout'?200:8000}};
      else {
        const id=results[0].agent.agent_id,last=results.at(-1);
        if(last.action==='stop')snapshot=last.snapshot;
        else if(last.action==='wait'&&last.settled.length)snapshot=last.settled[0];
        else if(!stopping)args={action:'wait',agent_ids:[id],mode:'all',timeout_ms:5000};
        else if(scenario==='before_assignment'){
          for(let i=0;i<200&&!(await calls()).some(c=>c.method==='SendStreamingMessage');i++)await delay(5);
          assert((await calls()).some(c=>c.method==='SendStreamingMessage'));args={action:'stop',agent_id:id};
        } else if(last.action==='inspect'&&last.snapshot.remote.remote.task_id)args={action:'stop',agent_id:id};
        else {await delay(10);args={action:'inspect',agent_id:id};}
      }
      if(rootCancel&&args&&(args as any).action==='stop'){cancelReady();args={action:'wait',agent_ids:[results[0].agent.agent_id],mode:'all',timeout_ms:5000};}
      const delta=args?{tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]}:{content:'boundary verified'};
      res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:args?'tool_calls':'stop'}]})}\n\ndata: [DONE]\n\n`);
    });
    const base=(await readFile(join(root,'docs/project/fixtures/c3-model-routes/three-providers.toml'),'utf8')).replace('max_model_calls=2','max_model_calls=30').replace('max_tool_calls=1','max_tool_calls=30');
    const entry=join(cwd,'entry.toml');await writeFile(entry,base+'\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=false\npath={base="workspace",path="trace-{session_id}.jsonl"}\n[options.a2a.remotes.peer]\ncard_url="https://agent.example.test/.well-known/agent-card.json"\nendpoint="https://agent.example.test/rpc"\n');
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--fixture-a2a-endpoint',`peer=${rpc}`];
    const started=performance.now();let task:any;
    if(surface.startsWith('cli')){
      const pending=exec(binary,['run','PRIVATE_ROOT_CONTEXT',...args,'--json'],{env:cleanEnv(),timeout:12000});
      if(rootCancel){await cancellable;pending.child.kill('SIGINT');}
      try {task=JSON.parse((await pending).stdout);}catch(error){if(!rootCancel||(error as any).code!==130)throw error;task=JSON.parse((error as any).stdout);}
    }
    else await withPablo({binary,args,env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});const pending=cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'PRIVATE_ROOT_CONTEXT'}]});if(rootCancel){await cancellable;await cx.notify('session/cancel',{sessionId});}task=taskOf(await pending);
    });
    assert(performance.now()-started<7000,'bounded task and joined cleanup');assert.equal(task.outcome.status,rootCancel?'cancelled':'completed');
    if(rootCancel){
      const history=await calls();assert.equal(history.filter(c=>c.method==='SendStreamingMessage').length,1);assert.equal(history.filter(c=>c.method==='CancelTask').length,scenario==='before_assignment'?0:1);
      const trace=await readFile(join(cwd,`trace-${task.session_id}.jsonl`),'utf8');assert(!trace.includes('PRIVATE_ROOT_CONTEXT'));assert(!trace.includes('PRIVATE_EXPLICIT_PART'));
      const events=trace.trim().split('\n').map(x=>JSON.parse(x));const terminal=events.filter(e=>e.agent.kind==='remote_a2a'&&e.type==='run.finished');assert.equal(terminal.length,1);assert.equal(terminal[0].outcome.status,'cancelled');assert.equal(events.at(-1).agent.kind,'root');return;
    }
    assert(snapshot);
    assert.equal(snapshot.outcome.status,stopping?'cancelled':scenario==='timeout'?'timed_out':scenario==='oversized'?'limit_exceeded':'failed');
    if(scenario==='oversized')assert.equal(snapshot.outcome.limit,'output_bytes');
    if(['stream_loss','input_required'].includes(scenario))assert.equal(snapshot.outcome.code,'remote_task');
    const receipt=snapshot.remote.cancellation;
    if(scenario==='before_assignment'){assert.equal(receipt,'unassigned');assert.equal(snapshot.remote.remote.task_id,null);}
    else if(scenario==='rejected')assert.equal(typeof receipt.rejected,'number');
    else if(scenario==='cancel_hang')assert.equal(receipt,'unconfirmed');
    else assert.deepEqual(receipt,{received:scenario==='late_completion'?'TASK_STATE_COMPLETED':'TASK_STATE_CANCELED'});
    if(scenario==='input_required')assert.equal(snapshot.remote.result.disposition,'input_required');
    else assert.equal(snapshot.remote.result,null);
    assert.equal(snapshot.accounting.model_calls,'0');assert.equal(snapshot.accounting.tool_calls,'0');
    const history=await calls();assert.equal(history.filter(c=>c.method==='GetAgentCard').length,1);assert.equal(history.filter(c=>c.method==='SendStreamingMessage').length,1);assert.equal(history.filter(c=>c.method==='CancelTask').length,scenario==='before_assignment'?0:1);
    const trace=await readFile(join(cwd,`trace-${task.session_id}.jsonl`),'utf8');assert(!trace.includes('PRIVATE_ROOT_CONTEXT'));assert(!trace.includes('PRIVATE_EXPLICIT_PART'));
    const events=trace.trim().split('\n').map(x=>JSON.parse(x));const remote=events.filter(e=>e.agent.kind==='remote_a2a');assert.equal(remote.filter(e=>e.type==='run.finished').length,1);assert.deepEqual(remote.at(-1).outcome,snapshot.outcome);
    assert.deepEqual(events.map(e=>e.root_seq),events.map((_,i)=>i+1));
  } finally {
    try {if(gateway)await gateway.close();} finally {if(peer.exitCode===null)peer.kill('SIGTERM');const timer=setTimeout(()=>peer.kill('SIGKILL'),3000);try{await exited;}finally{clearTimeout(timer);await rm(cwd,{recursive:true,force:true});}}
  }
});
