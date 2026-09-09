import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test, type TestContext } from 'node:test';
import { createServer, type ServerResponse } from 'node:http';
import { mkdtemp, mkdir, writeFile, readFile, rm, realpath, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { Ajv2020 } from 'ajv/dist/2020.js';
import { RequestError, type ClientContext, type SessionNotification } from '@agentclientprotocol/sdk';
import { withPablo, outcomeOf, taskOf } from '../examples/acp-client.ts';

const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const sdkSchema = JSON.parse(await readFile(new URL('../node_modules/@agentclientprotocol/sdk/schema/schema.json', import.meta.url), 'utf8'));
const ajv = new Ajv2020({ strict: false, validateFormats: false });
ajv.addSchema(sdkSchema, 'acp');
const extensionCheck = ajv.compile(JSON.parse(await readFile(new URL('../docs/pablo-acp-v1.schema.json', import.meta.url), 'utf8')));
function validateExtension(value: unknown) { assert.ok(extensionCheck(value), ajv.errorsText(extensionCheck.errors)); }
const validate = (name: string, value: unknown) => {
  const check = ajv.compile({ $ref: `acp#/$defs/${name}` });
  assert.ok(check(value), `${name}: ${ajv.errorsText(check.errors)}`);
};
const frame = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\r\n\r\n`;
const end = (reason = 'stop') => frame({}, reason) + 'data: [DONE]\n\n';
const tool = (command: string, id = 'call_1') => {
  const args = JSON.stringify({ command, cwd: '.' });
  let body = frame({ tool_calls: [{ index: 0, id, type: 'function', function: { name: 'shell_run', arguments: '' } }] });
  for (let i = 0; i < args.length; i += 3) body += frame({ tool_calls: [{ index: 0, function: { arguments: args.slice(i, i + 3) } }] });
  return body + end('tool_calls');
};
async function sse(res: ServerResponse, body: string) {
  res.writeHead(200, { 'content-type': 'text/event-stream' });
  const bytes = Buffer.from(body);
  for (let i = 0; i < bytes.length; i += 97) {
    if (res.destroyed) return;
    if (!res.write(bytes.subarray(i, i + 97))) await once(res, 'drain');
  }
  res.end();
}
async function fixture(t: TestContext, handler: (body: any, res: ServerResponse, call: number) => Promise<void>) {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-acp-')));
  const errors: unknown[] = []; const requests: any[] = []; const connections = new Set();
  const server = createServer(async (req, res) => {
    try {
      connections.add(req.socket);
      const chunks: Buffer[] = [];
      for await (const chunk of req) chunks.push(chunk);
      const body = JSON.parse(Buffer.concat(chunks).toString());
      assert.equal(req.headers.authorization, 'Bearer pablo-local-fixture');
      requests.push(body);
      await handler(body, res, requests.length);
    } catch (error) { errors.push(error); res.destroy(); }
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); assert.ok(address && typeof address !== 'string');
  const env = { ...process.env, AI_GATEWAY_API_KEY: 'synthetic-secret', PABLO_FIXTURE_ENDPOINT: `http://127.0.0.1:${address.port}/v1/chat/completions` };
  t.after(async () => { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); await rm(cwd, { recursive: true, force: true }); assert.deepEqual(errors, []); });
  return { cwd, env, requests, connections };
}
async function setup(cx: ClientContext, cwd: string, extended = true) {
  const init = await cx.request('initialize', { protocolVersion: 1, clientCapabilities: extended ? { _meta: { 'pablo/v1': true } } : {} });
  validate('InitializeResponse', init);
  assert.equal(init.protocolVersion, 1);
  assert.equal(init.agentCapabilities?.loadSession, false);
  assert.deepEqual(init.agentCapabilities?.promptCapabilities, { image: false, audio: false, embeddedContext: false });
  assert.deepEqual(init.agentCapabilities?.sessionCapabilities, {});
  assert.deepEqual(init.authMethods, []);
  const session = await cx.request('session/new', { cwd, mcpServers: [] });
  validate('NewSessionResponse', session);
  return session.sessionId;
}
const prompt = (cx: ClientContext, sessionId: string) => cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Read the synthetic evidence.' }] });
async function trace(path: string): Promise<any[]> { return (await readFile(path, 'utf8')).trim().split('\n').map(line => JSON.parse(line)); }
function gone(pid: number) { assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' }); }
async function waitUntil(condition: () => Promise<boolean> | boolean, timeout = 4000) {
  const deadline = Date.now() + timeout;
  while (!(await condition())) { if (Date.now() > deadline) throw new Error('fixture condition timed out'); await delay(10); }
}

test('official TypeScript client runs model/shell/model; ordered v1 updates match the native trace', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (body, res, call) => {
    if (call === 1) {
      assert.equal(body.model, 'fixture/acp');
      assert.equal(body.tools[0].function.name, 'shell_run');
      await sse(res, frame({ content: 'Inspecting evidence. ' }) + tool('cat evidence.txt'));
    } else {
      const result = JSON.parse(body.messages.at(-1).content);
      assert.equal(result.shell.stdout, 'violet-319\n');
      assert.equal(result.shell.exit_code, 0);
      assert.deepEqual(body.tools, f.requests[0].tools);
      assert.deepEqual(body.messages[0], f.requests[0].messages[0]);
      await sse(res, frame({ content: 'Evidence: ' }) + frame({ content: 'violet-319 🟣' }) + end());
    }
  });
  await writeFile(join(f.cwd, 'evidence.txt'), 'violet-319\n');
  const updates: SessionNotification[] = []; let raw = ''; let stderr = '';
  const path = join(f.cwd, 'trace.jsonl');
  const response = await withPablo({ binary, env: f.env, args: ['--model', 'fixture/acp', '--trace', path],
    onSpawn: c => c.stdout.on('data', data => { raw += data; }),
    onDiagnostic: text => { stderr += text; },
    onUpdate: notification => { validate('SessionNotification', notification); updates.push(notification); },
  }, async cx => {
    const sessionId = await setup(cx, f.cwd);
    const response = await prompt(cx, sessionId);
    assert.equal(updates.at(-1)?.update.sessionUpdate, 'agent_message_chunk');
    validate('PromptResponse', response);
    validateExtension(response._meta?.['pablo/v1']);
    return response;
  });
  assert.equal(response.stopReason, 'end_turn');
  const outcome = outcomeOf(response); assert.equal(outcome.status, 'completed');
  if (outcome.status === 'completed') { assert.equal(outcome.output, 'Evidence: violet-319 🟣'); assert.equal(outcome.usage.input_tokens, null); }
  assert.deepEqual(updates.flatMap(({ update }) => update.sessionUpdate === 'tool_call' || update.sessionUpdate === 'tool_call_update' ? [update.status ?? 'pending'] : []), ['pending', 'in_progress', 'completed']);
  const records = await trace(path); let last = 0;
  for (const n of updates) {
    const metadata = n._meta?.['pablo/v1'] as any;
    validateExtension(metadata);
    assert.ok(metadata.seq_start > last); assert.ok(metadata.seq_end >= metadata.seq_start); last = metadata.seq_end;
    const native = records.find(e => e.seq === metadata.seq_end);
    for (const key of ['run_id', 'session_id', 'trace_id', 'span_id', 'trace_flags']) assert.equal(metadata[key], native[key]);
  }
  assert.equal(records.at(-1).outcome.status, 'completed');
  assert.equal((response._meta?.['pablo/v1'] as any).trace_id, records.at(-1).trace_id);
  const messages = raw.trim().split('\n').map(line => JSON.parse(line));
  assert.ok(messages.every(m => m.jsonrpc === '2.0'));
  assert.equal(messages.at(-1).result.stopReason, 'end_turn');
  assert.equal(messages.filter(m => m.result?.stopReason).length, 1);
  assert.ok(!(await readFile(path, 'utf8')).includes('violet-319'));
  assert.ok(!raw.includes('synthetic-secret')); assert.ok(!stderr.includes('synthetic-secret'));
  assert.equal(f.requests.length, 2);
});

