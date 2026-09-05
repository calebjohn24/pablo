import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test, type TestContext } from 'node:test';
import { createServer, type ServerResponse } from 'node:http';
import { mkdtemp, writeFile, readFile, rm, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { Ajv2020 } from 'ajv/dist/2020.js';
import { RequestError, type ClientContext, type SessionNotification } from '@agentclientprotocol/sdk';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';

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
  const errors: unknown[] = []; const requests: any[] = [];
  const server = createServer(async (req, res) => {
    try {
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
  return { cwd, env, requests };
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
    await assert.rejects(cx.request('session/new', { cwd: f.cwd, mcpServers: [] }), (e: any) => e.code === -32602);
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
  await withPablo({ env: f.env, args: ['--no-shell'], onUpdate: () => streamed() }, async cx => {
    const response = await cx.request('initialize', { protocolVersion: 2, clientCapabilities: {} });
    assert.equal(response.protocolVersion, 1);
    await assert.rejects(cx.request('session/new', { cwd: '.', mcpServers: [] }), (e: any) => e.code === -32602);
    await assert.rejects(cx.request('session/new', { cwd: f.cwd, mcpServers: [{ name: 'fake', command: 'fake', args: [], env: [] }] }), (e: any) => e.code === -32602);
    const { sessionId } = await cx.request<{ sessionId: string }>('session/new', { cwd: f.cwd, mcpServers: [], futureField: true });
    await cx.notify('session/cancel', { sessionId }); // No active turn: no effect.
    const running = prompt(cx, sessionId);
    await first;
    await assert.rejects(prompt(cx, sessionId), (e: any) => e.code === -32602);
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
