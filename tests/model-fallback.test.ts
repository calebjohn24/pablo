import { assertExtension } from './fixtures/acp-extension-schema.ts';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { responsesEvents, responsesWire } from './fixtures/open-responses.ts';
import { Ajv2020 } from 'ajv/dist/2020.js';
import { withPablo, taskOf } from '../examples/acp-client.ts';
const ajv = new Ajv2020({strict:false});
const validateMeta = ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/pablo-acp-v2.schema.json',import.meta.url),'utf8')));
const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const base = (await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml', import.meta.url), 'utf8')).replace('max_model_calls=2', 'max_model_calls=4');
const frame = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
const wire = (delta: object, finish: string) => frame(delta, finish) + 'data: [DONE]\n\n';

for (const later of [false, true]) test(`F02 CLI and fresh ACP routes ${later ? 'preserve completed tool history' : 'remain sticky after fallback'}`, { timeout: 15000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-fallback-')));
  const expected = 'fresh route evidence 🌱'; const requests: any[] = [];
  const gateway = await server(async (req, res) => {
    assert.equal(req.headers.authorization, 'Bearer pablo-local-fixture');
    const request = JSON.parse((await body(req)).toString()); requests.push(request);
    const primary = request.model === 'z-ai/glm-5.3-flash'; const tool = request.messages?.at(-1).role === 'tool';
    assert(primary || request.model === 'zai/glm-5.3-flash', 'third entry must stay untouched');
    if (primary && (!later || tool)) { res.writeHead(503); res.end('private provider error must not escape'); return; }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (tool) {
      assert.equal(JSON.parse(request.messages.at(-1).content).filesystem.text, expected);
      res.end(wire({ content: expected }, 'stop'));
    } else res.end(wire({ tool_calls: [{ index: 0, id: 'read_once', type: 'function', function: { name: 'fs_read', arguments: '{"path":"evidence.txt"}' } }] }, 'tool_calls'));
  });
  try {
    const entry = join(cwd, 'entry.toml'); await writeFile(entry, base); await writeFile(join(cwd, 'evidence.txt'), expected);
    const args = ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url];
    const trace = join(cwd, 'cli.jsonl');
    const task = JSON.parse((await exec(binary, ['run', 'read file', ...args, '--trace', trace, '--json'], { env: cleanEnv() })).stdout);
    assert.equal(task.outcome.output, expected); assert.equal(task.accounting.model_calls, '3'); assert.equal(task.accounting.tool_calls, '1');
    const native = await readFile(trace, 'utf8'); assert(!native.includes('private provider error')); assert(!native.includes(expected));
    const events = native.trim().split('\n').map(s => JSON.parse(s)); const starts = events.filter(e => e.type === 'model.started');
    assert.deepEqual(starts.map(e => e.provider), later ? ['openrouter', 'openrouter', 'vercel'] : ['openrouter', 'vercel', 'vercel']);
    assert.equal(new Set(starts.map(e => e.span_id)).size, 3); assert.equal(new Set(starts.map(e => e.run_id)).size, 1);
    assert.equal(events.filter(e => e.type === 'model.finished').length, 3); assert.equal(events.filter(e => e.type === 'tool.started').length, 1);
    assert.equal(events.filter(e => e.type === 'run.finished').length, 1); assert.deepEqual(events.at(-1).accounting, task.accounting);
    if (later) assert.deepEqual(requests[1].messages, requests[2].messages);
    await withPablo({ binary, args, env: cleanEnv() }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } } });
      for (let i = 0; i < 2; i++) {
        const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
        const next = taskOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'read file' }] }));
        assert.deepEqual(next.outcome, task.outcome); assert.deepEqual(next.accounting, task.accounting);
      }
    });
    assert.equal(requests.length, 9);
    for (let i = 0; i < 9; i += 3) assert.deepEqual(requests.slice(i, i + 3).map(r => r.model), later ? ['z-ai/glm-5.3-flash', 'z-ai/glm-5.3-flash', 'zai/glm-5.3-flash'] : ['z-ai/glm-5.3-flash', 'zai/glm-5.3-flash', 'zai/glm-5.3-flash']);
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});