test('generic v1 peers need no Pablo metadata; unsupported methods and content are explicit', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (_body, res) => { await sse(res, frame({ content: 'plain text' }) + end()); });
  await withPablo({ env: f.env, onUpdate: n => assert.equal(n._meta, undefined) }, async cx => {
    await assert.rejects(cx.request('session/new', { cwd: f.cwd, mcpServers: [] }), (e: any) => e instanceof RequestError);
    const id = await setup(cx, f.cwd, false);
    for (const [method, params] of [['session/load', { sessionId: id, cwd: f.cwd, mcpServers: [] }], ['session/set_mode', { sessionId: id, modeId: 'fake' }], ['_unknown', {}]] as const) {
      await assert.rejects(cx.request(method, params), (e: any) => e.code === -32601);
    }
    await assert.rejects(cx.request('session/prompt', { sessionId: id, prompt: [{ type: 'image', mimeType: 'image/png', data: '' }] }), (e: any) => e.code === -32602);
    await assert.rejects(prompt(cx, 'unknown'), (e: any) => e.code === -32602);
    const response = await cx.request('session/prompt', { sessionId: id, prompt: [{ type: 'resource_link', uri: 'file:///unfetched.txt', name: 'reference' }, { type: 'text', text: 'Describe the reference' }] });
    assert.equal(response.stopReason, 'end_turn'); assert.equal(response._meta, undefined);
    await assert.rejects(prompt(cx, id), (e: any) => e.code === -32602);
    const next = await cx.request('session/new', { cwd: f.cwd, mcpServers: [] });
    assert.notEqual(next.sessionId, id);
    await assert.rejects(prompt(cx, id), (e: any) => e.code === -32602);
  });
  assert.equal(f.requests.length, 1);
  assert.ok(f.requests[0].messages[1].content.includes('file:///unfetched.txt'));
});

test('session cancellation delivers terminal tool updates and responds only after shell cleanup', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (_body, res) => { await sse(res, tool('exec sleep 30')); });
  const path = join(f.cwd, 'trace.jsonl'); const updates: SessionNotification[] = [];
  await withPablo({ env: f.env, args: ['--trace', path], onUpdate: async (n, cx) => {
    updates.push(n);
    if (n.update.sessionUpdate === 'tool_call_update' && n.update.status === 'in_progress') await cx.notify('session/cancel', { sessionId: n.sessionId });
  } }, async cx => {
    const id = await setup(cx, f.cwd);
    const response = await prompt(cx, id);
    assert.equal(response.stopReason, 'cancelled'); assert.equal(outcomeOf(response).status, 'cancelled');
    assert.equal(updates.at(-1)?.update.sessionUpdate, 'tool_call_update');
    const records = await trace(path);
    const started = records.find(e => e.type === 'shell.started'); assert.ok(started); gone(started.process_id);
    assert.equal(records.at(-1).outcome.status, 'cancelled');
  });
  assert.equal(f.requests.length, 1);
});

test('provider errors and deadlines preserve typed terminal failure without false end_turn', { timeout: 15000 }, async t => {
  for (const scenario of ['malformed', 'deadline'] as const) {
    await t.test(scenario, async t => {
      const f = await fixture(t, async (_body, res) => {
        if (scenario === 'malformed') await sse(res, 'data: {invalid}\n\n');
        else { res.writeHead(200, { 'content-type': 'text/event-stream' }); res.flushHeaders(); }
      });
      await withPablo({ env: f.env, args: ['--timeout', '1'] }, async cx => {
        const id = await setup(cx, f.cwd);
        await assert.rejects(prompt(cx, id), (error: any) => {
          assert.equal(error.code, -32603);
          validateExtension(error.data['pablo/v1']);
          assert.equal(error.data['pablo/v1'].outcome.status, scenario === 'deadline' ? 'timed_out' : 'failed');
          return true;
        });
      });
    });
  }
});

test('gateway rejection bodies and streamed errors stay private across ACP and native traces', { timeout: 20000 }, async t => {
  for (const scenario of [401, 429, 500, 'sse-error', 'eof'] as const) await t.test(String(scenario), async t => {
    const sensitive = 'synthetic-provider-body-do-not-serialize';
    const f = await fixture(t, async (_body, res) => {
      if (typeof scenario === 'number') {
        res.writeHead(scenario, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ error: { message: sensitive } }));
      } else if (scenario === 'sse-error') {
        await sse(res, `data: ${JSON.stringify({ error: { message: sensitive } })}\n\n`);
      } else {
        // A finish chunk is insufficient: missing [DONE] must remain a failure.
        await sse(res, frame({}, 'stop'));
      }
    });
    const path = join(f.cwd, 'trace.jsonl'); let raw = ''; let stderr = '';
    await withPablo({ env: f.env, args: ['--trace', path, '--capture-content'],
      onSpawn: c => c.stdout.on('data', data => { raw += data; }),
      onDiagnostic: text => { stderr += text; },
    }, async cx => {
      const id = await setup(cx, f.cwd);
      await assert.rejects(prompt(cx, id), (error: any) => {
        assert(error instanceof RequestError);
        assert.equal(error.code, -32603);
        const meta = (error.data as any)['pablo/v1'];
        validateExtension(meta);
        assert.deepEqual(meta.outcome, {
          status: 'failed', code: scenario === 'eof' ? 'malformed_stream' : 'provider_rejected',
          delivery: 'response_received',
        });
        return true;
      });
    });
    const records = await trace(path);
    assert.equal(records.filter(e => e.type === 'run.finished').length, 1);
    assert.equal(records.at(-1).outcome.status, 'failed');
    assert.equal(records.filter(e => e.type === 'shell.started').length, 0);
    assert.equal(f.requests.length, 1, 'no automatic retry');
    const serialized = raw + stderr + await readFile(path, 'utf8');
    for (const secret of [sensitive, 'synthetic-secret', 'pablo-local-fixture']) assert(!serialized.includes(secret));
    assert(raw.trim().split('\n').every(line => JSON.parse(line).jsonrpc === '2.0'));
  });
});

