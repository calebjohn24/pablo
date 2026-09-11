import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { otlpSpans } from './fixtures/otlp-spans.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const base = (await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml', import.meta.url), 'utf8'))
  .replace('max_model_calls=2', 'max_model_calls=20').replace('max_tool_calls=1', 'max_tool_calls=10');
const wire = (delta: object, finish = 'stop') => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\ndata: [DONE]\n\n`;

test('A02 configured CLI and ACP supervise a child with tools and one attributed root stream', { timeout: 20000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-children-')));
  const exports: Buffer[] = [];
  const native: any[] = [];
  const collector = await server(async (req,res) => { exports.push(await body(req)); res.end(); });
  const gateway = await server(async (req, res) => {
    const r = JSON.parse((await body(req)).toString());
    const child = r.model === 'zai/glm-5.3-flash';
    const results = r.messages.filter((m: any) => m.role === 'tool').map((m: any) => JSON.parse(m.content));
    const call = (name: string, args: object) => wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name,arguments:JSON.stringify(args)}}]}, 'tool_calls');
    res.writeHead(200, {'content-type':'text/event-stream'});
    if (child) {
      assert(!JSON.stringify(r.messages).includes('root private task'));
      assert(!r.tools.some((t: any) => t.function.name === 'subagent'));
      if (!results.length) res.end(call('fs_read', {path:'evidence.txt'}));
      else { assert.equal(results[0].filesystem.text, 'child private file'); res.end(wire({content:'child done'})); }
      return;
    }
    assert.equal(r.model, 'z-ai/glm-5.3-flash');
    assert(!JSON.stringify(r.messages).includes('child private file'));
    assert(r.tools.some((t: any) => t.function.name === 'subagent'));
    if (!results.length) res.end(call('subagent', {action:'spawn',request:{input:'read selected evidence',capabilities:{model_route:['secondary'],tools:['fs.read']}}}));
    else if (results.length === 1) {
      assert.equal(results[0].subagent.agent.state, 'queued');
      res.end(call('subagent', {action:'inspect',agent_id:results[0].subagent.agent.agent_id}));
    } else if (results.length === 2) res.end(call('subagent', {action:'wait',agent_ids:[results[0].subagent.agent.agent_id],mode:'all',timeout_ms:5000}));
    else {
      assert.deepEqual(results[2].subagent.remaining, []);
      assert.equal(results[2].subagent.settled[0].outcome.output, 'child done');
      res.end(wire({content:'root done'}));
    }
  });
  try {
    const entry = join(cwd, 'entry.toml');
    await writeFile(entry, base + `[options.otel]\nexporter="otlp"\nendpoint="${collector.url}/v1/traces"\n` + '\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=true\npath={base="workspace",path="trace-{session_id}.jsonl"}\n');
    await writeFile(join(cwd, 'evidence.txt'), 'child private file');
    const args = ['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const cli = JSON.parse((await exec(binary, ['run','root private task',...args,'--json'], {env:cleanEnv()})).stdout);
    assert.equal(cli.outcome.output, 'root done');
    assert.equal(cli.accounting.model_calls, '6'); assert.equal(cli.accounting.tool_calls, '4');
    const verify = async (session: string) => {
      const events = (await readFile(join(cwd, `trace-${session}.jsonl`),'utf8')).trim().split('\n').map(s => JSON.parse(s));
      native.push(...events);
      assert.deepEqual(events.map(e => e.root_seq), events.map((_,i) => i+1));
      const roots = events.filter(e => e.agent.depth === 0), children = events.filter(e => e.agent.depth === 1);
      assert(children.some(e => e.type === 'tool.started'));
      assert.equal(new Set(children.map(e => e.run_id)).size, 1);
      assert.deepEqual(children.map(e => e.seq), children.map((_,i) => i+1));
      assert.equal(events.at(-1).type, 'run.finished'); assert.equal(events.at(-1).agent.depth, 0);
      assert.equal(children.at(-1).type, 'run.finished');
      assert.equal(children[0].agent.parent_agent_id, roots[0].agent.agent_id);
      assert.equal(children[0].agent.root_run_id, roots[0].run_id);
      assert.equal(children[0].trace_id, roots[0].trace_id);
      assert(roots.some(e => e.type === 'tool.started' && e.span_id === children[0].parent_span_id));
      return children;
    };
    await verify(cli.session_id);
    const updates: any[] = [];
    await withPablo({binary,args,env:cleanEnv(),onUpdate:n => { updates.push(n); }}, async cx => {
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
      const {sessionId} = await cx.request('session/new',{cwd,mcpServers:[]});
      const result = taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'root private task'}]}));
      assert.equal(result.outcome.status, 'completed');
      assert(result.outcome.status === 'completed');
      assert.equal(result.outcome.output, 'root done');
      assert.equal(result.accounting.model_calls, '6');
      const child = await verify(sessionId);
      assert(updates.length > 0);
      const starts = updates.filter(n => n.update.sessionUpdate === 'tool_call');
      assert.equal(new Set(starts.map(n => n.update.toolCallId)).size, 4);
      for (const n of starts) {
        assert(n.update.toolCallId.startsWith(n._meta['pablo/v1'].agent.agent_id + '/'));
        assert(updates.some(done => done.update.sessionUpdate === 'tool_call_update' && done.update.toolCallId === n.update.toolCallId));
      }
      assert(updates.every(n => n.sessionId === sessionId));
      assert(updates.some(n => n._meta?.['pablo/v1']?.agent?.agent_id === child[0].agent.agent_id));
    });
    const spans = exports.flatMap(otlpSpans);
    assert(spans.length > 0);
    for (const update of updates) {
      const identity=update._meta['pablo/v1'];
      assert(native.some(e=>e.run_id===identity.run_id && e.seq===identity.seq_end && e.root_seq===identity.root_seq && e.span_id===identity.span_id));
      assert(spans.some(span=>span.trace_id===identity.trace_id && span.span_id===identity.span_id));
    }
    for (const event of native.filter(e => ['run.started','model.started','tool.started'].includes(e.type))) {
      const matches = spans.filter(span => span.trace_id === event.trace_id && span.span_id === event.span_id);
      assert.equal(matches.length, 1, event.type);
      assert.equal(matches[0].parent_span_id, event.parent_span_id ?? '');
      assert.equal(matches[0].attributes['pablo.agent.id'], event.agent.agent_id);
      assert.equal(matches[0].attributes['pablo.root.run.id'], event.agent.root_run_id);
    }
    for (const payload of exports) assert(!payload.includes(Buffer.from('child private file')));
  } finally { await gateway.close(); await collector.close(); await rm(cwd,{recursive:true,force:true}); }
});

test('A02 configured stop joins an active child before the root completes', { timeout: 15000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-child-stop-')));
  let started = false, closed = false;
  const gateway = await server(async (req, res) => {
    const r = JSON.parse((await body(req)).toString());
    res.writeHead(200, {'content-type':'text/event-stream'});
    if (r.model === 'zai/glm-5.3-flash') {
      started = true;
      res.on('close', () => { closed = true; });
      res.write(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'working'},finish_reason:null}]})}\n\n`);
      return;
    }
    const results = r.messages.filter((m: any) => m.role === 'tool').map((m: any) => JSON.parse(m.content));
    const call = (args: object) => wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]},'tool_calls');
    if (!results.length) res.end(call({action:'spawn',request:{input:'wait for cancellation',capabilities:{model_route:['secondary'],tools:[]}}}));
    else if (results.length === 1) res.end(call({action:'wait',agent_ids:[results[0].subagent.agent.agent_id],mode:'all',timeout_ms:100}));
    else if (results.length === 2) {
      assert(started); assert.equal(results[1].subagent.remaining.length, 1);
      res.end(call({action:'stop',agent_id:results[0].subagent.agent.agent_id}));
    } else {
      assert.equal(results[2].subagent.snapshot.outcome.status, 'cancelled');
      assert.equal(results[2].subagent.snapshot.agent.state, 'settled');
      res.end(wire({content:'stopped and joined'}));
    }
  });
  try {
    const entry = join(cwd,'entry.toml');
    await writeFile(entry,base+'\n[options.children]\nenabled=true\n');
    const trace = join(cwd,'trace.jsonl');
    const result = JSON.parse((await exec(binary,['run','stop child','--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--trace',trace,'--json'],{env:cleanEnv()})).stdout);
    assert.equal(result.outcome.output, 'stopped and joined'); assert(closed);
    const events = (await readFile(trace,'utf8')).trim().split('\n').map(s=>JSON.parse(s));
    const terminals = events.filter(e=>e.type==='run.finished');
    assert.deepEqual(terminals.map(e=>e.agent.depth),[1,0]);
    assert.equal(terminals[0].outcome.status, 'cancelled');
  } finally { await gateway.close(); await rm(cwd,{recursive:true,force:true}); }
});

