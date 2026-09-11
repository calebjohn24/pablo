import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,realpath,rm,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './telemetry.ts';
import {withPablo,taskOf} from '../../examples/acp-client.ts';
const root=fileURLToPath(new URL('../../',import.meta.url));
export async function a2aTelemetry(endpoint:string,mode:'negotiated'|'generic',binary=join(root,'target/debug/pablo')){
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-a2a-otel-')));
  const peer=spawn(join(root,'.pablo/a2a-fixture-venv/bin/python'),[join(root,'tests/fixtures/a2a/telemetry_server.py'),endpoint,mode],{cwd,env:{},stdio:['ignore','pipe','pipe']});
  const exited=new Promise<void>((resolve,reject)=>{peer.once('error',reject);peer.once('close',()=>resolve());});
  let diagnostics='';peer.stderr.on('data',b=>{diagnostics+=b.toString();});
  const ready=new Promise<string>((resolve,reject)=>{
    let text='';const timer=setTimeout(()=>reject(new Error('SDK readiness timeout')),5000);
    peer.stdout.on('data',b=>{text+=b.toString();if(text.length>4096){clearTimeout(timer);reject(new Error('SDK readiness bound'));}else if(text.includes('\n')){clearTimeout(timer);resolve(JSON.parse(text.split('\n')[0]).url);}});
    exited.then(()=>{clearTimeout(timer);reject(new Error(diagnostics));},reject);
  });
  let gateway:Awaited<ReturnType<typeof server>>|undefined,snapshot:any;
  try {
    const rpc=await ready;
    gateway=await server(async(req,res)=>{
      const request=JSON.parse((await body(req)).toString());
      const results=request.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content).subagent);
      let args:object|undefined;
      if(!results.length)args={action:'spawn_remote',remote_request:{remote:'peer',parts:[{text:'PRIVATE_REMOTE_PART'}],accepted_output_modes:['text/plain'],stream:true}};
      else if(results.length===1)args={action:'wait',agent_ids:[results[0].agent.agent_id],mode:'all',timeout_ms:5000};
      else {assert.equal(results.length,2);assert.deepEqual(results[1].remaining,[]);snapshot=results[1].settled[0];assert.equal(snapshot.outcome.status,'completed');}
      const delta=args?{tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]}:{content:'verified'};
      res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:args?'tool_calls':'stop'}]})}\n\ndata: [DONE]\n\n`);
    });
    const base=(await readFile(join(root,'docs/project/fixtures/c3-model-routes/three-providers.toml'),'utf8')).replace('max_model_calls=2','max_model_calls=10').replace('max_tool_calls=1','max_tool_calls=10');
    const entry=join(cwd,'entry.toml');await writeFile(entry,base+`\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=false\npath={base="workspace",path="trace-{session_id}.jsonl"}\n[options.otel]\nexporter="otlp"\nendpoint="${endpoint}"\nsampler="always_on"\nschedule_delay_ms=60000\n[options.a2a.remotes.peer]\ncard_url="https://agent.example.test/.well-known/agent-card.json"\nendpoint="https://agent.example.test/rpc"\ntrace_context=true\n`);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--fixture-a2a-endpoint',`peer=${rpc}`];
    let task:any;
    await withPablo({binary,args,env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});task=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'PRIVATE_ROOT_CONTEXT'}]}));
    });
    assert.equal(task.outcome.status,'completed');assert(snapshot);assert.equal(snapshot.remote.remote_reported_usage.totalTokens,'20');
    const trace=await readFile(join(cwd,`trace-${task.session_id}.jsonl`),'utf8');const events=trace.trim().split('\n').map(x=>JSON.parse(x));
    const calls=(await readFile(join(cwd,'calls.jsonl'),'utf8')).trim().split('\n').map(x=>JSON.parse(x));assert.deepEqual(calls.map(c=>c.method),['GetAgentCard','SendStreamingMessage']);
    return {task,snapshot,events,diagnostics};
  } finally {
    try {if(gateway)await gateway.close();} finally {if(peer.exitCode===null)peer.kill('SIGTERM');const timer=setTimeout(()=>peer.kill('SIGKILL'),3000);try{await exited;}finally{clearTimeout(timer);await rm(cwd,{recursive:true,force:true});}}
  }
}