test('ACP live-shaped usage chunks accumulate reported counters and preserve unknown cache usage', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (body, res, call) => {
    assert.deepEqual(body.stream_options, { include_usage: true });
    let response: string;
    if (call === 1) response = tool('cat evidence.txt').replace('data: [DONE]\n\n', '');
    else {
      assert.equal(JSON.parse(body.messages.at(-1).content).shell.stdout, 'usage-evidence\n');
      response = frame({ content: 'usage-evidence' }) + frame({}, 'stop');
    }
    await sse(res, response + `data: ${JSON.stringify({ choices: [], usage: {
      prompt_tokens: call * 100, completion_tokens: call * 10,
      prompt_tokens_details: { cached_tokens: call * 5 },
    } })}\n\ndata: [DONE]\n\n`);
  });
  await writeFile(join(f.cwd, 'evidence.txt'), 'usage-evidence\n');
  const path = join(f.cwd, 'trace.jsonl');
  const response = await withPablo({ env: f.env, args: ['--trace', path] }, async cx => prompt(cx, await setup(cx, f.cwd)));
  const outcome = outcomeOf(response); assert(outcome.status === 'completed');
  assert.deepEqual(outcome.usage, { input_tokens: 300, output_tokens: 30, cache_read_input_tokens: 15, cache_write_input_tokens: null });
  const records = await trace(path);
  assert.equal(records.filter(e => e.type === 'model.finished').length, 2);
  assert.deepEqual(records.at(-1).outcome.usage, outcome.usage);
  assert.equal(f.requests.length, 2);
});

class RawPeer {
  child: ChildProcessWithoutNullStreams;
  exited: Promise<unknown[]>;
  messages: any[] = []; stderr = ''; pending = new Map<number, { resolve: (value: any) => void; reject: (reason: unknown) => void }>(); next = 1;
  constructor(t: TestContext, env: NodeJS.ProcessEnv, args: string[] = []) {
    this.child = spawn(binary, ['acp', '--stdio', ...args], { env });
    this.exited = once(this.child, 'exit'); this.child.stdin.on('error', () => {});
    let buffer = '';
    this.child.stdout.on('data', (bytes: Buffer) => {
      buffer += bytes.toString();
      while (buffer.includes('\n')) {
        const i = buffer.indexOf('\n'); const message = JSON.parse(buffer.slice(0, i)); buffer = buffer.slice(i + 1);
        this.messages.push(message);
        const waiting = this.pending.get(message.id);
        if (waiting) { this.pending.delete(message.id); if (message.error) waiting.reject(message.error); else waiting.resolve(message.result); }
      }
    });
    this.child.stderr.on('data', data => { this.stderr += data; });
    t.after(async () => { this.child.stdin.end(); const timer = setTimeout(() => this.child.kill('SIGKILL'), 4000); await this.exited; clearTimeout(timer); });
  }
  request(method: string, params: unknown) {
    const id = this.next++;
    const result = new Promise<any>((resolve, reject) => this.pending.set(id, { resolve, reject }));
    result.catch(() => {}); // Disconnection fixtures intentionally abandon a response.
    this.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    return result;
  }
  async setup(cwd: string) {
    await this.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true } } });
    return (await this.request('session/new', { cwd, mcpServers: [] })).sessionId;
  }
  prompt(sessionId: string) { return this.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'Synthetic fixture' }] }); }
}

test('stdin EOF and stdout disconnect cancel a running shell and await process cleanup', { timeout: 20000 }, async t => {
  for (const side of ['stdin', 'stdout'] as const) await t.test(side, async t => {
    const f = await fixture(t, async (_body, res) => { await sse(res, tool('exec sleep 30')); });
    const path = join(f.cwd, 'trace.jsonl');
    const peer = new RawPeer(t, f.env, ['--trace', path]);
    const id = await peer.setup(f.cwd); void peer.prompt(id);
    await waitUntil(() => peer.messages.some(m => m.params?.update?.status === 'in_progress'));
    if (side === 'stdin') peer.child.stdin.end();
    else {
      peer.child.stdout.destroy();
      // A closed output is observable on the next write. Cancellation creates
      // that write without closing the independent input half.
      peer.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'session/cancel', params: { sessionId: id } }) + '\n');
    }
    await peer.exited;
    const records = await trace(path);
    gone(records.find(e => e.type === 'shell.started').process_id);
    assert.equal(records.filter(e => e.type === 'run.finished').length, 1);
    assert.equal(records.at(-1).outcome.status, 'cancelled');
  });
});

test('slow output applies bounded backpressure, resumes losslessly, and has a finite stall deadline', { timeout: 45000 }, async t => {
  for (const resume of [true, false]) await t.test(resume ? 'resumes' : 'stalls', async t => {
    const f = await fixture(t, async (_body, res) => {
      let body = ''; for (let i = 0; i < 60; i++) body += frame({ content: '\u0001'.repeat(1000) });
      await sse(res, body + end());
    });
    const peer = new RawPeer(t, f.env, ['--no-shell', '--trace', join(f.cwd, 'trace.jsonl')]);
    const id = await peer.setup(f.cwd);
    peer.child.stdout.pause(); const result = peer.prompt(id);
    await waitUntil(() => peer.stderr.includes('consumer slow'));
    if (resume) {
      await delay(150); peer.child.stdout.resume();
      const response = await result;
      assert.equal(outcomeOf(response).status, 'completed');
      const text = peer.messages.filter(m => m.params?.update?.sessionUpdate === 'agent_message_chunk').map(m => m.params.update.content.text).join('');
      assert.equal(text, '\u0001'.repeat(60000));
      assert.ok(peer.messages.filter(m => m.params?.update).length < 60, 'coalesces adjacent text');
    } else {
      await peer.exited;
      assert.match(peer.stderr, /consumer stalled/);
      assert.equal((await trace(join(f.cwd, 'trace.jsonl'))).filter(e => e.type === 'run.finished').length, 1);
    }
  });
});

test('oversized, unterminated, batched and flooded input is bounded; malformed JSON uses SDK errors', { timeout: 15000 }, async t => {
  for (const input of ['x'.repeat(32 * 1024 * 1024 + 1), '{"jsonrpc":"2.0"}', '[]\n', '{}\n'.repeat(129)]) {
    await t.test(`input ${input.length} bytes`, async t => {
      const peer = new RawPeer(t, process.env);
      peer.child.stdin.end(input);
      await peer.exited;
      assert.ok(peer.messages.length <= 128);
    });
  }
  await t.test('malformed message', async t => {
    const peer = new RawPeer(t, process.env);
    peer.child.stdin.write('{broken}\n');
    await waitUntil(() => peer.messages.length > 0);
    assert.equal(peer.messages[0].error.code, -32700);
    const response = await peer.request('initialize', { protocolVersion: 1, clientCapabilities: {} });
    assert.equal(response.protocolVersion, 1);
  });
});


