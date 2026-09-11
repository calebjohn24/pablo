import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,realpath,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';import {join} from 'node:path';import {fileURLToPath} from 'node:url';
import {Ajv2020} from 'ajv/dist/2020.js';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
import {responsesEvents,responsesWire} from './fixtures/open-responses.ts';
import {withPablo,structuredOf} from '../examples/acp-client.ts';
const exec=promisify(execFile),binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const schema={type:'object',properties:{answer:{type:'integer',minimum:1}},required:['answer'],additionalProperties:false};
const ajv=new Ajv2020({strict:false}),checkTask=ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/pablo-task.schema.json',import.meta.url),'utf8'))),checkMeta=ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/pablo-acp-v1.schema.json',import.meta.url),'utf8')));
const textOf=(m:any)=>typeof m?.content==='string'?m.content:(m?.content??[]).map((v:any)=>v.text??'').join('');
for(const provider of ['vercel','openrouter','open_responses'])for(const mode of ['valid','invalid','first_valid','disabled','tool_history','provider_failure'])test(`J02 ${provider} ${mode}: one repair, same conversation and negotiated result`,{timeout:15000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-repair-')));const requests:any[]=[];let first:any;let firstFinal:any;
 const gateway=await server(async(req,res)=>{
  const r=JSON.parse((await body(req)).toString());requests.push(r);const history=r.input??r.messages;const last=history.at(-1);const repairing=textOf(last).startsWith('Correct the previous final answer.');
  const prefix=r.instructions??textOf(history.find((m:any)=>m.role==='system'));
  if(!repairing&&!(last.role==='tool'||last.type==='function_call_output')){first={history:structuredClone(history),prefix,tools:structuredClone(r.tools)};firstFinal=undefined;}
  assert.equal(prefix,first.prefix);assert.deepEqual(r.tools,first.tools);
  if(repairing){
   if(r.tools?.length)assert.equal(r.tool_choice,'none');assert(textOf(last).includes('/answer'));assert(Buffer.byteLength(textOf(last))<=512);
   assert.deepEqual(history.slice(0,firstFinal.length),firstFinal);
   assert.equal(history.length,firstFinal.length+2);
   const candidate=history.at(-2);assert.equal(textOf(candidate),'{"answer":0}');
   if(provider==='open_responses')assert.equal(candidate.id,'msg_measure','preserve private response item identity');
   if(mode==='provider_failure'){res.writeHead(503);res.end();return;}
  }
  const tool=mode==='tool_history'&&!repairing&&last.role!=='tool'&&last.type!=='function_call_output';
  if(!tool&&!repairing)firstFinal=structuredClone(history);
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(tool){
   res.end(provider==='open_responses'?responsesWire(responsesEvents(r,{call:{id:'read-once',name:'fs_read',arguments:'{"path":"evidence.txt"}'}})):`data: ${JSON.stringify({choices:[{index:0,delta:{tool_calls:[{index:0,id:'read-once',type:'function',function:{name:'fs_read',arguments:'{"path":"evidence.txt"}'}}]},finish_reason:'tool_calls'}]})}\n\ndata: [DONE]\n\n`);return;
  }
  const text=(repairing&&mode!=='invalid')||mode==='first_valid'?'{"answer":7}':'{"answer":0}';
  res.end(provider==='open_responses'?responsesWire(responsesEvents(r,{chunks:[text]})):`data: ${JSON.stringify({choices:[{index:0,delta:{content:text},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);
 });
 try{
  await writeFile(join(cwd,'evidence.txt'),'task-relevant fact');const entry=join(cwd,'entry.toml');const model=provider==='vercel'?'zai/glm-5.3-flash':provider==='openrouter'?'z-ai/glm-5.3-flash':'fixture-text-tools-v1';
  await writeFile(entry,`schema_version=1
[options]
model_route="repair-route"
[options.routes.repair-route]
entries=[{model="primary"},{model="fallback"}]
[options.models.primary]
provider="${provider}"
id="${model}"
credential="key"
${provider==='open_responses'?'endpoint="https://responses.example.test/v1/responses"\ncapability_profile="open-responses-text-tools-v1"':''}
[options.models.fallback]
provider="${provider}"
id="${model}-fallback"
credential="key"
${provider==='open_responses'?'endpoint="https://responses.example.test/v1/responses"\ncapability_profile="open-responses-text-tools-v1"':''}
[credentials.key]
consumer="provider.${provider}"
sources=[{kind="environment",name="UNREAD_REPAIR_KEY"}]
[options.output]
schema='${JSON.stringify(schema)}'
[options.output.repair]
enabled=${mode!=='disabled'}
max_feedback_bytes=512
[options.limits]
max_model_calls=4
max_output_tokens=128
[options.shell]
enabled=false
[options.filesystem]
enabled=${mode==='tool_history'}
`);
  const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url],env=cleanEnv();const trace=join(cwd,'trace.jsonl');
  const cli=await exec(binary,['run','Return the answer',...args,'--json','--trace',trace],{env}).catch((e:any)=>{assert.equal(e.code,1,e.stderr);return e;});const task=JSON.parse(cli.stdout);assert(checkTask(task),ajv.errorsText(checkTask.errors));
  const success=['valid','first_valid','tool_history'].includes(mode);assert.equal(task.outcome.status,success?'completed':'failed');
  const calls=mode==='tool_history'?3:['first_valid','disabled'].includes(mode)?1:2;assert.equal(task.accounting.model_calls,String(calls));assert.equal(task.accounting.tool_calls,mode==='tool_history'?'1':'0');
  if(mode==='disabled'){assert.equal(task.schema_version,'c3.13');assert.equal(task.output_repair,undefined);}else{
   assert.equal(task.schema_version,'c3.14');assert.equal(task.output_repair.status,mode==='first_valid'?'not_needed':success?'succeeded':'failed');assert.equal(task.output_repair.attempts,mode==='first_valid'?0:1);
  }
  assert.equal(task.output_validation.status,mode==='provider_failure'?'unvalidated':success?'valid':'invalid');
  if(success)assert.equal(task.outcome.output,'{"answer":7}');
  const native=(await readFile(trace,'utf8')).trim().split('\n').map(s=>JSON.parse(s));assert.deepEqual(native.at(-1).output_repair,task.output_repair);assert(!JSON.stringify(native).includes('Correct the previous'));
  for(const cap of ['repair','output','legacy','generic']){
   await withPablo({binary,args,env},async cx=>{
    const caps=cap==='generic'?{}:{'pablo/v1':true,'pablo/task-v1':true,'pablo/output-v1':cap!=='legacy','pablo/output-repair-v1':cap==='repair'};
    const init=await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:caps}});assert.equal(init.agentCapabilities?._meta?.['pablo/output-repair-v1'],true);
    const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});let response:any,meta:any;
    try{response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Return the answer'}]});meta=response._meta?.['pablo/v1'];}catch(e:any){assert(!success);meta=e.data?.['pablo/v1'];}
    if(cap==='generic'){assert.equal(meta,undefined);return;}assert(checkMeta(meta),ajv.errorsText(checkMeta.errors));assert.deepEqual(meta.task.outcome,task.outcome);
    assert.equal(meta.task.schema_version,cap==='legacy'?'c2.3':cap==='output'||mode==='disabled'?'c3.13':'c3.14');
    assert.deepEqual(meta.task.output_repair,cap==='repair'?task.output_repair:undefined);
    if(success&&cap!=='legacy')assert.deepEqual(structuredOf(response),{answer:7});
   });
  }
  assert.equal(requests.length,5*calls,'no fallback, replay, replacement session or third repair');
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
