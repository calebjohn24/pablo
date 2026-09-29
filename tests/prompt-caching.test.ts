import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,realpath,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
const exec=promisify(execFile),binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
test('prompt caching CLI defaults, auto serialization and rejection before credential loading', async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-cache-')));let expected:any;
 const gateway=await server(async(req,res)=>{
  const r=JSON.parse((await body(req)).toString());assert.deepEqual(r.providerOptions,expected);
  res.writeHead(200,{'content-type':'text/event-stream'});
  res.end('data: '+JSON.stringify({id:'gen_01ARZ3NDEKTSV4RRFFQ69G5FAV',choices:[{index:0,delta:{content:'ok'},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
 });
 try{
  for(const setting of [null,'provider_default','auto']){
   expected=setting==='auto'?{gateway:{caching:'auto'}}:undefined;
   const result=await exec(binary,['run','task','--workspace',cwd,'--no-shell','--json',...(setting?['--prompt-caching',setting]:[])],{env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url}});
   assert.equal(JSON.parse(result.stdout).outcome.status,'completed');
  }
  await assert.rejects(exec(binary,['run','task','--workspace',cwd,'--provider','openrouter','--prompt-caching','auto','--env-file',join(cwd,'does-not-exist')],{env:cleanEnv()}),(e:any)=>e.code===2&&/automatic prompt caching/.test(e.stderr));
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