test('SDK and stable-v1 schema pins match installed artifacts', async () => {
  const lock = JSON.parse(await readFile(new URL('../docs/acp-lock.json', import.meta.url), 'utf8'));
  const pkg = JSON.parse(await readFile(new URL('../node_modules/@agentclientprotocol/sdk/package.json', import.meta.url), 'utf8'));
  const bytes = await readFile(new URL('../' + lock.typescript_sdk.schema_path, import.meta.url));
  assert.equal(pkg.version, lock.typescript_sdk.version);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), lock.typescript_sdk.schema_sha256);
  const cargo = await readFile(new URL('../Cargo.lock', import.meta.url), 'utf8');
  for (const pin of [lock.rust_sdk, lock.rust_schema]) assert.ok(cargo.includes(`name = "${pin.crate}"\nversion = "${pin.version}"`));
});

test('busy prompts, idle cancellation, future fields and version negotiation keep session ownership explicit', { timeout: 15000 }, async t => {
  let release!: () => void;
  const released = new Promise<void>(resolve => { release = resolve; });
  const f = await fixture(t, async (body, res) => {
    assert.equal(body.tools, undefined);
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    res.write(frame({ content: 'first' }));
    await released;
    res.end(end());
  });
  let streamed!: () => void;
  const first = new Promise<void>(resolve => { streamed = resolve; });
  await withPablo({ env: f.env, args: ['--no-shell', '--no-filesystem'], onUpdate: () => streamed() }, async cx => {
    const response = await cx.request('initialize', { protocolVersion: 2, clientCapabilities: {} });
    assert.equal(response.protocolVersion, 1);
    await assert.rejects(cx.request('session/new', { cwd: '.', mcpServers: [] }), (e: any) => e.code === -32602);
    await assert.rejects(cx.request('session/new', { cwd: f.cwd, mcpServers: [{ name: 'fake', command: 'fake', args: [], env: [] }] }), (e: any) => e.code === -32602);
    const { sessionId } = await cx.request<{ sessionId: string }>('session/new', { cwd: f.cwd, mcpServers: [], futureField: true });
    await cx.notify('session/cancel', { sessionId }); // No active turn: no effect.
    const running = prompt(cx, sessionId);
    await first;
    await assert.rejects(prompt(cx, sessionId), (e: any) => e.code === -32602);
    await assert.rejects(cx.request('session/new', { cwd: f.cwd, mcpServers: [] }), (e: any) => e.code === -32602);
    await cx.notify('session/cancel', { sessionId: 'another-session' });
    release();
    assert.equal((await running).stopReason, 'end_turn');
  });
  assert.equal(f.requests.length, 1);
});

test('host setup failures respond safely; ACP initialization never requires credentials', { timeout: 15000 }, async t => {
  const f = await fixture(t, async () => { throw new Error('must not connect'); });
  await withPablo({ env: { ...f.env, PABLO_FIXTURE_ENDPOINT: 'https://unsupported.example' } }, async cx => {
    const id = await setup(cx, f.cwd);
    await assert.rejects(prompt(cx, id), (e: any) => e.code === -32603 && !JSON.stringify(e).includes('synthetic-secret'));
  });
  assert.equal(f.requests.length, 0);
});

test('missing or invalid credential files fail ACP prompt safely after successful initialization', { timeout: 15000 }, async t => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-acp-config-')));
  t.after(() => rm(cwd, { recursive: true, force: true }));
  const env = { ...process.env };
  delete env.PABLO_FIXTURE_ENDPOINT;
  delete env.AI_GATEWAY_API_KEY;
  delete env.VERCEL_AI_GATEWAY;
  for (const scenario of ['missing', 'invalid'] as const) await t.test(scenario, async () => {
    const path = join(cwd, `${scenario}.env`);
    if (scenario === 'invalid') await writeFile(path, 'AI_GATEWAY_API_KEY="synthetic invalid credential"\n');
    const updates: SessionNotification[] = []; let raw = ''; let stderr = '';
    await withPablo({ env, args: ['--env-file', path],
      onSpawn: c => c.stdout.on('data', data => { raw += data; }),
      onDiagnostic: text => { stderr += text; }, onUpdate: n => { updates.push(n); },
    }, async cx => {
      const id = await setup(cx, cwd);
      await assert.rejects(prompt(cx, id), (error: any) => {
        assert(error instanceof RequestError);
        assert.equal(error.code, -32603);
        assert.equal(error.data, 'runtime setup or execution failed');
        return true;
      });
    });
    assert.equal(updates.length, 0);
    assert(!(raw + stderr).includes('synthetic invalid credential'));
    assert(!raw.includes('end_turn'));
  });
});

test('SIGTERM cancels active shell ownership even when the client keeps stdin open', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (_body, res) => { await sse(res, tool('exec sleep 30')); });
  const path = join(f.cwd, 'trace.jsonl');
  const peer = new RawPeer(t, f.env, ['--trace', path]);
  const id = await peer.setup(f.cwd); void peer.prompt(id);
  await waitUntil(() => peer.messages.some(m => m.params?.update?.status === 'in_progress'));
  peer.child.kill('SIGTERM'); await peer.exited;
  const records = await trace(path); gone(records.find(e => e.type === 'shell.started').process_id);
  assert.equal(records.at(-1).outcome.status, 'cancelled');
});

test('cancellation while final updates drain reports ACP cancelled and preserves the already settled native outcome', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (_body, res) => {
    await sse(res, frame({ content: '\u0001'.repeat(20000) }).repeat(3) + end());
  });
  const path = join(f.cwd, 'trace.jsonl');
  const peer = new RawPeer(t, f.env, ['--no-shell', '--trace', path]);
  const id = await peer.setup(f.cwd);
  peer.child.stdout.pause(); const result = peer.prompt(id);
  await waitUntil(async () => {
    try { return (await trace(path)).at(-1).type === 'run.finished'; } catch { return false; }
  });
  peer.child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'session/cancel', params: { sessionId: id } }) + '\n');
  await waitUntil(() => peer.stderr.includes('ACP cancellation requested'));
  peer.child.stdout.resume();
  const response = await result;
  assert.equal(response.stopReason, 'cancelled');
  assert.equal(outcomeOf(response).status, 'completed');
  assert.equal((await trace(path)).at(-1).outcome.status, 'completed');
});

async function referenceCli(t: TestContext, f: Awaited<ReturnType<typeof fixture>>, args: string[] = []) {
  const child = spawn(process.execPath, [fileURLToPath(new URL('../examples/acp-client.ts', import.meta.url)), 'Read the synthetic evidence.', f.cwd, ...args], { env: f.env });
  const exited = once(child, 'exit');
  let stdout = ''; let stderr = '';
  child.stdout.on('data', data => { stdout += data; });
  child.stderr.on('data', data => { stderr += data; });
  t.after(() => { if (child.exitCode === null) child.kill('SIGTERM'); });
  const [code] = await exited;
  return { code, stdout, stderr };
}

