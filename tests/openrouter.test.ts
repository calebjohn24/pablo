import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { once } from 'node:events';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
const exec=promisify(execFile);
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const model='z-ai/glm-5.3-flash';
const privateKey='private-router-sentinel';
const frame=(delta:object={},finish:string|null=null,usage?:object)=>'data: '+JSON.stringify({choices:[{index:0,delta,finish_reason:finish}],...(usage===undefined?{}:{usage})})+'\r\n\r\n';
const done='data: [DONE]\n\n';
const end=(usage?:object)=>frame({},'stop')+(usage===undefined?'':frame({role:'assistant',content:''},'stop',usage))+done;
const accounting={prompt_tokens:17,completion_tokens:3,completion_tokens_details:{reasoning_tokens:2},prompt_tokens_details:{cached_tokens:8,cache_write_tokens:4},cost:0.0000025,cost_details:{upstream_inference_cost:999}};
const unknown={input_tokens:null,output_tokens:null,cache_read_input_tokens:null,cache_write_input_tokens:null};
const preset=(otel:string)=>`schema_version=1
[credentials.gateway]
consumer="provider.openrouter"
sources=[{kind="environment",name="OPENROUTER_API_KEY"}]
[options.model]
reasoning="low"
provider="openrouter"
[options.shell]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
[options.otel]
exporter="otlp"
endpoint="${otel}"
`;
async function records(path:string){return (await readFile(path,'utf8')).trim().split('\n').map(s=>JSON.parse(s));}
async function waitUntil(check:()=>boolean,ms=3000){const start=performance.now();while(!check()){assert(performance.now()-start<ms,'condition timed out');await delay(10);}}

// Byte boundaries split UTF-8, CRLF, JSON tokens and argument fragments independently.
async function fragmented(res:import('node:http').ServerResponse,text:string){
 res.writeHead(200,{'content-type':'text/event-stream'});const bytes=Buffer.from(text);
 for(let i=0;i<bytes.length;i+=7){if(res.destroyed)return;if(!res.write(bytes.subarray(i,i+7)))await once(res,'drain');}
 res.end();
}

