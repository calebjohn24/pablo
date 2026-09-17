import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Ajv2020 } from 'ajv/dist/2020.js';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
import { assertExtension } from './fixtures/acp-extension-schema.ts';
const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const base = await readFile(new URL('../docs/guide/examples/jev-router.toml', import.meta.url), 'utf8');
const models = { simple: 'google/gemini-3.8-flash', complex: 'openai/gpt-5.6-sol' };
const wire = (delta: object, reason = 'stop') => `data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:reason}]})}\n\ndata: [DONE]\n\n`;
const choice = (name: string) => ({answers:{complexity:{type:'choice',choice:name}},usage:{inputTokens:10,outputTokens:73}});
async function fixture(config = base) {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-jev-')));
  const entry = join(cwd, 'entry.toml'); await writeFile(entry, config);
  return {cwd, entry, args:['--config',entry,'--bind',`workspace=${cwd}`], close:()=>rm(cwd,{recursive:true,force:true})};
}

test('Jev config is offline, round-trippable and has no selected model before task start', async()=>{
  const f = await fixture();
  try {
    const run = async(args:string[]) => (await exec(binary,args,{env:cleanEnv()})).stdout;
    const explained = JSON.parse(await run(['config','explain',...f.args]));
    const ajv = new Ajv2020({strict:false});
    ajv.addSchema(JSON.parse(await readFile(new URL('../docs/project/schemas/deployment-v1.schema.json',import.meta.url),'utf8')));
    const check = ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/project/schemas/resolved-deployment-v1.schema.json',import.meta.url),'utf8')));
    assert(check(explained),ajv.errorsText(check.errors));
    assert.equal(explained.model_route.selected_entry,null);
    assert.equal(explained.model_route.selection_reason,'task_start_classification');
    assert.equal(explained.model_route.policy.max_attempts,1);
    await writeFile(f.entry,await run(['config','render',...f.args]));
    assert.equal(JSON.parse(await run(['config','explain',...f.args])).fingerprint,explained.fingerprint);
    for (const config of [
      base.replace('provider = "vercel"','provider = "openrouter"'),
      base.replace('model = "typesafe-ai/jev"','model = "other/classifier"'),
      base.replace('model = "deep"\n\n[options.shell]','model = "missing"\n\n[options.shell]'),
      base.replace('[options.routes.complexity]\n','[options.routes.complexity]\nmax_attempts = 2\n'),
      base.replace('[options.routes.complexity]\n','[options.routes.complexity]\neligible_errors = ["rate_limited"]\n'),
      base.replace('timeout_ms = 10000','timeout_ms = 0'),
      base.replace('credential = "gateway"\ntimeout','credential = "missing"\ntimeout'),
      base + '\n[[authority]]\nid="restrict"\nmodel_ids=["google/gemini-3.8-flash","openai/gpt-5.6-sol"]\n',
      base + '\n[[authority]]\nid="restrict"\nprovider_endpoints=["https://ai-gateway.vercel.sh/v1/chat/completions"]\n',
    ]) {
      await writeFile(f.entry,config);
      await assert.rejects(run(['config','validate',...f.args]));
    }
  } finally { await f.close(); }
});