test('Gemini defaults complete five sequential tools through both Rust CLI and ACP', { timeout: 20000 }, async t => {
  for (const mode of ['run', 'acp'] as const) await t.test(mode, async t => {
    const f = await fixture(t, async (body, res, call) => {
      assert.equal(body.model, 'google/gemini-3.8-flash');
      assert.equal(body.tool_choice, 'auto');
      assert.ok(!body.messages[0].content.includes('at most two'));
      const outputs = body.messages.filter((m: any) => m.role === 'tool').map((m: any) => JSON.parse(m.content).shell.stdout);
      assert.deepEqual(outputs, Array.from({ length: call - 1 }, (_, i) => `evidence-${i + 1}`));
      if (call <= 5) await sse(res, tool(`printf evidence-${call}`, `call_${call}`));
      else { assert.equal(call, 6); await sse(res, frame({ content: outputs.join(', ') }) + end()); }
    });
    if (mode === 'acp') {
      const result = await referenceCli(t, f);
      assert.equal(result.code, 0, result.stderr);
      assert.match(result.stdout, /evidence-1, evidence-2, evidence-3, evidence-4, evidence-5/);
      assert.match(result.stderr, /shell.run: "printf evidence-5" \(cwd "\."\)/);
      assert.match(result.stderr, /exit 0, stdout 10 bytes/);
      assert.match(result.stderr, /pablo outcome: completed/);
    } else {
      const child = spawn(binary, ['run', 'Read the evidence.', '--workspace', f.cwd], { env: f.env });
      const exited = once(child, 'exit'); let stdout = ''; let stderr = '';
      child.stdout.on('data', data => { stdout += data; }); child.stderr.on('data', data => { stderr += data; });
      t.after(() => { if (child.exitCode === null) child.kill('SIGTERM'); });
      const [code] = await exited;
      assert.equal(code, 0, stderr); assert.match(stdout, /evidence-5/);
    }
    assert.equal(f.requests.length, 6);
  });
});

test('explicit call caps request an answer from collected results and retain hard enforcement', { timeout: 20000 }, async t => {
  for (const scenario of ['tool-cap', 'model-cap', 'zero-tools', 'ignored-tool-cap', 'ignored-model-cap', 'zero-models', 'provider-error'] as const) await t.test(scenario, async t => {
    const f = await fixture(t, async (body, res, call) => {
      if (scenario === 'zero-models') assert.fail('zero model budget must prevent dispatch');
      if (scenario === 'provider-error') return sse(res, 'data: {invalid}\n\n');
      const toolCount = scenario === 'zero-tools' ? 0 : scenario.includes('model-cap') ? 1 : 2;
      assert.equal(body.tool_choice, call <= toolCount ? 'auto' : 'none');
      assert.deepEqual(body.tools, f.requests[0].tools);
      assert.deepEqual(body.messages[0], f.requests[0].messages[0]);
      if (call <= toolCount) return sse(res, tool('printf evidence', `call_${call}`));
      if (scenario.startsWith('ignored-')) return sse(res, tool('touch forbidden', `call_${call}`));
      const outputs = body.messages.filter((m: any) => m.role === 'tool').map((m: any) => JSON.parse(m.content).shell.stdout);
      assert.deepEqual(outputs, Array(toolCount).fill('evidence'));
      await sse(res, frame({ content: toolCount ? outputs.join(' ') : 'No tools available.' }) + end());
    });
    const args = scenario === 'provider-error' ? [] : scenario === 'zero-models' ? ['--max-model-calls', '0'] : scenario === 'zero-tools' ? ['--max-tool-calls', '0'] : scenario.includes('model-cap') ? ['--max-model-calls', '2'] : ['--max-tool-calls', '2'];
    const result = await referenceCli(t, f, args);
    const failed = scenario.startsWith('ignored-') || scenario === 'zero-models' || scenario === 'provider-error';
    assert.equal(result.code, failed ? 1 : 0, result.stderr);
    if (scenario === 'provider-error') assert.match(result.stderr, /pablo outcome: failed: malformed_stream \(response_received\)/);
    else if (failed) assert.match(result.stderr, new RegExp(`pablo outcome: limit_exceeded: ${scenario === 'ignored-tool-cap' ? 'tool_calls' : 'model_calls'}`));
    else assert.match(result.stderr, /pablo outcome: completed/);
    await assert.rejects(readFile(join(f.cwd, 'forbidden')), { code: 'ENOENT' });
    const expected = scenario === 'zero-models' ? 0 : scenario === 'zero-tools' || scenario === 'provider-error' ? 1 : scenario.includes('model-cap') ? 2 : 3;
    assert.equal(f.requests.length, expected);
  });
});

test('large prompts, shell evidence and escaped answers pass through gateway, ACP and captured trace', { timeout: 20000 }, async t => {
  const evidence = 'evidence-'.repeat(400000);
  const answer = '\u0001'.repeat(300000);
  const task = 'Read evidence.txt. ' + 'p'.repeat(700000);
  const f = await fixture(t, async (body, res, call) => {
    assert.equal(body.max_tokens, 65536);
    assert.equal(body.messages[1].content, task);
    if (call === 1) return sse(res, tool('cat evidence.txt'));
    assert.equal(call, 2);
    assert.equal(JSON.parse(body.messages.at(-1).content).shell.stdout, evidence);
    await sse(res, frame({ content: answer }) + frame({}, 'stop') + 'data: {"choices":[],"usage":{"prompt_tokens":9000,"completion_tokens":7000}}\n\ndata: [DONE]\n\n');
  });
  await writeFile(join(f.cwd, 'evidence.txt'), evidence);
  const path = join(f.cwd, 'trace.jsonl'); let streamed = '';
  const response = await withPablo({ env: f.env, args: ['--trace', path, '--capture-content'], onUpdate: ({ update }) => {
    if (update.sessionUpdate === 'agent_message_chunk' && update.content.type === 'text') streamed += update.content.text;
  } }, async cx => {
    const sessionId = await setup(cx, f.cwd);
    return cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: task }] });
  });
  const outcome = outcomeOf(response);
  assert.equal(outcome.status, 'completed');
  if (outcome.status === 'completed') assert.equal(outcome.output, answer);
  assert.equal(streamed, answer);
  const records = await trace(path);
  assert.equal(records.at(-1).outcome.output, answer);
  assert.equal(records.find(e => e.type === 'tool.finished').result.shell.stdout, evidence);
  assert.ok((await readFile(path)).length > 1024 * 1024);
});

test('explicit shell timeout remains effective with generous defaults', { timeout: 10000 }, async t => {
  const f = await fixture(t, async (_body, res) => sse(res, tool('exec sleep 30')));
  const path = join(f.cwd, 'trace.jsonl');
  const result = await referenceCli(t, f, ['--tool-timeout', '1', '--trace', path]);
  assert.equal(result.code, 1);
  assert.match(result.stderr, /pablo outcome: timed_out/);
  const records = await trace(path);
  gone(records.find(e => e.type === 'shell.started').process_id);
  assert.equal(records.at(-1).outcome.status, 'timed_out');
});