test('P02 CLI and reused ACP perform real OpenRouter file reads with stable prefixes, accounting and private telemetry',{timeout:25000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p02-')));const marker='violet-🟣-evidence';
 const requests:any[]=[];const connections=new Set();const exports:Buffer[]=[];let stdout='';let stderr='';
 const collector=await server(async(req,res)=>{exports.push(await body(req));res.end();});
 const gateway=await server(async(req,res)=>{
  assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');assert.equal(req.headers.traceparent,undefined);
  connections.add(req.socket);const request=JSON.parse((await body(req)).toString());requests.push(request);
  assert.equal(request.model,model);assert.deepEqual(request.reasoning,{effort:"low"});assert.equal(request.stream,true);assert.equal(request.stream_options,undefined);
  assert.equal(request.parallel_tool_calls,false);assert.equal(request.max_tokens,65536);
  if(request.messages.at(-1).role==='tool'){
   assert.equal(request.messages.at(-1).tool_call_id,'call_read');
   assert.equal(JSON.parse(request.messages.at(-1).content).filesystem.text,marker);
   const assistant=request.messages.at(-2);assert.equal(assistant.tool_calls[0].id,'call_read');
   assert.deepEqual(JSON.parse(assistant.tool_calls[0].function.arguments),{path:'evidence-🟣.txt'});
   await fragmented(res,frame({reasoning:'private-reasoning-sentinel',content:'Evidence: '})+frame({content:marker})+end(accounting));
  }else{
   let text=': OPENROUTER PROCESSING\r\n\r\n'+frame({role:'assistant',content:null});
   text+=frame({tool_calls:[{index:0,id:'call_read',type:'function',function:{name:'fs_',arguments:''}}]});
   text+=frame({tool_calls:[{index:0,id:null,type:null,function:{name:'read',arguments:null}}]});
   const args=JSON.stringify({path:'evidence-🟣.txt'});
   for(const part of [...args])text+=frame({tool_calls:[{index:0,id:null,type:null,function:{name:null,arguments:part}}]});
   text+=frame({},'tool_calls')+': keepalive\n\n'+frame({role:'assistant',content:''},'tool_calls',accounting)+done;
   await fragmented(res,text);
  }
 });
 try{
  await writeFile(join(cwd,'evidence-🟣.txt'),marker);await writeFile(join(cwd,'entry.toml'),preset(collector.url+'/v1/traces'));
  const env={...cleanEnv(),OPENROUTER_API_KEY:privateKey,AI_GATEWAY_API_KEY:'private-vercel-sentinel',PABLO_FIXTURE_ENDPOINT:gateway.url+'/v1/chat/completions',OTEL_TRACES_EXPORTER:'otlp',OTEL_EXPORTER_OTLP_ENDPOINT:collector.url};
  const variants=[['--reasoning-effort','low','--provider','openrouter','--no-shell','--trace',join(cwd,'legacy-{session_id}.jsonl')],['--config',join(cwd,'entry.toml'),'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions']];
  const tasks:any[]=[];
  for(const args of variants){
   const cli=await exec(binary,['run','Read evidence',...args,'--workspace',cwd,'--json'],{env,timeout:5000});stdout+=cli.stdout;stderr+=cli.stderr;tasks.push(JSON.parse(cli.stdout));
   const before=connections.size;
   await withPablo({binary,args,env,onSpawn:child=>{child.stdout.on('data',s=>{stdout+=s;});},onDiagnostic:s=>{stderr+=s;}},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});
    for(let i=0;i<2;i++){
     const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
     tasks.push(taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read evidence'}]})));
    }
   });
   assert.equal(connections.size-before,1,'two tasks reuse one provider HTTP connection');
  }
  assert.equal(tasks.length,6);assert.equal(requests.length,12);
  for(const task of tasks){
   assert.equal(task.outcome.status,'completed');assert.equal(task.outcome.output,'Evidence: '+marker);
   assert.deepEqual(task.accounting,{model_calls:'2',tool_calls:'1',usage:{input_tokens:'34',output_tokens:'6',cache_read_input_tokens:'16',cache_write_input_tokens:'8'},cost_microusd:'6',charged_tokens:null,charged_cost_microusd:null});
  }
  for(let i=0;i<requests.length;i++){
   assert.equal(JSON.stringify(requests[i].tools),JSON.stringify(requests[0].tools));
   assert.equal(JSON.stringify(requests[i].messages.slice(0,2)),JSON.stringify(requests[0].messages.slice(0,2)));
  }
  const traces=(await readdir(cwd)).filter(n=>n.endsWith('.jsonl'));assert.equal(traces.length,6);let native='';
  for(const name of traces){const path=join(cwd,name);native+=await readFile(path,'utf8');const events=await records(path);
   assert.equal(events.filter(e=>e.type==='run.finished').length,1);assert.equal(events.filter(e=>e.type==='model.finished').length,2);
   assert(events.filter(e=>e.type==='model.started').every(e=>e.provider==='openrouter'&&e.model===model));
   for(const e of events.filter(e=>e.type==='model.finished')) {
    const d=e.diagnostics;assert.equal(d.requested_reasoning,'low');assert.equal(d.reasoning_tokens,2);assert.equal(d.reported_reasoning_effort,null);assert.equal(d.http_version,'HTTP/1.1');
    const phases=[d.preparation_us,d.dispatch_us,d.headers_us,d.first_data_us,d.first_text_us??d.first_tool_delta_us,d.terminal_us,d.complete_us];
    assert(phases.every(Number.isSafeInteger));assert.deepEqual(phases,phases.toSorted((a,b)=>a-b));
   }
   assert.deepEqual(events.at(-1).accounting,tasks[0].accounting);
  }
  assert(exports.length>=4);assert(exports.every(p=>p.includes(Buffer.from('openrouter'))));
  for(const secret of [privateKey,'private-vercel-sentinel','private-reasoning-sentinel','pablo-local-fixture']){
   assert(!(stdout+stderr+native+JSON.stringify(requests)).includes(secret));
   assert(exports.every(p=>!p.includes(Buffer.from(secret))));
  }
  assert(!native.includes(marker));assert(exports.every(p=>!p.includes(Buffer.from(marker))));
 }finally{await gateway.close();await collector.close();await rm(cwd,{recursive:true,force:true});}
});

