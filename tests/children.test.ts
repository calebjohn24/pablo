import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const base = (await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml', import.meta.url), 'utf8'))
  .replace('max_model_calls=2', 'max_model_calls=20').replace('max_tool_calls=1', 'max_tool_calls=10');
const wire = (delta: object, finish = 'stop') => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\ndata: [DONE]\n\n`;

test('A02 configured CLI and ACP supervise a child with tools and one attributed root stream', { timeout: 20000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-children-')));
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
    await writeFile(entry, base + '\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=true\npath={base="workspace",path="trace-{session_id}.jsonl"}\n');
    await writeFile(join(cwd, 'evidence.txt'), 'child private file');
    const args = ['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const cli = JSON.parse((await exec(binary, ['run','root private task',...args,'--json'], {env:cleanEnv()})).stdout);
    assert.equal(cli.outcome.output, 'root done');
    assert.equal(cli.accounting.model_calls, '6'); assert.equal(cli.accounting.tool_calls, '4');
    const verify = async (session: string) => {
      const events = (await readFile(join(cwd, `trace-${session}.jsonl`),'utf8')).trim().split('\n').map(s => JSON.parse(s));
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
  } finally { await gateway.close(); await rm(cwd,{recursive:true,force:true}); }
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