for (const privateOrigin of [true, false]) test(`F02 ${privateOrigin ? 'private Open Responses' : 'gateway assistant'} history stops before incompatible fallback dispatch`, { timeout: 10000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-continuation-fallback-'))); const requests: any[] = [];
  const gateway = await server(async (req, res) => {
    const request = JSON.parse((await body(req)).toString()); requests.push(request);
    assert.equal(request.model, privateOrigin ? 'fixture-text-tools-v1' : 'z-ai/glm-5.3-flash');
    if (requests.length % 2 === 0) { res.writeHead(503); res.end(); return; }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (privateOrigin) res.end(responsesWire(responsesEvents(request, { call: { id: 'read_once', name: 'fs_read', arguments: '{"path":"evidence.txt"}' } })));
    else res.end(wire({ tool_calls: [{ index: 0, id: 'read_once', type: 'function', function: { name: 'fs_read', arguments: '{"path":"evidence.txt"}' } }] }, 'tool_calls'));
  });
  try {
    const entry = join(cwd, 'entry.toml'); await writeFile(entry, base.replace('entries=[{model="primary"},{model="secondary"},{model="third"}]', privateOrigin ? 'entries=[{model="third"},{model="primary"},{model="secondary"}]' : 'entries=[{model="primary"},{model="third"},{model="secondary"}]'));
    await writeFile(join(cwd, 'evidence.txt'), 'private carrier fixture');
    const trace = join(cwd,'cli.jsonl');
    let stdout = '';
    try { await exec(binary, ['run', 'read', '--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url, '--trace', trace, '--json'], { env: cleanEnv() }); assert.fail('expected task failure'); }
    catch (error) { const e = error as any; assert.equal(e.code, 1); stdout = e.stdout; }
    const task = JSON.parse(stdout); assert.equal(task.outcome.code, 'continuation_incompatible'); assert.equal(task.accounting.model_calls, '2'); assert.equal(task.accounting.tool_calls, '1'); assert.equal(requests.length, 2);
    const terminal = JSON.parse((await readFile(trace,'utf8')).trim().split('\n').at(-1)!);
    assert.equal(terminal.model_route.phase,'blocked'); assert.equal(terminal.model_route.entry_index,1);
    assert.equal(terminal.model_route.failure_code,'continuation_incompatible'); assert.equal(terminal.model_route.dispatched,false);
    const attempts: unknown[]=[];
    await withPablo({binary,args:['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url],env:cleanEnv(),onModelAttempt:n=>{assertExtension(n);attempts.push(n);}},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true,'pablo/model-route-v1':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      await assert.rejects(cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'read'}]}),(e:any)=>{
        const details=e.data['pablo/v2']; assert.deepEqual(details.task.outcome,task.outcome); assert.deepEqual(details.model_route,terminal.model_route); return true;
      });
    });
    assert.equal(requests.length,4); assert.equal(attempts.length,4);
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});

const routeLine = 'entries=[{model="primary"},{model="secondary"},{model="third"}]';
const models: Record<string, string> = {primary:'z-ai/glm-5.3-flash',secondary:'zai/glm-5.3-flash',third:'fixture-text-tools-v1'};
const scenarios = [
  ...['primary','secondary','third'].flatMap(first => ['primary','secondary','third'].filter(second => second !== first).map(second => ({name:`${first} to ${second}`, entries:[first,second], mode:'success', count:2, extra:''}))),
  {name:'two entry exhaustion', entries:['third','primary'], mode:'exhaust', count:2, extra:''},
  {name:'three entry exhaustion', entries:['secondary','third','primary'], mode:'exhaust', count:3, extra:''},
  {name:'attempt allowance', entries:['primary','secondary'], mode:'exhaust', count:1, extra:'max_attempts=1'},
  {name:'root call allowance', entries:['primary','third'], mode:'calls', count:1, extra:''},
  {name:'partial stream blocks opt-in', entries:['secondary','primary'], mode:'partial', count:1, extra:'eligible_errors=["transport_uncertain"]\nretry_uncertain_delivery=true'},
  {name:'uncertainty defaults to stop', entries:['primary','secondary'], mode:'uncertain', count:1, extra:''},
  {name:'explicit uncertainty continues', entries:['primary','third'], mode:'uncertain', count:2, extra:'eligible_errors=["transport_uncertain"]\nretry_uncertain_delivery=true'},
];
for (const scenario of scenarios) test(`F03 CLI and negotiated ACP ${scenario.name}`, {timeout:15000}, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-f03-')));
  const requests: string[] = [];
  const gateway = await server(async (req,res) => {
    const request = JSON.parse((await body(req)).toString()); requests.push(request.model);
    const index = scenario.entries.findIndex(e => models[e] === request.model);
    assert(index >= 0, 'only configured entries');
    if (index === 0 && (scenario.mode === 'uncertain' || scenario.mode === 'partial')) {
      if (scenario.mode === 'partial') { res.writeHead(200, {'content-type':'text/event-stream'}); res.write(frame({content:'escaped'})); await new Promise(r => setTimeout(r, 20)); }
      res.destroy(); return;
    }
    if (scenario.mode === 'exhaust' || scenario.mode === 'calls' || index === 0) {
      res.writeHead(index % 2 ? 503 : 429); res.end('PRIVATE_ERROR_BODY'); return;
    }
    res.writeHead(200, {'content-type':'text/event-stream'});
    res.end(request.model === models.third ? responsesWire(responsesEvents(request,{chunks:['done']})) : wire({content:'done'},'stop'));
  });
  try {
    const entry = join(cwd,'entry.toml');
    await writeFile(entry, base.replace(routeLine, `entries=[${scenario.entries.map(e=>`{model="${e}"}`).join(',')}]\n${scenario.extra}`).replace('max_model_calls=4', `max_model_calls=${scenario.mode === 'calls' ? 1 : 4}`));
    const args = ['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const trace = join(cwd,'cli.jsonl');
    const cli = await exec(binary,['run','test route',...args,'--json','--trace',trace],{env:cleanEnv()}).catch((e:any)=>{assert.equal(e.code,1); return e;});
    const task = JSON.parse(cli.stdout);
    assert.equal(task.accounting.model_calls, String(scenario.count));
    const native = await readFile(trace,'utf8'); assert(!native.includes('PRIVATE_ERROR_BODY'));
    const events = native.trim().split('\n').map(s=>JSON.parse(s));
    const finishes = events.filter(e=>e.type==='model.finished');
    assert.equal(finishes.length,scenario.count);
    assert.equal(events.filter(e=>e.type==='run.finished').length,1);
    for (const [i,e] of finishes.entries()) {
      assert.equal(e.model_route.entry,scenario.entries[i]); assert.equal(e.model_route.entry_index,i);
      assert.equal(e.model_route.operation,'1'); assert.equal(e.model_route.attempt,i+1);
      assert.equal(e.model_route.accounting.model_calls,String(i+1)); assert.equal(e.model_route.dispatched,true);
    }
    if (scenario.mode === 'calls') { assert.equal(events.at(-1).model_route.phase,'blocked'); assert.equal(events.at(-1).model_route.entry,scenario.entries[1]); }
    for (const negotiated of [true,false]) {
      const attempts: any[] = []; const updates: any[] = [];
      await withPablo({binary,args,env:cleanEnv(),onModelAttempt:n=>{assertExtension(n);attempts.push(n);},onUpdate:n=>{updates.push(n);}}, async cx=>{
        const init = await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true,'pablo/model-route-v1':negotiated}}});
        assert.equal(init.agentCapabilities?._meta?.['pablo/model-route-v1'],true);
        const {sessionId} = await cx.request('session/new',{cwd,mcpServers:[]});
        let details: any;
        try { details=(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'test route'}]}))._meta?.['pablo/v2']; }
        catch(e:any) { details=e.data?.['pablo/v2']; assert(details, String(e)); }
        assert(validateMeta(details),ajv.errorsText(validateMeta.errors));
        assert.deepEqual(details.task.outcome,task.outcome); assert.deepEqual(details.task.accounting,task.accounting);
        assert.equal(attempts.length, negotiated ? 2*scenario.count : 0);
        if (negotiated) {
          assert.deepEqual(details.model_route,events.at(-1).model_route);
          const closed=attempts.filter(n=>n.type==='model.finished');
          assert.deepEqual(closed.map(n=>n['pablo/v2'].model_route),finishes.map(e=>e.model_route));
          assert.equal(new Set(closed.map(n=>n['pablo/v2'].span_id)).size,scenario.count);
          assert(attempts.every(n=>n.sessionId===sessionId));
          for(const n of attempts) assert(validateMeta(n['pablo/v2']),ajv.errorsText(validateMeta.errors));
        } else assert.equal(details.model_route,undefined);
        assert(updates.every(n=>!['agent_thought_chunk','tool_call'].includes(n.update.sessionUpdate)), 'model routing is not projected as tools or thoughts');
      });
    }
    assert.deepEqual(requests, Array.from({length:3},()=>scenario.entries.slice(0,scenario.count).map(e=>models[e])).flat());
  } finally { await gateway.close(); await rm(cwd,{recursive:true,force:true}); }
});