test('P02 OpenRouter absent accounting stays unknown, zero stays zero, and exact large decimal cost survives JSON',{timeout:15000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p02-usage-')));let response='';
 const gateway=await server(async(req,res)=>{await body(req);res.writeHead(200,{'content-type':'text/event-stream'});res.end(response);});
 try{
  for(const [usage,expectedCost,expectedUsage] of [
   [undefined,null,unknown],
   [{cost:null},null,unknown],
   [{prompt_tokens:0,completion_tokens:0,prompt_tokens_details:{cached_tokens:0,cache_write_tokens:0},cost:0},'0',{input_tokens:'0',output_tokens:'0',cache_read_input_tokens:'0',cache_write_input_tokens:'0'}],
   [{cost_details:{upstream_inference_cost:12}},null,unknown],
  ] as const){
   response=frame({content:'ok'})+end(usage);
   const task=JSON.parse((await exec(binary,['run','task','--provider','openrouter','--no-shell','--workspace',cwd,'--json'],{env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url},timeout:5000})).stdout);
   assert.equal(task.outcome.status,'completed');assert.equal(task.accounting.cost_microusd,expectedCost);assert.deepEqual(task.accounting.usage,expectedUsage);
  }
  response=frame({content:'ok'})+frame({},'stop')+'data: {"choices":[],"usage":{"cost":18446744073709.551615,"prompt_tokens":9007199254740993}}\n\n'+done;
  const task=JSON.parse((await exec(binary,['run','task','--provider','openrouter','--no-shell','--workspace',cwd,'--json'],{env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url},timeout:5000})).stdout);
  assert.equal(task.accounting.cost_microusd,'18446744073709551615');assert.equal(task.accounting.usage.input_tokens,'9007199254740993');
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('P02 OpenRouter errors, malformed accounting and bounded frames fail once without redirects or private diagnostics',{timeout:30000},async t=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p02-errors-')));let mode='';let requests=0;let redirects=0;
 const rejected=await server((_req,res)=>{redirects++;res.end();});
 const gateway=await server(async(req,res)=>{
  await body(req);requests++;
  if(mode==='reject'||mode==='throttle'){res.writeHead(mode==='reject'?401:429,{'content-type':'application/json','retry-after':'0'});res.end(JSON.stringify({error:{message:privateKey}}));return;}
  if(mode==='redirect'){res.writeHead(307,{location:rejected.url});res.end(privateKey);return;}
  if(mode==='content-type'){res.writeHead(200,{'content-type':'application/json'});res.end(privateKey);return;}
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(mode==='disconnect'){res.write(frame({content:'partial'}));await delay(15);res.destroy();return;}
  const bad:Record<string,string>={
   'midstream':frame({content:'partial'})+'data: '+JSON.stringify({error:{message:privateKey},choices:[{index:0,delta:{content:''},finish_reason:'error'}]})+'\n\n',
   'invalid-json':'data: '+privateKey+'\n\n',
   'invalid-utf8':'',
   'missing-done':frame({content:'partial'})+frame({},'stop'),
   'duplicate-finish':frame({},'stop')+end(),
   'duplicate-usage':frame({},'stop')+frame({},'stop',accounting)+frame({},'stop',accounting)+done,
   'post-finish-content':frame({},'stop')+frame({content:privateKey},'stop',accounting)+done,
   'mismatched-finish':frame({},'stop')+frame({},'length',accounting)+done,
   'negative-usage':end({prompt_tokens:-1}),
   'invalid-details':end({prompt_tokens_details:12}),
   'duplicate-counter':frame({},'stop')+'data: {"choices":[],"usage":{"prompt_tokens":1,"prompt_tokens":2}}\n\n'+done,
   'negative-cost':end({cost:-0.1}),
   'overflow-cost':end({cost:1e30}),
   'duplicate-cost':frame({},'stop')+'data: {"choices":[],"usage":{"cost":1,"cost":2}}\n\n'+done,
   'invalid-index':frame({tool_calls:[{index:128,id:'bad',function:{name:'fs_read',arguments:'{}'}}]},'tool_calls')+done,
   'unsupported-tool':frame({tool_calls:[{index:0,id:'bad',function:{name:'private_unknown',arguments:'{}'}}]},'tool_calls')+done,
   'malformed-arguments':frame({tool_calls:[{index:0,id:'bad',function:{name:'fs_read',arguments:'{'}}]},'tool_calls')+done,
  };
  if(mode==='oversized-frame'){res.end('data: '+ 'x'.repeat(32*1024*1024)+'\n\n');return;}
  if(mode==='invalid-utf8'){res.end(Buffer.from([100,97,116,97,58,32,255,10,10]));return;}
  res.end(bad[mode]);
 });
 try{
  for(mode of ['reject','throttle','redirect','content-type','disconnect','midstream','invalid-json','invalid-utf8','missing-done','duplicate-finish','duplicate-usage','post-finish-content','mismatched-finish','negative-usage','invalid-details','duplicate-counter','negative-cost','overflow-cost','duplicate-cost','invalid-index','unsupported-tool','malformed-arguments','oversized-frame'])await t.test(mode,async()=>{
   const before=requests;const args=['--provider','openrouter','--no-shell','--trace',join(cwd,mode+'-{session_id}.jsonl')];
   const env={...cleanEnv(),OPENROUTER_API_KEY:privateKey,PABLO_FIXTURE_ENDPOINT:gateway.url};
   let raw='';let task:any;
   await assert.rejects(exec(binary,['run','task',...args,'--workspace',cwd,'--json'],{env,timeout:7000}),error=>{const e=error as {stdout:string,stderr:string,code:number};assert.equal(e.code,1);raw+=e.stdout+e.stderr;task=JSON.parse(e.stdout);return true;});
   const code=['reject','throttle','redirect','midstream'].includes(mode)?'provider_rejected':mode==='disconnect'?'provider_transport':mode==='malformed-arguments'?'invalid_tool_arguments':'malformed_stream';
   assert.deepEqual(task.outcome,{status:'failed',code,delivery:'response_received'});
   assert.equal(task.accounting.model_calls,'1');assert.equal(task.accounting.tool_calls,'0');
   await withPablo({binary,args,env,onSpawn:child=>{child.stdout.on('data',s=>{raw+=s;});},onDiagnostic:s=>{raw+=s;}},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});
    const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
    await assert.rejects(cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'task'}]}),(error:any)=>{assert.equal(error.code,-32603);const acp=error.data['pablo/v2'].task;assert.deepEqual(acp.outcome,task.outcome);assert.deepEqual(acp.accounting,task.accounting);return true;});
   });
   assert.equal(requests-before,2);assert.equal(redirects,0);
   const paths=(await readdir(cwd)).filter(n=>n.startsWith(mode+'-')&&n.endsWith('.jsonl'));assert.equal(paths.length,2);
   for(const name of paths){const events=await records(join(cwd,name));assert.equal(events.filter(e=>e.type==='run.finished').length,1);assert.equal(events.filter(e=>e.type==='tool.started').length,0);raw+=await readFile(join(cwd,name),'utf8');}
   assert(!raw.includes(privateKey));
  });
 }finally{await gateway.close();await rejected.close();await rm(cwd,{recursive:true,force:true});}
});