for (const mode of ['cancel','deadline','complete'] as const) test(`A02 configured ACP root ${mode} joins its streaming child`, {timeout:15000}, async () => {
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-child-root-')));
  let started!:()=>void; const childStarted=new Promise<void>(resolve=>{started=resolve;});
  let closed=false;
  const gateway=await server(async(req,res)=>{
    const r=JSON.parse((await body(req)).toString());
    res.writeHead(200,{'content-type':'text/event-stream'});
    if(r.model==='zai/glm-5.3-flash'){
      res.on('close',()=>{closed=true;});
      res.write(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'working'},finish_reason:null}]})}\n\n`);
      started();return;
    }
    const results=r.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
    if(results.length){
      await childStarted;
      if(mode==='complete'){res.end(wire({content:'root finished'}));return;}
    }
    const args=results.length?{action:'wait',agent_ids:[results[0].subagent.agent.agent_id],mode:'all',timeout_ms:5000}:{action:'spawn',request:{input:'stream until cancelled',capabilities:{model_route:['secondary'],tools:[]}}};
    res.end(wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]},'tool_calls'));
  });
  try{
    const entry=join(cwd,'entry.toml');
    await writeFile(entry,base.replace('max_tool_calls=10',`max_tool_calls=10\nmax_run_duration_ms=${mode==='deadline'?700:10000}`)+'\n[options.children]\nenabled=true\n[options.trace]\npath={base="workspace",path="trace-{session_id}.jsonl"}\n');
    let sent=false;
    await withPablo({binary,args:['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url],env:cleanEnv(),onUpdate:async(n,cx)=>{
      if(mode==='cancel'&&!sent&&(n._meta?.['pablo/v1'] as any)?.agent?.depth===1&&n.update.sessionUpdate==='agent_message_chunk'){
        sent=true;await cx.notify('session/cancel',{sessionId:n.sessionId});
      }
    }},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      const result=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'exercise root completion'}]})
        .then(taskOf).catch((error:any)=>{assert.equal(mode,'deadline');assert.equal(error.code,-32603);return error.data['pablo/v1'].task;});
      assert.equal(result.outcome.status,mode==='cancel'?'cancelled':mode==='deadline'?'timed_out':'completed');
      assert(closed);
      const events=(await readFile(join(cwd,`trace-${sessionId}.jsonl`),'utf8')).trim().split('\n').map(s=>JSON.parse(s));
      const terminals=events.filter(e=>e.type==='run.finished');
      assert.deepEqual(terminals.map(e=>e.agent.depth),[1,0]);
      assert(['cancelled','timed_out'].includes(terminals[0].outcome.status));
      assert.equal(terminals[1].outcome.status,result.outcome.status);
      assert.deepEqual(events.map(e=>e.root_seq),events.map((_,i)=>i+1));
    });
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});


const fixtureRoot=fileURLToPath(new URL('../',import.meta.url));
const fixturePython=join(fixtureRoot,'.pablo/mcp-fixture-venv/bin/python');
test('A02 changed child MCP catalog fails startup without a provider call and joins both processes', {timeout:15000,skip:!existsSync(fixturePython)}, async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-child-startup-')));
  let rootPid=0,childPid=0;
  const gateway=await server(async(req,res)=>{
    const r=JSON.parse((await body(req)).toString());
    assert.equal(r.model,'z-ai/glm-5.3-flash','child setup failure must precede provider dispatch');
    const results=r.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
    res.writeHead(200,{'content-type':'text/event-stream'});
    if(!results.length){
      rootPid=Number(await readFile(join(cwd,'pid'),'utf8'));
      await writeFile(join(cwd,'catalog-version'),'changed');
    }
    if(results.length===2){
      const snapshot=results[1].subagent.settled[0];
      assert.equal(snapshot.agent.state,'settled');
      assert.deepEqual(snapshot.outcome,{status:'failed',code:'child_admission',delivery:'not_sent'});
      childPid=Number(await readFile(join(cwd,'pid'),'utf8'));assert.notEqual(childPid,rootPid);
      assert.throws(()=>process.kill(childPid,0),(e:any)=>e.code==='ESRCH');
      res.end(wire({content:'startup failure observed'}));return;
    }
    const args=results.length?{action:'wait',agent_ids:[results[0].subagent.agent.agent_id],mode:'all',timeout_ms:5000}:{action:'spawn',request:{input:'selected MCP task',capabilities:{model_route:['secondary'],tools:['mcp/local/read'],mcp_servers:['local']}}};
    res.end(wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]},'tool_calls'));
  });
  try{
    await writeFile(join(cwd,'catalog-version'),'original');
    const entry=join(cwd,'entry.toml');
    await writeFile(entry,base+`\n[options.children]\nenabled=true\n[options.mcp.servers.local]\ntransport="stdio"\ncommand=${JSON.stringify(fixturePython)}\nargs=[${JSON.stringify(join(fixtureRoot,'tests/fixtures/mcp/adversarial.py'))},"child_catalog"]\nrequired=true\n`);
    const result=JSON.parse((await exec(binary,['run','observe failed child','--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--json'],{env:cleanEnv()})).stdout);
    assert.equal(result.outcome.output,'startup failure observed');
    assert.equal(result.accounting.model_calls,'3');
    assert.throws(()=>process.kill(rootPid,0),(e:any)=>e.code==='ESRCH');
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});


test('A02 configured trace setup failure precedes root MCP startup and provider dispatch', {timeout:10000,skip:!existsSync(fixturePython)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-child-trace-failure-')));
  let requests=0;
  const gateway=await server((_req,res)=>{requests++;res.end();});
  try{
    const entry=join(cwd,'entry.toml'),trace=join(cwd,'trace.jsonl');
    await writeFile(trace,'preserve existing trace');
    await writeFile(entry,base+`\n[options.children]\nenabled=true\n[options.mcp.servers.local]\ntransport="stdio"\ncommand=${JSON.stringify(fixturePython)}\nargs=[${JSON.stringify(join(fixtureRoot,'tests/fixtures/mcp/adversarial.py'))},"startup_hang"]\nrequired=true\n`);
    await assert.rejects(exec(binary,['run','trace failure','--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--trace',trace,'--json'],{env:cleanEnv()}),(e:any)=>{
      assert.equal(e.code,2);assert.equal(e.stdout,'');assert.match(e.stderr,/config_.*trace/);return true;
    });
    assert.equal(requests,0);assert(!existsSync(join(cwd,'pid')));
    assert.equal(await readFile(trace,'utf8'),'preserve existing trace');
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
