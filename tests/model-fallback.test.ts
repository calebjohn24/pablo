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
import { withPablo, taskOf } from '../examples/acp-client.ts';
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
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true, 'pablo/task-v1': true } } });
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
    if (requests.length === 2) { res.writeHead(503); res.end(); return; }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (privateOrigin) res.end(responsesWire(responsesEvents(request, { call: { id: 'read_once', name: 'fs_read', arguments: '{"path":"evidence.txt"}' } })));
    else res.end(wire({ tool_calls: [{ index: 0, id: 'read_once', type: 'function', function: { name: 'fs_read', arguments: '{"path":"evidence.txt"}' } }] }, 'tool_calls'));
  });
  try {
    const entry = join(cwd, 'entry.toml'); await writeFile(entry, base.replace('entries=[{model="primary"},{model="secondary"},{model="third"}]', privateOrigin ? 'entries=[{model="third"},{model="primary"},{model="secondary"}]' : 'entries=[{model="primary"},{model="third"},{model="secondary"}]'));
    await writeFile(join(cwd, 'evidence.txt'), 'private carrier fixture');
    let stdout = '';
    try { await exec(binary, ['run', 'read', '--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url, '--json'], { env: cleanEnv() }); assert.fail('expected task failure'); }
    catch (error) { const e = error as any; assert.equal(e.code, 1); stdout = e.stdout; }
    const task = JSON.parse(stdout); assert.equal(task.outcome.code, 'continuation_incompatible'); assert.equal(task.accounting.model_calls, '2'); assert.equal(task.accounting.tool_calls, '1'); assert.equal(requests.length, 2);
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});