test('Jev classifies once per CLI/ACP task and pins each model across tool turns', {timeout:20000}, async()=>{
  const f = await fixture(); const calls:any[]=[];
  const gateway = await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString()); calls.push(request);
    assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');
    if (request.questions) {
      assert.equal(req.headers['ai-model-id'],'typesafe-ai/jev');
      assert.equal(req.headers['ai-evaluation-model-specification-version'],'4');
      assert.equal(req.headers['ai-gateway-protocol-version'],'0.0.1');
      assert.equal(request.questions.complexity.type,'choice');
      assert.deepEqual(Object.keys(request.questions.complexity.criteria).sort(),['complex','simple']);
      assert.equal(request.messages,undefined);
      res.setHeader('content-type','application/json'); res.end(JSON.stringify(choice(request.state.task))); return;
    }
    const category=request.messages.find((m:any)=>m.role==='user').content as keyof typeof models;
    assert.equal(request.model,models[category]);
    assert(!request.messages.some((m:any)=>m.content?.includes('Classify the reasoning')));
    res.setHeader('content-type','text/event-stream');
    if(request.messages.at(-1).role==='tool') res.end(wire({content:'done'}));
    else res.end(wire({tool_calls:[{index:0,id:'read',type:'function',function:{name:'fs_read',arguments:'{"path":"evidence.txt"}'}}]},'tool_calls'));
  });
  try {
    await writeFile(join(f.cwd,'evidence.txt'),'synthetic evidence');
    const args=[...f.args,'--fixture-endpoint',gateway.url];
    const trace=join(f.cwd,'trace.jsonl');
    const cli=JSON.parse((await exec(binary,['run','complex',...args,'--json','--trace',trace,'--capture-content'],{env:cleanEnv()})).stdout);
    assert.equal(cli.outcome.output,'done'); assert.equal(cli.accounting.model_calls,'3');
    const events=(await readFile(trace,'utf8')).trim().split('\n').map(s=>JSON.parse(s));
    assert.deepEqual(events.filter(e=>e.type==='model.started').map(e=>e.model),['typesafe-ai/jev',models.complex,models.complex]);
    assert.deepEqual(events.filter(e=>e.type==='assistant.text.delta').map(e=>e.text),['done']);
    assert.equal(events.at(-1).model_route.entry,'deep');
    assert.equal(events.filter(e=>e.type==='model.started')[1].model_route.selection_reason,'complexity');
    await withPablo({binary,args,env:cleanEnv(),onModelAttempt:assertExtension},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true,'pablo/model-route-v1':true}}});
      for(const category of ['simple','complex','simple']) {
        const {sessionId}=await cx.request('session/new',{cwd:f.cwd,mcpServers:[]});
        const task=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:category}]}));
        assert.equal(task.outcome.status,'completed'); if(task.outcome.status==='completed') assert.equal(task.outcome.output,'done'); assert.equal(task.accounting.model_calls,'3');
      }
    });
    assert.equal(calls.filter(c=>c.questions).length,4);
    assert.deepEqual(calls.filter(c=>c.model).map(c=>c.model),[models.complex,models.complex,models.simple,models.simple,models.complex,models.complex,models.simple,models.simple]);
  } finally { await gateway.close(); await f.close(); }
});

for(const mode of ['unknown','malformed','oversized','rejected','timeout','generation_error','tool_error','zero_calls','one_call'] as const) {
  test(`Jev ${mode} stops without reclassification or model fallback`,{timeout:15000},async()=>{
    const f=await fixture(mode==='timeout'?base.replace('timeout_ms = 10000','timeout_ms = 30'):base);
    let evaluations=0; const generations:string[]=[];
    const gateway=await server(async(req,res)=>{
      const request=JSON.parse((await body(req)).toString());
      if(request.questions) {
        evaluations++;
        if(mode==='timeout') { await new Promise(r=>setTimeout(r,100)); res.end(); return; }
        if(mode==='rejected') {res.writeHead(503);res.end('private error');return;}
        res.setHeader('content-type','application/json');
        res.end(mode==='malformed'?'invalid':mode==='oversized'?'x'.repeat(65537):JSON.stringify(choice(mode==='unknown'?'missing':'simple'))); return;
      }
      generations.push(request.model);
      if(mode==='tool_error' && generations.length===1) {
        res.setHeader('content-type','text/event-stream');res.end(wire({tool_calls:[{index:0,id:'read',type:'function',function:{name:'fs_read',arguments:'{"path":"absent"}'}}]},'tool_calls'));return;
      }
      res.writeHead(503);res.end('private error');
    });
    try {
      const args=['run','synthetic',...f.args,'--fixture-endpoint',gateway.url,'--json'];
      if(mode==='zero_calls'||mode==='one_call') args.push('--max-model-calls',mode==='zero_calls'?'0':'1');
      const result=await exec(binary,args,{env:cleanEnv()}).catch((e:any)=>{assert.equal(e.code,1);return e;});
      const task=JSON.parse(result.stdout);assert.notEqual(task.outcome.status,'completed');
      assert(!result.stdout.includes('private error'));
      assert.equal(evaluations,mode==='zero_calls'?0:1);
      assert.deepEqual(generations,mode==='generation_error'?[models.simple]:mode==='tool_error'?[models.simple,models.simple]:[]);
      assert.equal(task.accounting.model_calls,String(evaluations+generations.length));
    } finally {await gateway.close();await f.close();}
  });
}