test('P02 cancellation drops the OpenRouter stream, preserves completed-call usage and permits a clean next ACP task',{timeout:20000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p02-cancel-')));let calls=0;let closed=0;let stage='cancel';
 const gateway=await server(async(req,res)=>{
  const request=JSON.parse((await body(req)).toString());calls++;res.writeHead(200,{'content-type':'text/event-stream'});
  if(stage==='clean'){res.end(frame({content:'clean'})+end(accounting));return;}
  if(request.messages.at(-1).role==='tool'){
   assert.equal(JSON.parse(request.messages.at(-1).content).filesystem.text,'marker');
   res.on('close',()=>{closed++;});res.write(frame({content:'cancel-now'}));return;
  }
  res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:'fs_read',arguments:'{"path":"marker.txt"}'}}]},'tool_calls')+frame({},'tool_calls',accounting)+done);
 });
 try{
  await writeFile(join(cwd,'marker.txt'),'marker');const env={...cleanEnv(),OPENROUTER_API_KEY:privateKey,PABLO_FIXTURE_ENDPOINT:gateway.url};
  const args=['--provider','openrouter','--no-shell','--trace',join(cwd,'{session_id}.jsonl')];
  const child=spawn(binary,['run','Read marker',...args,'--workspace',cwd,'--json'],{env});let stdout='';let stderr='';
  child.stdout.on('data',s=>{stdout+=s;});child.stderr.on('data',s=>{stderr+=s;});const exited=once(child,'exit');
  await waitUntil(()=>calls===2);child.kill('SIGINT');const [code,signal]=await exited;assert.equal(code,130);assert.equal(signal,null);
  const cli=JSON.parse(stdout);assert.equal(cli.outcome.status,'cancelled');await waitUntil(()=>closed===1);
  assert.equal(cli.accounting.model_calls,'2');assert.equal(cli.accounting.tool_calls,'1');assert.equal(cli.accounting.cost_microusd,null);assert.equal(cli.accounting.usage.input_tokens,null);
  let cancelled=false;
  await withPablo({binary,args,env,onUpdate:async({sessionId,update},cx)=>{
   if(!cancelled&&update.sessionUpdate==='agent_message_chunk'){cancelled=true;await cx.notify('session/cancel',{sessionId});}
  }},async cx=>{
   await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});
   const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
   const acp=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read marker'}]}));
   assert.equal(acp.outcome.status,'cancelled');assert.deepEqual(acp.accounting,cli.accounting);await waitUntil(()=>closed===2);
   stage='clean';const next=await cx.request('session/new',{cwd,mcpServers:[]});
   const task=taskOf(await cx.request('session/prompt',{sessionId:next.sessionId,prompt:[{type:'text',text:'Next task'}]}));assert.equal(task.outcome.status,'completed');assert.equal(task.outcome.output,'clean');assert.equal(task.accounting.model_calls,'1');
  });
  const traces=(await readdir(cwd)).filter(n=>n.endsWith('.jsonl'));assert.equal(traces.length,3);
  for(const path of traces){const events=await records(join(cwd,path));assert.equal(events.filter(e=>e.type==='run.finished').length,1);if(events.at(-1).outcome.status==='cancelled'){
   const finished=events.filter(e=>e.type==='model.finished');assert.equal(finished.length,2);
   // The settled first call is visible even though the aggregate includes an unknown second call.
   assert(events.some(e=>e.type==='model.finished'&&e.usage?.input_tokens===17));
  }}
  assert(!(stdout+stderr).includes(privateKey));
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('P02 private OpenRouter key failures are isolated from Vercel keys and reject before trace or provider effects',{timeout:20000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p02-key-')));const file=join(cwd,'private.env');let requests=0;
 const forbidden=await server((_req,res)=>{requests++;res.end();});
 try{
  const trace=join(cwd,'never-created.jsonl');
  for(const content of ['AI_GATEWAY_API_KEY=private-vercel-sentinel\n','OPENROUTER_API_KEY="private invalid router"\n','OPENROUTER_API_KEY=one\nOPENROUTER_API_KEY=two\n']){
   await writeFile(file,content);
   const env={...cleanEnv(),AI_GATEWAY_API_KEY:'private-vercel-primary',VERCEL_AI_GATEWAY:'private-vercel-alias',HTTPS_PROXY:forbidden.url};
   const args=['--provider','openrouter','--env-file',file,'--trace',trace];
   await assert.rejects(exec(binary,['run','task',...args,'--workspace',cwd,'--json'],{env,timeout:5000}),(e:any)=>{assert.equal(e.code,2);assert(!(e.stdout+e.stderr).includes('private'));assert((e.stdout+e.stderr).includes('OPENROUTER_API_KEY'));return true;});
   await withPablo({binary,args,env},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true}}});
    const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
    await assert.rejects(cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'task'}]}),(e:any)=>{assert.equal(e.code,-32603);assert.equal(e.data,'runtime setup or execution failed');return true;});
   });
   await assert.rejects(readFile(trace),{code:'ENOENT'});
  }
  // Configured credentials use the same consumer scope but an explicit literal file source.
  const entry=join(cwd,'entry.toml');await writeFile(entry,`schema_version=1
[credentials.gateway]
consumer="provider.openrouter"
sources=[{kind="file",path={base="config",path="private.env"},encoding="dotenv",key="OPENROUTER_API_KEY"}]
[options.model]
provider="openrouter"
[options.trace]
path={base="workspace",path="never-created.jsonl"}
`);
  for(const command of [['run','task'],['acp','--stdio']])await assert.rejects(exec(binary,[...command,'--config',entry,'--bind',`workspace=${cwd}`],{env:cleanEnv(),timeout:5000}),(e:any)=>{assert.equal(e.code,2);assert.match(e.stderr,/config_credential_invalid/);assert(!e.stderr.includes('private'));return true;});
  await assert.rejects(readFile(trace),{code:'ENOENT'});assert.equal(requests,0);
 }finally{await forbidden.close();await rm(cwd,{recursive:true,force:true});}
});

