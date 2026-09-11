import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,readFile,realpath,rm,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {Ajv2020} from 'ajv/dist/2020.js';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
import {responsesEvents,responsesWire} from './fixtures/open-responses.ts';
import {withPablo,taskOf} from '../examples/acp-client.ts';
const exec=promisify(execFile);
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const ajv=new Ajv2020({strict:false});
const validateMeta=ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/pablo-acp-v1.schema.json',import.meta.url),'utf8')));
const textOf=(m:any)=>typeof m.content==='string'?m.content:(m.content??[]).map((p:any)=>p.text??'').join('');
const chat=(delta:object,reason:string)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:reason}]})}\n\ndata: [DONE]\n\n`;
const summary='Goal: inspect evidence.txt in three steps. Preserve workspace scope. Keep its artifact revision and continue unresolved work; completed reads must not be replayed.';
function privateResponse(request:any,output:{chunks?:string[],call?:{id:string,name:string,arguments:string}},id:string) {
  const events=responsesEvents(request,output);
  for(const e of events)if(e.output_index!==undefined)e.output_index++;
  const item={type:'reasoning',id:`rs_${id}`,summary:[],encrypted_content:`PRIVATE_COMPACTION_STATE_${id}`};
  events.splice(2,0,{type:'response.output_item.added',output_index:0,item:structuredClone(item)},{type:'response.output_item.done',output_index:0,item:structuredClone(item)});
  events.at(-1)!.response.output.unshift(item);
  events.forEach((e,i)=>e.sequence_number=i);
  return responsesWire(events);
}
for(const retain of [0,1])for(const provider of ['vercel','openrouter','open_responses'])for(const trigger of ['threshold','overflow'])test(`CP01/CP02 ${provider} ${trigger} retain=${retain}: CLI and isolated ACP sessions compact real tool history`,{timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-compaction-')));
  const effectful=provider==='vercel'&&trigger==='threshold';
  const facts=['Budget USD 731.25; no production changes.','Decision: use release r17 because r16 failed E_REVISION.','Pending: verify results/alpha.json at revision abc123.'];
  const handoff=summary+(retain===0?' '+facts.join(' '):'');
  const model=provider==='vercel'?'zai/glm-5.3-flash':provider==='openrouter'?'z-ai/glm-5.3-flash':'fixture-text-tools-v1';
  const runs=new Map<string,{reads:number,summaries:number,overflow:boolean,after:boolean,calls:number,prefix:unknown,tools:unknown,artifact:string}>();
  const gateway=await server(async(req,res)=>{
    assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');
    const r=JSON.parse((await body(req)).toString());assert.equal(r.model,model);
    const messages=r.messages??r.input;const task=textOf(messages.find((m:any)=>m.role==='user'));
    const prefix=r.instructions??textOf(messages.find((m:any)=>m.role==='system'));
    const state=runs.get(task)??{reads:0,summaries:0,overflow:false,after:false,calls:0,prefix,tools:r.tools,artifact:`artifact-${runs.size}.txt`};runs.set(task,state);state.calls++;
    assert.deepEqual(prefix,state.prefix);assert.deepEqual(r.tools,state.tools);
    const summarizing=textOf(messages.at(-1)).startsWith('Create a concise handoff');
    const respond=(output:{chunks?:string[],call?:{id:string,name:string,arguments:string}},id:string)=>{
      res.writeHead(200,{'content-type':'text/event-stream'});
      res.end(provider==='open_responses'?privateResponse(r,output,id):output.call?chat({tool_calls:[{index:0,id:output.call.id,type:'function',function:{name:output.call.name,arguments:output.call.arguments}}]},'tool_calls'):chat({content:output.chunks!.join('')},'stop'));
    };
    if(summarizing){
      state.summaries++;assert.equal(state.summaries,1);assert.equal(r.tool_choice,'none');
      if(provider==='open_responses')assert.deepEqual(messages.filter((m:any)=>m.type==='reasoning').map((m:any)=>m.id),Array.from({length:state.reads-retain},(_,i)=>`rs_read${i+1}`));
      if(retain===0)for(const fact of facts.slice(effectful?1:0))assert(JSON.stringify(messages).includes(fact));
      respond({chunks:[handoff+(effectful?` Artifact: ${state.artifact} was created once.`:'')]},'summary');return;
    }
    if(state.summaries&&!state.after){
      state.after=true;const users=messages.filter((m:any)=>m.role==='user');assert(textOf(users[1]).includes(handoff));
      const tools=messages.filter((m:any)=>m.role==='tool'||m.type==='function_call_output');assert.equal(tools.length,retain);
      if(provider==='open_responses')assert.deepEqual(messages.filter((m:any)=>m.type==='reasoning').map((m:any)=>m.id),retain?[`rs_read${state.reads}`]:[]);
    }
    if(state.reads<3){state.reads++;const writing=effectful&&state.reads===1;respond({call:{id:`read${state.reads}`,name:writing?'fs_write':'fs_read',arguments:JSON.stringify(writing?{path:state.artifact,text:'written-once',expected_revision:null}:{path:retain===0?`evidence-${state.reads}.txt`:'evidence.txt'})}},`read${state.reads}`);return;}
    if(trigger==='overflow'&&!state.overflow){state.overflow=true;res.writeHead(400,{'content-type':'application/json'});res.end(JSON.stringify({error:{code:'context_length_exceeded',message:'PRIVATE_ERROR_COMPACTION'}}));return;}
    assert.equal(state.summaries,1);respond({chunks:['done']},'final');
  });
  try{
    await writeFile(join(cwd,'evidence.txt'),'e'.repeat(6000));
    for(let i=1;i<=3;i++)await writeFile(join(cwd,`evidence-${i}.txt`),'e'.repeat(6000)+' '+facts[i-1]);
    const entry=join(cwd,'entry.toml');
    const config=(capture:boolean)=>`schema_version=1\n[options.model]\nprovider="${provider}"\nid="${model}"\ncredential="key"\ncontext_window_tokens=${trigger==='threshold'?(retain===0?(effectful?'6000':'8000'):(effectful?'4000':'5000')):'{unset=true}'}\n${provider==='open_responses'?'endpoint="https://responses.example.test/v1/responses"\ncapability_profile="open-responses-text-tools-v1"\n':''}[credentials.key]\nconsumer="provider.${provider}"\nsources=[{kind="environment",name="COMPACTION_FIXTURE_KEY"}]\n[options.run]\ninstructions="Preserve workspace scope and original task constraints."\n[options.context]\nmax_summary_tokens=128\nkeep_recent_turns=${retain}\n[options.limits]\nmax_output_tokens=${retain===0?2048:128}\nmax_model_calls=8\n[options.shell]\nenabled=false\n[options.filesystem]\nwrite=${effectful}\n[options.trace]\ncapture_content=${capture}\n${capture?'path={base="workspace",path="acp-{session_id}.jsonl"}\n':''}`;
    await writeFile(entry,config(false));const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const trace=join(cwd,'cli.jsonl');const cli=JSON.parse((await exec(binary,['run','CLI: inspect evidence in three steps',...args,'--json','--trace',trace],{env:cleanEnv()})).stdout);
    assert.equal(cli.outcome.output,'done');assert.equal(cli.accounting.model_calls,trigger==='overflow'?'6':'5');assert.equal(cli.accounting.tool_calls,'3');
    const native=await readFile(trace,'utf8');assert(!native.includes(handoff));assert(!native.includes('PRIVATE_'));
    const events=native.trim().split('\n').map(s=>JSON.parse(s));const finished=events.filter(e=>e.type==='context.compaction.finished');assert.equal(finished.length,1);assert.equal(finished[0].summary,null);
    const record=finished[0].compaction;assert.equal(record.trigger,trigger);assert.equal(record.status,'completed');assert(BigInt(record.after_bytes)<BigInt(record.before_bytes));if(retain===0)assert(Number(record.after_bytes)/Number(record.before_bytes)<0.3);assert.equal(events.filter(e=>e.type==='assistant.text.delta').length,1);
    await writeFile(entry,config(true));const notifications:any[]=[];const answers:string[]=[];
    await withPablo({binary,args,env:cleanEnv(),onCompaction:n=>{notifications.push(n);},onUpdate:n=>{if(n.update.sessionUpdate==='agent_message_chunk'&&n.update.content.type==='text')answers.push(n.update.content.text);}},async cx=>{
      const init=await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true,'pablo/compaction-v1':true}}});assert.equal(init.agentCapabilities?._meta?.['pablo/compaction-v1'],true);
      for(let i=0;i<2;i++){
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:`ACP ${i}: inspect evidence in three steps`}]});
        const task=taskOf(response);assert.deepEqual(task.outcome,cli.outcome);assert.deepEqual(task.accounting,cli.accounting);
        const captured=await readFile(join(cwd,`acp-${sessionId}.jsonl`),'utf8');assert(captured.includes(handoff));assert(!captured.includes('PRIVATE_'));
        const details:any=response._meta?.['pablo/v1'];assert(validateMeta(details),ajv.errorsText(validateMeta.errors));assert.equal(details.compaction.status,'completed');
        const local=notifications.filter(n=>n.sessionId===sessionId);assert.equal(local.length,2);assert.equal(local[1].summary,handoff+(effectful?` Artifact: ${runs.get(`ACP ${i}: inspect evidence in three steps`)!.artifact} was created once.`:''));
        for(const n of local)assert(validateMeta(n['pablo/v1']),ajv.errorsText(validateMeta.errors));
      }
    });
    assert.equal(new Set(notifications.map(n=>n['pablo/v1'].compaction.id)).size,2);assert(!JSON.stringify(notifications).includes('PRIVATE_'));assert.deepEqual(answers,['done','done']);
    if(effectful)for(const state of runs.values())assert.equal(await readFile(join(cwd,state.artifact),'utf8'),'written-once');
    assert.equal(runs.size,3);assert([...runs.values()].every(s=>s.reads===3&&s.summaries===1&&s.after));
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

const failures=[
  {mode:'unknown',code:'provider_rejected',calls:4,summaries:0},
  {mode:'oversized_error',code:'provider_rejected',calls:4,summaries:0},
  {mode:'partial',code:'context_overflow',calls:4,summaries:0},
  {mode:'no_history',code:'compaction_failed',calls:1,summaries:0},
  {mode:'second_overflow',code:'context_overflow',calls:6,summaries:1},
  {mode:'summary_error',code:'provider_rejected',calls:5,summaries:1},
  {mode:'recovery_error',code:'provider_rejected',calls:6,summaries:1},
  {mode:'stream_code',code:null,calls:6,summaries:1},
  {mode:'nested_error',code:null,calls:6,summaries:1},
];
for(const scenario of failures)test(`CP02 CLI/ACP bounded recovery: ${scenario.mode}`,{timeout:10000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-compaction-failure-')));
  const runs=new Map<string,{reads:number,summary:number,calls:number,overflow:boolean}>();
  const gateway=await server(async(req,res)=>{
    const r=JSON.parse((await body(req)).toString());assert.equal(r.model,'zai/glm-5.3-flash','summary/recovery must not fall through to another route entry');
    const task=textOf(r.messages.find((m:any)=>m.role==='user'));const state=runs.get(task)??{reads:0,summary:0,calls:0,overflow:false};runs.set(task,state);state.calls++;
    const respond=(delta:object,reason:string)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end(chat(delta,reason));};
    if(textOf(r.messages.at(-1)).startsWith('Create a concise handoff')){
      state.summary++;assert.equal(state.summary,1);assert.equal(r.tool_choice,'none');
      if(scenario.mode==='summary_error'){res.writeHead(503);res.end('PRIVATE_SUMMARY_ERROR');return;}
      respond({content:summary},'stop');return;
    }
    if(scenario.mode!=='no_history'&&state.reads<3){state.reads++;respond({tool_calls:[{index:0,id:`r${state.reads}`,type:'function',function:{name:'fs_read',arguments:'{"path":"evidence.txt"}'}}]},'tool_calls');return;}
    if(state.summary&&scenario.mode==='recovery_error'){res.writeHead(503);res.end('PRIVATE_RECOVERY_ERROR');return;}
    if(!state.overflow||scenario.mode==='second_overflow'){
      state.overflow=true;
      const error={code:'context_length_exceeded',message:'PRIVATE_CONTEXT_ERROR'};
      if(scenario.mode==='stream_code'||scenario.mode==='partial'){
        res.writeHead(200,{'content-type':'text/event-stream'});
        if(scenario.mode==='partial')res.write(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'escaped'},finish_reason:null}]})}\n\n`);
        res.end(`data: ${JSON.stringify({error})}\n\n`);return;
      }
      res.writeHead(400,{'content-type':'application/json'});
      res.end(JSON.stringify(scenario.mode==='unknown'?{error:{code:'invalid_request',message:'context_length_exceeded PRIVATE_CONTEXT_ERROR'}}:scenario.mode==='oversized_error'?{error,padding:'x'.repeat(8192)}:scenario.mode==='nested_error'?{error:{code:400,metadata:{raw:JSON.stringify({error})}}}:{error}));return;
    }
    respond({content:'done'},'stop');
  });
  try{
    await writeFile(join(cwd,'evidence.txt'),'e'.repeat(6000));const entry=join(cwd,'entry.toml');
    await writeFile(entry,`schema_version=1
[options]
model_route="main"
[options.models.primary]
provider="vercel"
id="zai/glm-5.3-flash"
credential="primary"
[options.models.secondary]
provider="openrouter"
id="z-ai/glm-5.3-flash"
credential="secondary"
[options.routes.main]
entries=[{model="primary"},{model="secondary"}]
[credentials.primary]
consumer="provider.vercel"
sources=[{kind="environment",name="COMPACTION_PRIMARY"}]
[credentials.secondary]
consumer="provider.openrouter"
sources=[{kind="environment",name="COMPACTION_SECONDARY"}]
[options.context]
max_summary_tokens=128
[options.limits]
max_output_tokens=128
max_model_calls=8
[options.shell]
enabled=false
`);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];const trace=join(cwd,'cli.jsonl');
    const cli=await exec(binary,['run','CLI failure fixture',...args,'--json','--trace',trace],{env:cleanEnv()}).catch((e:any)=>{assert.equal(e.code,1);return e;});
    const task=JSON.parse(cli.stdout);assert.equal(task.outcome.code??null,scenario.code);assert.equal(task.accounting.model_calls,String(scenario.calls));
    const native=await readFile(trace,'utf8');assert(!native.includes('PRIVATE_'));const events=native.trim().split('\n').map(s=>JSON.parse(s));assert.equal(events.filter(e=>e.type==='run.finished').length,1);
    const compacted=events.filter(e=>e.type==='context.compaction.finished');assert.equal(compacted.length,scenario.summaries||scenario.mode==='no_history'?1:0);
    if(scenario.mode==='no_history')assert.equal(compacted[0].compaction.reason,'no_history');
    const notifications:any[]=[];
    for(const negotiated of [true,false]) {
    await withPablo({binary,args,env:cleanEnv(),onCompaction:n=>{notifications.push(n);}},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true,'pablo/compaction-v1':negotiated}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});let details:any;
      try{details=(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:`ACP ${negotiated} failure fixture`}]}))._meta?.['pablo/v1'];}catch(e:any){details=e.data?.['pablo/v1'];assert(details,String(e));}
      assert(validateMeta(details),ajv.errorsText(validateMeta.errors));assert.deepEqual(details.task.outcome,task.outcome);assert.deepEqual(details.task.accounting,task.accounting);if(!negotiated)assert.equal(details.compaction,undefined);
    });
    }
    assert.equal(notifications.length,compacted.length*2);assert(notifications.every(n=>n.summary===null));assert(!JSON.stringify(notifications).includes('PRIVATE_'));
    assert([...runs.values()].every(s=>s.calls===scenario.calls&&s.summary===scenario.summaries));
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('context options retain per-profile capacities through import, render and reload; invalid bounds reject offline',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-context-options-')));
  try {
    const entry=join(cwd,'entry.toml'),base=join(cwd,'base.toml');
    await writeFile(base,`schema_version=1
[options]
model_route="main"
[options.models.primary]
provider="vercel"
id="zai/glm-5.3-flash"
credential="key"
context_window_tokens=100000
[options.models.secondary]
provider="vercel"
id="fixture/secondary"
credential="key"
[options.routes.main]
entries=[{model="primary"},{model="secondary"}]
[credentials.key]
consumer="provider.vercel"
sources=[{kind="environment",name="UNREAD_CONTEXT_KEY"}]
[options.context]
keep_recent_turns=2
`);
    const config='schema_version=1\nimports=["base.toml"]\n[options.context]\nmax_summary_tokens=256\n';
    await writeFile(entry,config);const args=['--config',entry,'--bind',`workspace=${cwd}`],env=cleanEnv();
    const explain=async(file:string)=>JSON.parse((await exec(binary,['config','explain','--config',file,'--bind',`workspace=${cwd}`],{env})).stdout);
    const resolved=await explain(entry);assert.equal(resolved.config.options.context.keep_recent_turns,2);assert.equal(resolved.config.options.context.max_summary_tokens,256);
    assert.equal(resolved.model_route.entries[0].profile.context_window_tokens,100000);assert.equal(resolved.model_route.entries[1].profile.context_window_tokens,undefined);
    const rendered=join(cwd,'rendered.toml');await writeFile(rendered,(await exec(binary,['config','render',...args],{env})).stdout);
    const reloaded=await explain(rendered);assert.equal(reloaded.fingerprint,resolved.fingerprint);assert.deepEqual(reloaded.model_route,resolved.model_route);
    await writeFile(entry,config+'[options.models.primary]\ncontext_window_tokens={unset=true}\n');assert.equal((await explain(entry)).model_route.entries[0].profile.context_window_tokens,undefined);
    for(const extra of ['[options.context]\nmax_summary_tokens=15','[options.context]\nsafety_margin_percent=0','[options.context]\nkeep_recent_turns=-1','[options.model]\ncontext_window_tokens=0','[options.models.primary]\ncontext_window_tokens=1000000001']){
      await writeFile(entry,'schema_version=1\nimports=["base.toml"]\n'+extra+'\n');
      await assert.rejects(exec(binary,['config','validate',...args],{env}),e=>{assert.equal((e as any).code,2);return true;});
    }
  }finally{await rm(cwd,{recursive:true,force:true});}
});
