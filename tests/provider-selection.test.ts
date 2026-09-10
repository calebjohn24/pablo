import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';
const exec=promisify(execFile);
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const preset=`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="UNREAD_SYNTHETIC_KEY"}]
[options.shell]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
`;
const frame=(delta:object,finish:string)=>'data: '+JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})+'\n\ndata: [DONE]\n\n';

test('P01 default and explicit Vercel selection share real CLI/ACP round trips and selected GLM default', {timeout:20000}, async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p01-')));
 const seen:any[]=[];
 const gateway=await server(async(req,res)=>{
  assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');
  const request=JSON.parse((await body(req)).toString());seen.push(request);
  assert.equal(request.model,'zai/glm-5.3-flash');
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(request.messages.at(-1).role==='tool') {
   const result=JSON.parse(request.messages.at(-1).content);assert.equal(result.filesystem.text,'selected-evidence');
   res.end(frame({content:result.filesystem.text},'stop'));
  } else res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:'fs_read',arguments:JSON.stringify({path:'evidence.txt'})}}]},'tool_calls'));
 });
 try {
  await writeFile(join(cwd,'evidence.txt'),'selected-evidence');
  await writeFile(join(cwd,'default.toml'),preset);
  await writeFile(join(cwd,'explicit.toml'),preset+'[options.model]\nprovider="vercel"\n');
  const env={...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url+'/v1/chat/completions',AI_GATEWAY_API_KEY:'private-vercel-primary',VERCEL_AI_GATEWAY:'private-vercel-alias',OPENROUTER_API_KEY:'private-router',UNREAD_SYNTHETIC_KEY:'private-configured'};
  const configured=(file:string)=>['--config',join(cwd,file),'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
  const variants=[['--no-shell'],['--no-shell','--provider','vercel'],configured('default.toml'),configured('explicit.toml'),[...configured('default.toml'),'--provider','vercel']];
  for(const args of variants) {
   const result=await exec(binary,['run','Read evidence',...args,'--workspace',cwd,'--json'],{env,timeout:5000});
   assert.equal(JSON.parse(result.stdout).outcome.output,'selected-evidence');
   await withPablo({binary,args,env},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
    const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
    const outcome=outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read evidence'}]}));
    assert(outcome.status==='completed');assert.equal(outcome.output,'selected-evidence');
   });
  }
  assert.equal(seen.length,20);
  for(let i=2;i<seen.length;i+=2) {assert.deepEqual(seen[i],seen[0]);assert.deepEqual(seen[i+1],seen[1]);}
  const traces=(await readdir(cwd)).filter(n=>n.endsWith('.jsonl'));assert.equal(traces.length,6);
  for(const name of traces) {
   const raw=await readFile(join(cwd,name),'utf8');assert(!raw.includes('private-'));
   const events=raw.trim().split('\n').map(s=>JSON.parse(s));
   assert.equal(events.filter(e=>e.type==='run.finished').length,1);
   assert.equal(events.filter(e=>e.type==='model.started').length,2);
   assert(events.filter(e=>e.type==='model.started').every(e=>e.provider==='vercel' && e.model==='zai/glm-5.3-flash'));
  }
 } finally {await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('P01 OpenRouter defaults inspect offline and unavailable Open Responses cannot reach credentials, trace or fixture endpoint', {timeout:15000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-p01-unavailable-')));let requests=0;
 const gateway=await server((_req,res)=>{requests++;res.end();});
 try {
  const file=join(cwd,'entry.toml');await writeFile(file,preset.replace('provider.vercel','provider.openrouter').replace('UNREAD_SYNTHETIC_KEY','OPENROUTER_API_KEY')+'[options.model]\nprovider="openrouter"\n');
  const env={...cleanEnv(),AI_GATEWAY_API_KEY:'private-vercel',OPENROUTER_API_KEY:'invalid private router',PABLO_FIXTURE_ENDPOINT:gateway.url+'/v1/chat/completions'};
  const configured=['--config',file,'--bind',`workspace=${cwd}`];
  const inspected=JSON.parse((await exec(binary,['config','explain',...configured],{env,timeout:5000})).stdout);
  assert.equal(inspected.config.options.model.id,'z-ai/glm-5.3-flash');assert.equal(inspected.config.options.model.endpoint,'https://openrouter.ai/api/v1/chat/completions');
  assert(!JSON.stringify(inspected).includes('private-'));
  await writeFile(file,(await readFile(file,'utf8')).replace('provider="openrouter"','provider="open_responses"'));
  for(const command of [['run','task'],['acp','--stdio']]) {
   for(const args of [['--provider','open_responses','--env-file',join(cwd,'never-read.env')],configured,[...configured,'--fixture-endpoint',gateway.url+'/v1/chat/completions']]) {
    await assert.rejects(exec(binary,[...command,...args],{env,timeout:5000}),e=>{
     const error=e as {stdout:string,stderr:string,code:number};assert.equal(error.code,2);assert.equal(error.stdout,'');assert.match(error.stderr,/provider_unavailable|config_unsupported_feature/);assert.match(error.stderr,/C3.9/);assert(!error.stderr.includes('private'));return true;
    });
   }
  }
  assert.equal(requests,0);assert.deepEqual(await readdir(cwd),['entry.toml']);
 } finally {await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