test('P02 OpenRouter credentials never enter a real shell environment or captured task surfaces',{timeout:10000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p02-shell-')));let calls=0;let observed='';
 const gateway=await server(async(req,res)=>{
  assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');const request=JSON.parse((await body(req)).toString());calls++;res.writeHead(200,{'content-type':'text/event-stream'});
  if(calls===1){res.end(frame({tool_calls:[{index:0,id:'env',type:'function',function:{name:'shell_run',arguments:'{"command":"env","cwd":"."}'}}]},'tool_calls')+frame({},'tool_calls',accounting)+done);return;}
  observed=JSON.parse(request.messages.at(-1).content).shell.stdout;
  for(const secret of [privateKey,'OPENROUTER_API_KEY','AI_GATEWAY_API_KEY','private-vercel'])assert(!observed.includes(secret));
  res.end(frame({content:'clean environment'})+end(accounting));
 });
 try{
  const path=join(cwd,'trace.jsonl');const result=await exec(binary,['run','Inspect environment','--provider','openrouter','--workspace',cwd,'--trace',path,'--capture-content','--json'],{env:{...cleanEnv(),OPENROUTER_API_KEY:privateKey,AI_GATEWAY_API_KEY:'private-vercel',PABLO_FIXTURE_ENDPOINT:gateway.url},timeout:5000});
  assert.equal(JSON.parse(result.stdout).outcome.output,'clean environment');assert.equal(calls,2);assert(observed.includes('PATH='));
  const surfaces=result.stdout+result.stderr+await readFile(path,'utf8');assert(!surfaces.includes(privateKey));assert(!surfaces.includes('private-vercel'));
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