test('sequential independent sessions reuse HTTP setup and recover after cancellation with separate workspaces and traces', { timeout: 20000 }, async t => {
  const f = await fixture(t, async (body, res) => {
    if (body.messages.at(-1).role === 'tool') {
      const result = JSON.parse(body.messages.at(-1).content);
      assert.equal(result.shell.exit_code, 0);
      await sse(res, frame({ content: result.shell.stdout }) + end());
    } else {
      assert.equal(body.messages.length, 2, 'each new task has only its system instructions and own input');
      await sse(res, tool(body.messages.at(-1).content === 'cancel task' ? 'exec sleep 30' : 'cat evidence.txt'));
    }
  });
  const secondCwd = join(f.cwd, 'second'); await mkdir(secondCwd);
  await writeFile(join(f.cwd, 'evidence.txt'), 'first-evidence');
  await writeFile(join(secondCwd, 'evidence.txt'), 'second-evidence');
  const ids: string[] = []; let cancelling = false;
  const updates: SessionNotification[] = [];
  await withPablo({ env: f.env, args: ['--trace', join(f.cwd, '{session_id}.jsonl')], onUpdate: async (n, cx) => {
    updates.push(n);
    if (cancelling && n.update.sessionUpdate === 'tool_call_update' && n.update.status === 'in_progress') {
      await cx.notify('session/cancel', { sessionId: n.sessionId });
    }
  } }, async cx => {
    let sessionId = await setup(cx, f.cwd);
    for (let index = 0; index < 3; index++) {
      cancelling = index === 1;
      if (index > 0) ({ sessionId } = await cx.request('session/new', { cwd: index === 2 ? secondCwd : f.cwd, mcpServers: [] }));
      ids.push(sessionId);
      const traceId = String(index + 1).repeat(32);
      const response = await cx.request('session/prompt', { sessionId,
        prompt: [{ type: 'text', text: cancelling ? 'cancel task' : `read task ${index}` }],
        _meta: { 'pablo/v1': { traceparent: `00-${traceId}-123456789abcdef0-01` } },
      });
      assert.equal(response.stopReason, cancelling ? 'cancelled' : 'end_turn');
      const outcome = outcomeOf(response);
      if (cancelling) assert.equal(outcome.status, 'cancelled');
      else { assert.equal(outcome.status, 'completed'); assert(outcome.status === 'completed'); assert.equal(outcome.output, index === 0 ? 'first-evidence' : 'second-evidence'); }
      const path = join(f.cwd, `${sessionId}.jsonl`);
      const records = await trace(path);
      assert.equal((await stat(path)).mode & 0o777, 0o600);
      assert.equal(records[0].seq, 1);
      assert.equal(records[0].parent_span_id, '123456789abcdef0');
      assert(records.every(e => e.session_id === sessionId && e.trace_id === traceId));
      assert.equal(records.filter(e => e.type === 'run.finished').length, 1);
      assert.equal(records.at(-1).outcome.status, cancelling ? 'cancelled' : 'completed');
      gone(records.find(e => e.type === 'shell.started').process_id);
      const native = await readFile(path, 'utf8');
      assert(!native.includes('first-evidence') && !native.includes('second-evidence'));
      const current = updates.filter(n => n.sessionId === sessionId);
      assert(current.length >= 3);
      assert(current.every((n, i) => i === 0 || (n._meta!['pablo/v1'] as any).seq_start > (current[i - 1]._meta!['pablo/v1'] as any).seq_end));
    }
  });
  assert.equal(new Set(ids).size, 3);
  assert.equal(f.requests.length, 5);
  assert.equal(f.connections.size, 1, 'completed model responses return one shared HTTP connection to the pool across tasks');
});

test('filesystem reads use the real gateway mapping and ACP/native lifecycle with a reusable catalog', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (body, res, call) => {
    if (call === 1) {
      assert.deepEqual(body.tools.map((x: any) => x.function.name), ['fs_read', 'fs_list', 'fs_search']);
      await sse(res, frame({tool_calls: [{index: 0, id: 'fs-read', type: 'function', function: {name: 'fs_read', arguments: JSON.stringify({path: 'evidence.txt'})}}]}) + end('tool_calls'));
    } else {
      const result = JSON.parse(body.messages.at(-1).content);
      assert.equal(result.filesystem.text, 'filesystem-marker-🟣\n');
      assert.equal(result.filesystem.kind, 'read');
      assert.equal(result.shell, null);
      assert.deepEqual(body.tools, f.requests[0].tools);
      await sse(res, frame({content: 'Read verified.'}) + end());
    }
  });
  await writeFile(join(f.cwd, 'evidence.txt'), 'filesystem-marker-🟣\n');
  const path = join(f.cwd, 'native.jsonl'); const updates: SessionNotification[] = [];
  await withPablo({env: f.env, args: ['--no-shell', '--trace', path], onUpdate: n => { validate('SessionNotification', n); updates.push(n); }}, async cx => {
    const id = await setup(cx, f.cwd); const result = await prompt(cx, id); assert.equal(outcomeOf(result).status, 'completed');
  });
  const records = await trace(path); const start = records.find(e => e.type === 'tool.started'); const finish = records.find(e => e.type === 'tool.finished');
  assert.equal(start.call.name, 'fs.read'); assert.equal(start.span_id, finish.span_id);
  assert.equal(finish.result.filesystem.kind, 'read'); assert.equal(finish.result.filesystem.bytes, Buffer.byteLength('filesystem-marker-🟣\n'));
  assert(!records.some(e => e.type === 'shell.started'));
  assert(!(await readFile(path, 'utf8')).includes('filesystem-marker'));
  const toolUpdate = updates.find(n => n.update.sessionUpdate === 'tool_call')!;
  assert.equal((toolUpdate.update as any).title, 'fs.read'); assert.equal((toolUpdate.update as any).kind, 'read');
  for (const n of updates) { const m = n._meta?.['pablo/v1'] as any; validateExtension(m); assert.equal(m.span_id, records.find(e => e.seq === m.seq_end).span_id); }
});

test('filesystem policy denial carries its rule and performs no second model request', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (_body, res, call) => {
    assert.equal(call, 1);
    await sse(res, frame({tool_calls: [{index: 0, id: 'fs-denied', type: 'function', function: {name: 'fs_read', arguments: JSON.stringify({path: 'private/evidence.txt'})}}]}) + end('tool_calls'));
  });
  await mkdir(join(f.cwd, 'private')); await writeFile(join(f.cwd, 'private/evidence.txt'), 'denied synthetic marker');
  const policy = join(f.cwd, 'policy.json'); await writeFile(policy, JSON.stringify({read_roots: {default: 'allow', deny: [{id: 'deny.private', value: 'private'}]}}));
  await withPablo({env: f.env, args: ['--no-shell', '--policy', policy]}, async cx => {
    const id = await setup(cx, f.cwd); const result = await prompt(cx, id);
    assert.equal(result.stopReason, 'refusal'); assert.deepEqual(outcomeOf(result), {status: 'policy_denied', rule: {configured: {id: 'deny.private'}}});
  });
  assert.equal(f.requests.length, 1);
});

