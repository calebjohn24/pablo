import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,realpath,writeFile,readFile,rm,symlink} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {Ajv2020} from 'ajv/dist/2020.js';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
import {responsesEvents,responsesWire} from './fixtures/open-responses.ts';
import {withPablo,structuredOf,taskOf} from '../examples/acp-client.ts';
const exec=promisify(execFile),binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const schema={$schema:'https://json-schema.org/draft/2020-12/schema',$defs:{positive:{type:'integer',minimum:1}},type:'object',properties:{answer:{$ref:'#/$defs/positive'},email:{type:'string',format:'email'}},required:['answer'],additionalProperties:false};
const ajv=new Ajv2020({strict:false});const checkTask=ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/pablo-task.schema.json',import.meta.url),'utf8')));const checkMeta=ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/pablo-acp-v2.schema.json',import.meta.url),'utf8')));
const textOf=(m:any)=>typeof m.content==='string'?m.content:(m.content??[]).map((v:any)=>v.text??'').join('');
const cases=[{name:'valid',text:'{"answer":7,"email":"not an email"}',status:'valid',code:null},{name:'malformed',text:'preface {"answer":7}',status:'invalid',code:'output_validation_failed'},{name:'violation',text:'{"answer":0}',status:'invalid',code:'output_validation_failed'},{name:'work',text:'{"answer":7}',status:'invalid',code:'output_validation_failed'},{name:'output_bytes',text:'{"answer":123456789}',status:'unvalidated',code:null}];
for(const provider of ['vercel','openrouter','open_responses'])for(const c of cases)test(`J01 ${provider} ${c.name}: local CLI/ACP validation and capability fallback`,{timeout:15000},async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-output-')));let requests=0;let prefix:unknown;
 const gateway=await server(async(req,res)=>{
  requests++;assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');const r=JSON.parse((await body(req)).toString());const current=r.instructions??textOf(r.messages.find((m:any)=>m.role==='system'));assert(current.includes('Final answer contract:'));assert(current.includes('"$defs"'));if(prefix===undefined)prefix=current;else assert.equal(current,prefix);
  res.writeHead(200,{'content-type':'text/event-stream'});res.end(provider==='open_responses'?responsesWire(responsesEvents(r,{chunks:[c.text]})):`data: ${JSON.stringify({choices:[{index:0,delta:{content:c.text},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);
 });
 try {
  const file=join(cwd,'schema.json');await writeFile(file,JSON.stringify(schema));const entry=join(cwd,'entry.toml');const model=provider==='vercel'?'zai/glm-5.3-flash':provider==='openrouter'?'z-ai/glm-5.3-flash':'fixture-text-tools-v1';
  await writeFile(entry,`schema_version=1
[options.model]
provider="${provider}"
id="${model}"
credential="key"
${provider==='open_responses'?'endpoint="https://responses.example.test/v1/responses"\ncapability_profile="open-responses-text-tools-v1"':''}
[credentials.key]
consumer="provider.${provider}"
sources=[{kind="environment",name="UNREAD_OUTPUT_KEY"}]
[options.output]
schema={base="workspace",path="schema.json"}
max_validation_work=${c.name==='work'?1:16777216}
[options.limits]
max_model_calls=1
max_output_tokens=128
max_output_bytes=${c.name==='output_bytes'?8:4096}
[options.shell]
enabled=false
[options.filesystem]
enabled=false
`);
  const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url],env=cleanEnv();const trace=join(cwd,'trace.jsonl');
  const cli=await exec(binary,['run','Return the answer',...args,'--json','--trace',trace],{env}).catch((e:any)=>{assert.equal(e.code,1);return e;});const task=JSON.parse(cli.stdout);assert(checkTask(task),ajv.errorsText(checkTask.errors));assert.equal(task.schema_version,'c3.33');assert.equal(task.output_validation.status,c.status);assert.equal(task.outcome.code??null,provider==='open_responses'&&c.name==='output_bytes'?'malformed_stream':c.code);assert.equal(task.accounting.model_calls,'1');
  if(c.name==='output_bytes'&&provider!=='open_responses')assert.equal(task.outcome.limit,'output_bytes');
  if(c.name==='valid')assert.deepEqual(JSON.parse(task.outcome.output),{answer:7,email:'not an email'});
  if(c.name==='violation')assert.equal(task.output_validation.diagnostics[0].instance_path,'/answer');
  const native=(await readFile(trace,'utf8')).trim().split('\n').map(s=>JSON.parse(s));assert.equal(native.at(-1).output_validation.status,c.status);assert(native.filter(e=>e.type==='assistant.text.delta').every(e=>e.output_validation.status==='unvalidated'));
  assert(native.at(-1).output_validation.diagnostics.every((d:any)=>d.instance_path===''&&d.schema_path===''));
  for(const mode of ['output','legacy','generic']) {
   const updates:any[]=[];
   await withPablo({binary,args,env,onUpdate:n=>{updates.push(n);}},async cx=>{
    const caps=mode==='generic'?{}:{'pablo/v2':true,'pablo/task-v2':true,'pablo/output-v1':mode==='output'};
    const init=await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:caps}});assert.equal(init.agentCapabilities?._meta?.['pablo/output-v1'],true);
    const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});let response:any,details:any;
    try{response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Return the answer'}]});details=response._meta?.['pablo/v2'];}catch(e:any){assert.notEqual(c.status,'valid');details=e.data?.['pablo/v2'];}
    if(mode==='generic'){assert.equal(details,undefined);return;}
    assert(checkMeta(details),ajv.errorsText(checkMeta.errors));assert.deepEqual(details.task.outcome,task.outcome);
    if(mode==='output'){
     assert.equal(details.task.output_validation.status,c.status);assert.equal(details.task.schema_version,'c3.33');
     if(c.status==='valid'){assert.deepEqual(structuredOf(response),{answer:7,email:'not an email'});assert.equal(taskOf(response).output_validation?.status,'valid');}
    }else{assert.equal(details.task.output_validation,undefined);assert.equal(details.task.schema_version,'c3.33');}
    for(const u of updates.filter(n=>n.update.sessionUpdate==='agent_message_chunk'))assert.equal(u._meta?.['pablo/v2']?.output_validation?.status,mode==='output'?'unvalidated':undefined);
   });
  }
  assert.equal(requests,4,'validation never requests repair or fallback');
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('J01 schema files are pinned, rendered inline, replaceable and rejected before dispatch when unsupported',async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-schema-admission-')));try{
  const file=join(cwd,'schema.json'),entry=join(cwd,'entry.toml');await writeFile(file,JSON.stringify(schema));const env=cleanEnv();const flags=['--config',entry,'--bind',`workspace=${cwd}`];
  await writeFile(entry,'schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.output]\nschema={base="workspace",path="schema.json"}\n');
  const explain=async()=>JSON.parse((await exec(binary,['config','explain',...flags],{env})).stdout);const original=await explain();assert.equal(typeof original.config.options.output.schema,'string');assert.deepEqual(JSON.parse(original.config.options.output.schema),schema);
  const rendered=join(cwd,'rendered.toml');await writeFile(rendered,(await exec(binary,['config','render',...flags],{env})).stdout);await writeFile(file,'{"type":"null"}');const changed=await explain();assert.notEqual(changed.fingerprint,original.fingerprint);
  const reloaded=JSON.parse((await exec(binary,['config','explain','--config',rendered,'--bind',`workspace=${cwd}`],{env})).stdout);assert.equal(reloaded.fingerprint,original.fingerprint);
  for(const invalid of [{$ref:'https://example.invalid/schema'},{$ref:'#'},{$defs:{bad:{$ref:'#/$defs/bad'}}},{pattern:'secret-pattern'},{$vocabulary:{'https://example.invalid/vocab':true}},{$schema:'https://json-schema.org/draft-07/schema'},JSON.parse('{"type":"integer","minimum":"private-invalid"}')]){
   await writeFile(file,JSON.stringify(invalid));await assert.rejects(exec(binary,['config','validate',...flags],{env}),e=>{assert.equal((e as any).code,2);assert.match((e as any).stderr,/config_invalid_value/);assert(!(e as any).stderr.includes('private-invalid'));return true;});
  }
  await writeFile(file,'{"type":"integer"}');const link=join(cwd,'link.json');await symlink(file,link);await writeFile(entry,'schema_version=1\n[options.output]\nschema={base="workspace",path="link.json"}\n');await assert.rejects(exec(binary,['config','validate',...flags],{env}));
  // Locked override admission must precede opening a nonexistent schema file.
  await writeFile(entry,'schema_version=1\n[deployment]\nlocked=true\nallowed_run_overrides=["input"]\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n');await assert.rejects(exec(binary,['run','x',...flags,'--output-schema',join(cwd,'missing.json'),'--json'],{env}),e=>{assert.match((e as any).stderr,/config_override_forbidden/);return true;});
 }finally{await rm(cwd,{recursive:true,force:true});}
});

test('J01 legacy schema flag validates final JSON with the same canonical digest as configuration', async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-schema-flag-')));let requests=0;
 const gateway=await server(async(req,res)=>{await body(req);requests++;res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'{"answer":7}'},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);});
 try{
  const file=join(cwd,'schema.json');await writeFile(file,JSON.stringify(schema));
  const env={...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url};const common=['--workspace',cwd,'--no-shell','--no-filesystem','--json'];
  const legacy=JSON.parse((await exec(binary,['run','answer','--output-schema',file,...common],{env,cwd})).stdout);
  const entry=join(cwd,'entry.toml');await writeFile(entry,`schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.output]\nschema='${JSON.stringify(schema)}'\n`);
  const configured=JSON.parse((await exec(binary,['run','answer','--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,...common],{env,cwd})).stdout);
  assert.equal(legacy.output_validation.status,'valid');assert.deepEqual(configured.output_validation,legacy.output_validation);assert.equal(requests,2);
 }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