for(const mode of ['repair','compaction'] as const) test(`Jev keeps the selected model during ${mode}`,{timeout:15000},async()=>{
  const extra=mode==='repair'?`\n[options.output]\nschema='{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false}'\n[options.output.repair]\nenabled=true\n`:'\n[options.context]\nkeep_recent_turns=0\n';
  const f=await fixture(base+extra);let evaluations=0;let generations=0;let summaries=0;
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());
    if(request.questions){evaluations++;res.end(JSON.stringify(choice('complex')));return;}
    assert.equal(request.model,models.complex);generations++;
    if(mode==='repair') {
      res.setHeader('content-type','text/event-stream');
      res.end(wire({content:generations===1?'invalid':'{"answer":7}'}));return;
    }
    const last=request.messages.at(-1);
    if(last.content?.startsWith('Create a concise handoff')){
      summaries++;res.setHeader('content-type','text/event-stream');res.end(wire({content:'Evidence was read; finish the original task.'}));return;
    }
    if(generations===1){res.setHeader('content-type','text/event-stream');res.end(wire({tool_calls:[{index:0,id:'read',type:'function',function:{name:'fs_read',arguments:'{"path":"evidence.txt"}'}}]},'tool_calls'));return;}
    if(summaries===0){res.writeHead(400,{'content-type':'application/json'});res.end(JSON.stringify({error:{code:'context_length_exceeded'}}));return;}
    res.setHeader('content-type','text/event-stream');res.end(wire({content:'done'}));
  });
  try {
    await writeFile(join(f.cwd,'evidence.txt'),'e'.repeat(6000));
    const task=JSON.parse((await exec(binary,['run','synthetic',...f.args,'--fixture-endpoint',gateway.url,'--json'],{env:cleanEnv()})).stdout);
    assert.equal(task.outcome.status,'completed');assert.equal(evaluations,1);
    assert.equal(generations,mode==='repair'?2:4);
    assert.equal(task.accounting.model_calls,String(generations+1));
    if(mode==='repair') assert.equal(task.output_repair.status,'succeeded');
    else assert.equal(summaries,1);
  } finally {await gateway.close();await f.close();}
});

test('ACP cancellation aborts an in-flight Jev evaluation without selecting a model',{timeout:15000},async()=>{
  const f=await fixture();let calls=0;let received!:()=>void;
  const entered=new Promise<void>(resolve=>{received=resolve;});
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());assert(request.questions);calls++;received();
    await new Promise<void>(resolve=>res.on('close',resolve));
  });
  try {
    await withPablo({binary,args:[...f.args,'--fixture-endpoint',gateway.url],env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});
      const {sessionId}=await cx.request('session/new',{cwd:f.cwd,mcpServers:[]});
      const pending=cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'synthetic'}]});
      await entered;await cx.notify('session/cancel',{sessionId});
      const task=taskOf(await pending);assert.equal(task.outcome.status,'cancelled');assert.equal(task.accounting.model_calls,'1');
    });
    assert.equal(calls,1);
  }finally{await gateway.close();await f.close();}
});