test('successive filesystem tasks keep workspace handles and content separate', { timeout: 15000 }, async t => {
  const f = await fixture(t, async (body, res, call) => {
    if (call % 2 === 1) await sse(res, frame({tool_calls: [{index: 0, id: `read-${call}`, type: 'function', function: {name: 'fs_read', arguments: '{"path":"evidence.txt"}'}}]}) + end('tool_calls'));
    else {
      const expected = call === 2 ? 'first workspace' : 'second workspace';
      assert.equal(JSON.parse(body.messages.at(-1).content).filesystem.text, expected);
      if (call === 4) assert(!JSON.stringify(body).includes('first workspace'));
      await sse(res, frame({content: expected}) + end());
    }
  });
  const second = join(f.cwd, 'second'); await mkdir(second);
  await writeFile(join(f.cwd, 'evidence.txt'), 'first workspace'); await writeFile(join(second, 'evidence.txt'), 'second workspace');
  await withPablo({env: f.env, args: ['--no-shell', '--trace', join(f.cwd, '{session_id}.jsonl')]}, async cx => {
    const first = await setup(cx, f.cwd); assert.equal((outcomeOf(await prompt(cx, first)) as any).output, 'first workspace');
    const {sessionId} = await cx.request('session/new', {cwd: second, mcpServers: []});
    assert.equal((outcomeOf(await prompt(cx, sessionId)) as any).output, 'second workspace');
    const a = await trace(join(f.cwd, `${first}.jsonl`)); const b = await trace(join(f.cwd, `${sessionId}.jsonl`));
    assert.notEqual(a[0].trace_id, b[0].trace_id);
  });
  assert.equal(f.requests.length, 4); assert.equal(f.connections.size, 1);
});

test('CLI and ACP perform revision-checked edits and atomic creates through the same lifecycle', { timeout: 20000 }, async t => {
  for (const mode of ['run', 'acp'] as const) await t.test(mode, async t => {
    const toolFrame = (name: string, args: object, call: number) => frame({tool_calls: [{index: 0, id: `mutation-${call}`, type: 'function', function: {name, arguments: JSON.stringify(args)}}]}) + end('tool_calls');
    const f = await fixture(t, async (body, res, call) => {
      assert.deepEqual(body.tools.map((x: any) => x.function.name), ['fs_read', 'fs_list', 'fs_search', 'fs_write', 'fs_edit']);
      if (call === 1) await sse(res, toolFrame('fs_read', {path: 'evidence.txt'}, call));
      else if (call === 2) {
        const read = JSON.parse(body.messages.at(-1).content).filesystem;
        assert.equal(read.text, 'before\n'); assert.equal(read.revision, createHash('sha256').update('before\n').digest('hex'));
        await sse(res, toolFrame('fs_edit', {path: 'evidence.txt', expected_revision: read.revision, old_text: 'before', new_text: 'after'}, call));
      } else if (call === 3) {
        const edited = JSON.parse(body.messages.at(-1).content).filesystem; assert.equal(edited.committed, true); assert.equal(edited.created, false);
        await sse(res, toolFrame('fs_write', {path: 'artifact.txt', text: 'created\n', expected_revision: null}, call));
      } else {
        assert.equal(call, 4); const written = JSON.parse(body.messages.at(-1).content).filesystem; assert.equal(written.committed, true); assert.equal(written.created, true);
        await sse(res, frame({content: 'edited and wrote'}) + end());
      }
    });
    await writeFile(join(f.cwd, 'evidence.txt'), 'before\n'); const path = join(f.cwd, 'trace.jsonl');
    if (mode === 'run') {
      const child = spawn(binary, ['run', 'Edit synthetic evidence', '--workspace', f.cwd, '--no-shell', '--allow-write', '--trace', path], {env: f.env});
      let stdout = ''; let stderr = ''; child.stdout.on('data', b => {stdout += b;}); child.stderr.on('data', b => {stderr += b;});
      const [code] = await once(child, 'exit'); assert.equal(code, 0, stderr); assert.equal(stdout.trim(), 'edited and wrote');
    } else {
      const updates: SessionNotification[] = [];
      await withPablo({env: f.env, args: ['--no-shell', '--allow-write', '--trace', path], onUpdate: n => {validate('SessionNotification', n); updates.push(n);}}, async cx => {
        const id = await setup(cx, f.cwd); assert.equal(outcomeOf(await prompt(cx, id)).status, 'completed');
      });
      assert.deepEqual(updates.filter(n => n.update.sessionUpdate === 'tool_call').map(n => (n.update as any).kind), ['read', 'edit', 'edit']);
    }
    assert.equal(await readFile(join(f.cwd, 'evidence.txt'), 'utf8'), 'after\n'); assert.equal(await readFile(join(f.cwd, 'artifact.txt'), 'utf8'), 'created\n');
    const native = await trace(path); const mutations = native.filter(e => e.type === 'tool.finished' && e.result.filesystem.kind === 'mutation');
    assert.equal(mutations.length, 2); assert(mutations.every(e => e.result.filesystem.committed === true));
    assert(!(await readFile(path, 'utf8')).includes('before')); assert(!mutations.some(e => e.result.filesystem.revision));
  });
});

test('write opt-in cannot override an explicit host denial', { timeout: 15000 }, async t => {
  for (const configuredDeny of [false, true]) await t.test(configuredDeny ? 'host deny' : 'no capability', async t => {
    const f = await fixture(t, async (_body, res, call) => {
      assert.equal(call, 1); await sse(res, frame({tool_calls: [{index: 0, id: 'write-denied', type: 'function', function: {name: 'fs_write', arguments: '{"path":"new.txt","text":"new","expected_revision":null}'}}]}) + end('tool_calls'));
    });
    const args = ['--no-shell'];
    if (configuredDeny) {const path = join(f.cwd, 'policy.json'); await writeFile(path, JSON.stringify({write_roots: {default: 'deny'}})); args.push('--allow-write', '--policy', path);}
    await withPablo({env: f.env, args}, async cx => {
      const id = await setup(cx, f.cwd); const result = await prompt(cx, id); assert.equal(result.stopReason, 'refusal');
      assert.equal(outcomeOf(result).status, 'policy_denied');
    });
    await assert.rejects(readFile(join(f.cwd, 'new.txt')), {code: 'ENOENT'}); assert.equal(f.requests.length, 1);
  });
});