test('F03 later missing credentials reject CLI and ACP before run admission', {timeout:10000}, async()=>{
  const cwd = await realpath(await mkdtemp(join(tmpdir(),'pablo-f03-credential-')));
  try {
    const entry=join(cwd,'entry.toml'); await writeFile(entry,base);
    const env={...cleanEnv(),ROUTE_ROUTER_KEY:'synthetic-valid-first-entry'};
    const args=['--config',entry,'--bind',`workspace=${cwd}`];
    const result=await exec(binary,['run','unadmitted',...args,'--json'],{env,cwd}).catch((e:any)=>{assert.equal(e.code,2); return e;});
    assert.match(result.stderr,/config_credential_missing/); assert(!result.stdout.includes(env.ROUTE_ROUTER_KEY));
    const diagnostics: string[]=[]; const notifications: unknown[]=[];
    await assert.rejects(withPablo({binary,args,env,onDiagnostic:s=>{diagnostics.push(s);},onModelAttempt:n=>{assertExtension(n);notifications.push(n);}},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true,'pablo/model-route-v1':true}}});
      assert.fail('credentials must fail before ACP startup');
    }), /ACP connection closed/);
    assert.match(diagnostics.join(''),/config_credential_missing/);
    assert.equal(notifications.length,0); assert(!diagnostics.join('').includes(env.ROUTE_ROUTER_KEY));
  } finally {await rm(cwd,{recursive:true,force:true});}
});
