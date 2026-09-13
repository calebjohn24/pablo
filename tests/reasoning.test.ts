import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,realpath,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
const exec=promisify(execFile),binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));

test('reasoning CLI wire format is model independent and defaults remain omitted', {timeout:20000}, async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-reasoning-')));let expected:any;
 const gateway=await server(async(req,res)=>{
  const r=JSON.parse((await body(req)).toString());assert.deepEqual(r.reasoning,expected);
  res.writeHead(200,{'content-type':'text/event-stream'});
  res.end('data: '+JSON.stringify({choices:[{index:0,delta:{content:'ok'},finish_reason:'stop'}],usage:{prompt_tokens:2,completion_tokens:2,completion_tokens_details:{reasoning_tokens:1}}})+'\n\ndata: [DONE]\n\n');
 });
 try {
  for(const provider of ['openrouter','vercel'])for(const model of ['vendor/future-model','another-family'])for(const [args,wire] of [
   [[],undefined],[['--reasoning-effort','provider_default'],undefined],[['--reasoning-effort','low'],{effort:'low'}],[['--reasoning-effort','max'],{effort:'max'}],[['--reasoning-budget-tokens','1024'],{max_tokens:1024}],
  ] as [string[],any][]){
   expected=wire;const result=await exec(binary,['run','task','--provider',provider,'--model',model,'--workspace',cwd,'--no-shell','--json',...args],{env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url}});
   assert.equal(JSON.parse(result.stdout).outcome.status,'completed');
  }
  for(const args of [['--reasoning-effort','low','--reasoning-budget-tokens','5'],['--reasoning-effort','invented'],['--reasoning-budget-tokens','0'],['--reasoning-budget-tokens','65536']]){
   await assert.rejects(exec(binary,['run','task','--workspace',cwd,...args],{env:cleanEnv()}), (e:any)=>e.code===2&&/reasoning/.test(e.stderr));
  }
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('declared capabilities, locked overrides and reasoning fingerprints validate offline', {timeout:15000}, async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-reasoning-config-'))),file=join(cwd,'entry.toml');
 const config=(reasoning:string)=>`schema_version=1\n[credentials.gateway]\nconsumer="provider.openrouter"\nsources=[{kind="environment",name="UNREAD_REASONING_KEY"}]\n[options.model]\nprovider="openrouter"\nid="vendor/unknown-model"\nreasoning=${reasoning}\nreasoning_capabilities={efforts=["low","high"],mandatory=true,token_budget=false}\n`;
 const explain=async()=>JSON.parse((await exec(binary,['config','explain','--config',file,'--bind',`workspace=${cwd}`],{env:cleanEnv()})).stdout);
 try{
  await writeFile(file,config('"low"'));const low=await explain();
  await writeFile(file,config('"high"'));const high=await explain();assert.notEqual(high.fingerprint,low.fingerprint);
  for(const setting of ['"none"','"medium"','{budget_tokens=1024}','{effort="low",budget_tokens=1024}']){
   await writeFile(file,config(setting));await assert.rejects(explain(),(e:any)=>e.code===2);
  }
  await writeFile(file,config('"provider_default"'));await explain();
  await writeFile(file,config('"low"')+'[deployment]\nlocked=true\nallowed_run_overrides=["input"]\n');
  await assert.rejects(exec(binary,['run','task','--config',file,'--bind',`workspace=${cwd}`,'--reasoning-effort','high'],{env:cleanEnv()}),(e:any)=>e.code===2&&/config_override_forbidden/.test(e.stderr));
 }finally{await rm(cwd,{recursive:true,force:true});}
});

test('cancellation preserves partial timing without fabricated first text or terminal time', {timeout:10000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-reasoning-cancel-'))),trace=join(cwd,'trace.jsonl');
 const gateway=await server(async(req,res)=>{await body(req);res.writeHead(200,{'content-type':'text/event-stream'});res.write(': private heartbeat\n\n');});
 try{
  const r=await exec(binary,['run','task','--provider','openrouter','--workspace',cwd,'--timeout','1','--no-shell','--json','--trace',trace],{env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url}}).catch((e:any)=>{assert.equal(e.code,1);return e;});
  assert.equal(JSON.parse(r.stdout).outcome.status,'timed_out');
  const raw=await readFile(trace,'utf8');const d=raw.trim().split('\n').map(s=>JSON.parse(s)).find(e=>e.type==='model.finished').diagnostics;
  assert(d.headers_us>=d.dispatch_us);assert(d.first_data_us>=d.headers_us);assert(d.complete_us>=d.first_data_us);
  assert.equal(d.first_text_us,null);assert.equal(d.terminal_us,null);assert(!raw.includes('private heartbeat'));
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