test('negotiated task envelopes preserve exact accounting across independent ACP sessions', async t => {
  const f = await fixture(t, async (_body, res) => {
    await sse(res, frame({content: 'line\n"🙂" {"ok":true}'}) + frame({}, 'stop') + 'data: {"choices":[],"usage":{"prompt_tokens":9007199254740993,"completion_tokens":2}}\n\ndata: [DONE]\n\n');
  });
  await withPablo({env:f.env, args:['--trace', join(f.cwd, '{session_id}.jsonl'), '--capture-content']}, async cx => {
    const init = await cx.request('initialize', {protocolVersion:1, clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
    assert.equal(init.agentCapabilities?._meta?.['pablo/task-v1'], true);
    for (let i=0;i<2;i++) {
      const {sessionId} = await cx.request('session/new', {cwd:f.cwd,mcpServers:[]});
      const response = await prompt(cx, sessionId); const meta = response._meta?.['pablo/v1'] as any;
      validateExtension(meta); assert.equal(meta.outcome, undefined);
      const task = taskOf(response); assert.equal(task.accounting.model_calls, '1');
      assert.equal(task.accounting.usage.input_tokens, '9007199254740993');
      assert.equal(task.accounting.usage.output_tokens, '2');
      const terminal = (await trace(join(f.cwd, `${sessionId}.jsonl`))).at(-1);
      for (const key of ['run_id','session_id','trace_id','accounting','outcome']) assert.deepEqual((task as any)[key], terminal[key]);
    }
  });
});

test('host executable policy prevents shell creation and exact allow rules reach CLI and ACP results', async t => {
  for (const surface of ['cli','acp']) for (const deny of [true,false]) await t.test(`${surface}-${deny ? 'deny':'allow'}`, async t => {
    const f=await fixture(t,async (_body,res,call)=>{ await sse(res,call===1 ? tool('printf policy-proof > marker') : frame({content:'done'})+end()); });
    const policy=join(f.cwd,'policy.json'); await writeFile(policy,JSON.stringify({executables:{default:'deny', [deny?'deny':'allow']:[{id:deny?'deny.launcher':'allow.launcher',value:'/bin/sh'}]}}));
    const path=join(f.cwd,'trace.jsonl'); const args=['--policy',policy,'--trace',path];
    let outcome:any;
    if(surface==='acp') await withPablo({env:f.env,args},async cx=>{outcome=outcomeOf(await prompt(cx,await setup(cx,f.cwd)));});
    else {
      const child=spawn(binary,['run','fixture','--json','--workspace',f.cwd,...args],{env:f.env});let stdout='';child.stdout.on('data',b=>stdout+=b);const [code]=await once(child,'exit');assert.equal(code,deny?1:0);outcome=JSON.parse(stdout).outcome;
    }
    const records=await trace(path);
    if(deny) {assert.deepEqual(outcome,{status:'policy_denied',rule:{configured:{id:'deny.launcher'}}});assert.equal(f.requests.length,1);await assert.rejects(stat(join(f.cwd,'marker')));assert(!records.some(e=>e.type==='shell.started'));}
    else {assert.equal(outcome.status,'completed');assert.equal(await readFile(join(f.cwd,'marker'),'utf8'),'policy-proof');assert(records.find(e=>e.type==='tool.finished').result.policy_decisions.includes('allow.launcher'));}
  });
});

test('unsupported hard accounting ceilings reject CLI and ACP before provider delivery', async t => {
  const f=await fixture(t,async()=>{throw new Error('must not send');});
  for(const flag of ['--max-total-tokens','--max-cost-microusd']) {
    const child=spawn(binary,['run','fixture','--json','--workspace',f.cwd,flag,'0'],{env:f.env});let stdout='';child.stdout.on('data',b=>stdout+=b);const [code]=await once(child,'exit');assert.equal(code,2);assert.equal(JSON.parse(stdout).error.code,'invalid_configuration');
    await withPablo({env:f.env,args:[flag,'0']},async cx=>{const id=await setup(cx,f.cwd);await assert.rejects(prompt(cx,id));});
  }
  assert.equal(f.requests.length,0);
});

test('task envelope fits maximum escaping-heavy output without duplicating legacy outcome', {timeout:30000}, async t=>{
  const output='\0'.repeat(4*1024*1024);
  const f=await fixture(t,async(_body,res)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({content:output})+end());});
  await withPablo({env:f.env,args:['--no-shell','--no-filesystem']},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
    const {sessionId}=await cx.request('session/new',{cwd:f.cwd,mcpServers:[]});const response=await prompt(cx,sessionId);const task=taskOf(response);
    assert(task.outcome.status==='completed');assert.equal(task.outcome.output,output);validateExtension(response._meta?.['pablo/v1']);
    assert(Buffer.byteLength(JSON.stringify(response))<32*1024*1024);assert.equal((response._meta?.['pablo/v1'] as any).outcome,undefined);
  });
});

test('filesystem result backpressure cancels cleanly on resume or disconnect and permits a fresh session', {timeout:20000}, async t=>{
  for(const disconnect of [false,true]) await t.test(disconnect?'disconnect':'resume and reuse',async t=>{
    const text='q'.repeat(1024*1024);
    const f=await fixture(t,async(body,res)=>{
      if(body.messages.at(-1).role==='tool') {
        const result=JSON.parse(body.messages.at(-1).content);assert.equal(result.filesystem.text,text);
        if(body.messages.find((m:any)=>m.role==='user').content==='Second task') await sse(res,frame({content:'second complete'})+end());
        // Hold the first model continuation open until actual cancellation.
      } else await sse(res,frame({tool_calls:[{index:0,id:'fs-backpressure',type:'function',function:{name:'fs_read',arguments:JSON.stringify({path:'evidence.txt',max_bytes:text.length})}}]})+end('tool_calls'));
    });
    await writeFile(join(f.cwd,'evidence.txt'),text);const pattern=join(f.cwd,'{session_id}.jsonl');
    const peer=new RawPeer(t,f.env,['--no-shell','--trace',pattern,'--capture-content']);const id=await peer.setup(f.cwd);
    peer.child.stdout.pause();const pending=peer.prompt(id);
    await waitUntil(async()=>{try{return (await trace(join(f.cwd,`${id}.jsonl`))).some(e=>e.type==='tool.finished');}catch{return false;}});
    if(disconnect) peer.child.stdout.destroy();
    peer.child.stdin.write(JSON.stringify({jsonrpc:'2.0',method:'session/cancel',params:{sessionId:id}})+'\n');
    if(disconnect) await peer.exited;
    else {
      await waitUntil(()=>peer.stderr.includes('ACP cancellation requested'));peer.child.stdout.resume();
      assert.equal((await pending).stopReason,'cancelled');
      const {sessionId}=await peer.request('session/new',{cwd:f.cwd,mcpServers:[]});
      const next=await peer.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Second task'}]});assert.equal((outcomeOf(next) as any).output,'second complete');
      const terminal=(await trace(join(f.cwd,`${sessionId}.jsonl`))).at(-1);assert.equal(terminal.accounting.model_calls,'2');assert.equal(terminal.accounting.tool_calls,'1');
    }
    const records=await trace(join(f.cwd,`${id}.jsonl`));assert.equal(records.at(-1).outcome.status,'cancelled');assert.equal(records.filter(e=>e.type==='run.finished').length,1);assert(!records.some(e=>e.type==='shell.started'));
    const files=records.filter(e=>e.type==='tool.finished');assert.equal(files.length,1);assert.equal(files[0].result.status,'completed');
  });
});
