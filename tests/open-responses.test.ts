import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
import { setTimeout as delay } from 'node:timers/promises';
import { Ajv2020 } from 'ajv/dist/2020.js';
const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const measure = fileURLToPath(new URL('../target/debug/examples/measure', import.meta.url));
const fixture = JSON.parse(await readFile(new URL('../docs/project/fixtures/c3-open-responses/roundtrip.json', import.meta.url), 'utf8'));
const model = 'fixture-text-tools-v1';
const endpoint = 'https://responses.example.test/v1/responses';
const metadataCheck = new Ajv2020({ strict: false }).compile(JSON.parse(await readFile(new URL('../docs/pablo-acp-v2.schema.json', import.meta.url), 'utf8')));
const wire = (events: any[], done = true) => events.map(e => `event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`).join('') + (done ? 'data: [DONE]\n\n' : '');
async function waitUntil(check: () => boolean) { const start = performance.now(); while (!check()) { assert(performance.now() - start < 5000, 'condition timed out'); await delay(10); } }
const preset = (capture: boolean, collector: string) => `schema_version=1
[credentials.gateway]
consumer="provider.open_responses"
sources=[{kind="environment",name="EXAMPLE_RESPONSES_TOKEN"}]
[options.model]
provider="open_responses"
id="${model}"
endpoint="${endpoint}"
capability_profile="open-responses-text-tools-v1"
[options.shell]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
capture_content=${capture}
[options.otel]
exporter="otlp"
endpoint="${collector}/v1/traces"
`;

test('OR02 core CLI and reused ACP read real files across two tool rounds and preserve private continuation', { timeout: 30000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-or02-')));
  const first = `first-${randomUUID()}`, second = `second-🌱-${randomUUID()}`;
  const expectedAnswer = first + ' ' + second;
  const requests: any[] = [], tasks: any[] = [], exports: Buffer[] = [];
  const histories = new Map<string, any[]>(), prefixes = new Map<string, string>(), connections = new Set();
  let raw = '', diagnostics = '';
  const collector = await server(async (req, res) => { exports.push(await body(req)); res.end(); });
  const gateway = await server(async (req, res) => {
    assert.equal(req.headers.authorization, 'Bearer pablo-local-fixture');
    assert.equal(req.headers.traceparent, undefined); assert.equal(req.url, '/v1/responses');
    connections.add(req.socket);
    const request = JSON.parse((await body(req)).toString()); requests.push(request);
    assert.equal(request.model, model); assert.equal(request.store, false); assert.equal(request.stream, true);
    assert.equal(request.background, false); assert.equal(request.parallel_tool_calls, false);
    assert.equal(request.truncation, 'disabled'); assert.equal(request.previous_response_id, undefined);
    assert.deepEqual(request.include, ['reasoning.encrypted_content']);
    const task = request.input[0].content[0].text;
    const results = request.input.filter((i: any) => i.type === 'function_call_output');
    const turn = results.length; assert(turn < 3);
    const prefix = JSON.stringify([request.instructions, request.tools, request.input[0]]);
    if (turn === 0) {
      assert(!histories.has(task)); prefixes.set(task, prefix);
    } else {
      assert.equal(prefixes.get(task), prefix);
      assert.deepEqual(request.input.slice(0, -1), histories.get(task));
      assert.equal(request.input.at(-1).call_id, `call_fixture_${turn}`);
      assert.equal(JSON.parse(request.input.at(-1).output).filesystem.text, turn === 1 ? first : second);
    }
    const events = structuredClone(fixture.turns[turn].events);
    if (turn === 2) {
      const partTexts = [JSON.parse(results[0].output).filesystem.text + ' ', JSON.parse(results[1].output).filesystem.text];
      for (const event of events) {
        if (event.type === 'response.output_text.delta') {
          const prior = events.filter((e: any) => e.type === event.type && e.content_index === event.content_index && e.sequence_number < event.sequence_number);
          const text = partTexts[event.content_index]; const at = Math.floor(text.length / 2);
          event.delta = prior.length === 0 ? text.slice(0, at) : text.slice(at);
        } else if (event.type === 'response.output_text.done') event.text = partTexts[event.content_index];
        else if (event.type === 'response.content_part.done') event.part.text = partTexts[event.content_index];
        else if (event.type === 'response.output_item.done') event.item.content.forEach((p: any, i: number) => p.text = partTexts[i]);
        else if (event.type === 'response.completed') event.response.output[0].content.forEach((p: any, i: number) => p.text = partTexts[i]);
      }
    }
    histories.set(task, [...request.input, ...events.at(-1).response.output]);
    const wire = ': keepalive\r\n\r\n' + events.map((e: any) => `event: ${e.type}\r\ndata: ${JSON.stringify(e)}\r\n\r\n`).join('') + 'data: [DONE]\r\n\r\n';
    const bytes = Buffer.from(wire); res.writeHead(200, { 'content-type': 'text/event-stream' });
    for (let i = 0; i < bytes.length; i += 7) if (!res.write(bytes.subarray(i, i + 7))) await once(res, 'drain');
    res.end();
  });
  try {
    await writeFile(join(cwd, 'first.txt'), first); await writeFile(join(cwd, 'second.txt'), second);
    for (const capture of [false, true]) {
      const entry = join(cwd, `entry-${capture}.toml`); await writeFile(entry, preset(capture, collector.url));
      const args = ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url + '/v1/responses'];
      const env = { ...cleanEnv(), EXAMPLE_RESPONSES_TOKEN: 'private-credential-sentinel', OPENROUTER_API_KEY: 'private-router-sentinel' };
      const cli = await exec(binary, ['run', `cli-${capture}`, ...args, '--json'], { env, timeout: 7000 });
      raw += cli.stdout; diagnostics += cli.stderr; tasks.push(JSON.parse(cli.stdout));
      const core = JSON.parse((await exec(measure, ['http', gateway.url + '/v1/responses', `core-${capture}`, ...args], { env, timeout: 7000 })).stdout);
      assert.equal(core.outcome.output, expectedAnswer); assert(core.first_text_ms >= 0 && core.first_text_ms < core.run_ms);
      const before = connections.size;
      await withPablo({ binary, args, env, onSpawn: child => child.stdout.on('data', chunk => raw += chunk), onDiagnostic: text => diagnostics += text }, async cx => {
        await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } } });
        for (let i = 0; i < 2; i++) {
          const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
          const result = await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: `acp-${capture}-${i}` }] });
          assert(metadataCheck(result._meta?.['pablo/v2']), JSON.stringify(metadataCheck.errors));
          tasks.push(taskOf(result));
        }
      });
      assert.equal(connections.size - before, 1, 'two ACP tasks reuse one transport connection');
    }
    assert.equal(requests.length, 24); assert.equal(tasks.length, 6);
    for (const task of tasks) {
      assert.equal(task.outcome.status, 'completed'); assert.equal(task.outcome.output, expectedAnswer);
      assert.deepEqual(task.accounting, { model_calls: '3', tool_calls: '2', usage: { input_tokens: '450', output_tokens: '90', cache_read_input_tokens: '240', cache_write_input_tokens: null }, cost_microusd: null, charged_tokens: null, charged_cost_microusd: null });
    }
    const traces = (await readdir(cwd)).filter(path => path.endsWith('.jsonl')); assert.equal(traces.length, 8);
    let native = '';
    for (const path of traces) {
      const text = await readFile(join(cwd, path), 'utf8'); native += text;
      const events = text.trim().split('\n').map(line => JSON.parse(line));
      assert.equal(events.filter(e => e.type === 'model.started').length, 3);
      assert.equal(events.filter(e => e.type === 'tool.finished').length, 2);
      assert.equal(events.filter(e => e.type === 'run.finished').length, 1);
      for (const event of events.filter(e => e.type === 'model.finished')) {
        assert.equal(event.model_profile.resolved_model, model);
        assert.equal(event.model_profile.capability_profile, fixture.profile);
        assert.equal(event.model_profile.revision, fixture.upstream_revision);
      }
    }
    for (const secret of [...fixture.private_sentinels, 'private-credential-sentinel', 'private-router-sentinel']) {
      assert(!(raw + diagnostics + native).includes(secret)); assert(exports.every(bytes => !bytes.includes(Buffer.from(secret))));
    }
    assert(exports.length > 0); assert(exports.every(bytes => !bytes.includes(Buffer.from(first))));
  } finally { await gateway.close(); await collector.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('OR02 malformed unsupported oversized and interrupted responses fail through CLI and ACP without tool effects', { timeout: 45000 }, async t => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-or02-fail-')));
  let response: string | Buffer = '', http = 200, calls = 0, redirects = 0, disconnect = false;
  const trap = await server((_req, res) => { redirects++; res.end(); });
  const gateway = await server(async (req, res) => {
    await body(req); calls++;
    res.writeHead(http, { 'content-type': 'text/event-stream', location: trap.url });
    if (disconnect) { res.write('event: response.created\ndata: {'); setImmediate(() => res.destroy()); }
    else res.end(response);
  });
  const mutate = (change: (e: any[]) => void) => { const e = structuredClone(fixture.turns[0].events); change(e); return wire(e); };
  const select = (e: any[], type: string) => e.find(v => v.type === type);
  const cases: Array<{ name: string, value: string | Buffer, code?: string, status?: number, disconnect?: boolean }> = [
    { name: 'HTTP rejection', value: 'private-server-error', status: 401, code: 'provider_rejected' },
    { name: 'redirect is not followed', value: '', status: 307, code: 'provider_rejected' },
    { name: 'missing event name', value: wire(fixture.turns[0].events).replace('event: response.created\n', '') },
    { name: 'event type mismatch', value: wire(fixture.turns[0].events).replace('event: response.created', 'event: response.completed') },
    { name: 'duplicate sequence', value: mutate(e => e[1].sequence_number = 0) },
    { name: 'response identity changed', value: mutate(e => e[1].response.id = 'changed') },
    { name: 'item index oversized', value: mutate(e => e[2].output_index = 9007199254740991) },
    { name: 'wrong item identity', value: mutate(e => select(e, 'response.output_text.delta').item_id = 'changed') },
    { name: 'missing item identity', value: mutate(e => delete select(e, 'response.output_text.delta').item_id) },
    { name: 'invalid obfuscation type', value: mutate(e => select(e, 'response.output_text.delta').obfuscation = {}) },
    { name: 'text snapshot mismatch', value: mutate(e => select(e, 'response.output_text.done').text = 'wrong') },
    { name: 'argument snapshot mismatch', value: mutate(e => select(e, 'response.function_call_arguments.done').arguments = '{}') },
    { name: 'terminal snapshot mismatch', value: mutate(e => e.at(-1).response.output = []) },
    { name: 'missing required response field', value: mutate(e => delete e[0].response.usage) },
    { name: 'duplicate terminal', value: mutate(e => e.push({ ...e.at(-1), sequence_number: 999 })) },
    { name: 'missing terminal', value: wire(fixture.turns[0].events.slice(0, -1)) },
    { name: 'missing DONE', value: wire(fixture.turns[0].events, false) },
    { name: 'duplicate nested field', value: wire(fixture.turns[0].events).replace('"background":false', '"background":false,"background":false') },
    { name: 'invalid JSON', value: 'event: response.created\ndata: {invalid}\n\n' },
    { name: 'invalid UTF8', value: Buffer.from([0xff, 10, 10]) },
    { name: 'unknown event semantics', value: mutate(e => e[2].type = 'vendor:private-event'), code: 'unsupported_provider_content' },
    { name: 'unknown item semantics', value: mutate(e => e[2].item.type = 'vendor:private-item'), code: 'unsupported_provider_content' },
    { name: 'raw reasoning cannot round trip', value: mutate(e => e[2].item.content = [{ type: 'reasoning_text', text: 'private-server-error' }]), code: 'unsupported_provider_content' },
    { name: 'image output', value: mutate(e => select(e, 'response.content_part.added').part = { type: 'input_image', image_url: 'https://example.test/private.png', detail: 'auto' }), code: 'unsupported_provider_content' },
    { name: 'refusal content', value: mutate(e => select(e, 'response.content_part.added').part = { type: 'refusal', refusal: 'private-server-error' }), code: 'unsupported_provider_content' },
    { name: 'negative token count', value: mutate(e => e.at(-1).response.usage.input_tokens = -1) },
    { name: 'inconsistent total tokens', value: mutate(e => e.at(-1).response.usage.total_tokens = 1) },
    { name: 'excessive generic JSON items', value: mutate(e => e[0].response.output = Array(4097).fill(null)) },
    { name: 'oversized SSE frame', value: 'data: ' + ' '.repeat(32 * 1024 * 1024) + '\n\n' },
    { name: 'disconnect after headers', value: '', disconnect: true, code: 'provider_transport' },
  ];
  try {
    const entry = join(cwd, 'entry.toml');
    await writeFile(entry, preset(true, trap.url).replace('exporter="otlp"', 'exporter="none"'));
    const args = ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url];
    const env = { ...cleanEnv(), EXAMPLE_RESPONSES_TOKEN: 'private-credential-sentinel' };
    for (const item of cases) await t.test(item.name, async () => {
      response = item.value; http = item.status ?? 200; disconnect = item.disconnect ?? false; const before = calls;
      let cli: any;
      await assert.rejects(exec(binary, ['run', 'must not execute tools', ...args, '--json'], { env, timeout: 7000 }), error => {
        const e = error as any; assert.equal(e.code, 1); cli = JSON.parse(e.stdout);
        assert(!(e.stdout + e.stderr).includes('private-server-error')); return true;
      });
      assert.equal(cli.outcome.code, item.code ?? 'malformed_stream');
      assert.equal(cli.outcome.delivery, 'response_received'); assert.equal(cli.accounting.model_calls, '1'); assert.equal(cli.accounting.tool_calls, '0');
      await withPablo({ binary, args, env }, async cx => {
        await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } } });
        const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
        await assert.rejects(cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'must not execute tools' }] }), error => {
          const task = (error as any).data['pablo/v2'].task; assert.deepEqual(task.outcome, cli.outcome); assert.deepEqual(task.accounting, cli.accounting);
          assert(!JSON.stringify(error).includes('private-server-error')); return true;
        });
      });
      assert.equal(calls - before, 2);
    });
    assert.equal(redirects, 0);
    for (const path of (await readdir(cwd)).filter(p => p.endsWith('.jsonl'))) {
      const raw = await readFile(join(cwd, path), 'utf8'); assert(!raw.includes('private-server-error')); assert(!raw.includes('private-credential-sentinel'));
      const events = raw.trim().split('\n').map(s => JSON.parse(s));
      assert.equal(events.filter(e => e.type === 'run.finished').length, 1); assert.equal(events.filter(e => e.type === 'tool.started').length, 0);
    }
  } finally { await gateway.close(); await trap.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('OR02 cancellation closes the stream and discards only the current task continuation', { timeout: 20000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-or02-cancel-')));
  let calls = 0, closed = 0, stage = 'cancel';
  const gateway = await server(async (req, res) => {
    const request = JSON.parse((await body(req)).toString()); calls++;
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (stage === 'clean') { assert.equal(request.input.length, 1); res.end(wire(fixture.turns[2].events)); return; }
    if (request.input.at(-1).type === 'function_call_output') {
      assert.equal(JSON.parse(request.input.at(-1).output).filesystem.text, 'marker');
      assert(request.input.some((i: any) => i.encrypted_content === 'opaque-synthetic-state-1'));
      res.on('close', () => closed++);
      res.write(wire(fixture.turns[2].events.slice(0, 5), false)); return;
    }
    // Keep the real first tool call, but omit commentary text so ACP cancellation
    // triggers only after the completed file read and second model request.
    const events = structuredClone(fixture.turns[0].events).filter((e: any) => !e.item_id?.startsWith('msg_') && !(e.item?.type === 'message'));
    for (const event of events) {
      if (event.output_index === 2) event.output_index = 1;
      if (event.response?.output?.length) event.response.output = event.response.output.filter((i: any) => i.type !== 'message');
    }
    res.end(wire(events));
  });
  try {
    await writeFile(join(cwd, 'first.txt'), 'marker'); const entry = join(cwd, 'entry.toml');
    await writeFile(entry, preset(false, gateway.url).replace('exporter="otlp"', 'exporter="none"'));
    const args = ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url]; const env = cleanEnv();
    const child = spawn(binary, ['run', 'cancel CLI', ...args, '--json'], { env }); let raw = ''; child.stdout.on('data', b => raw += b); child.stderr.resume();
    const exited = once(child, 'exit'); await waitUntil(() => calls === 2); child.kill('SIGINT'); assert.deepEqual(await exited, [130, null]); await waitUntil(() => closed === 1);
    const cli = JSON.parse(raw); assert.equal(cli.outcome.status, 'cancelled'); assert.equal(cli.accounting.tool_calls, '1'); assert.equal(cli.accounting.usage.input_tokens, null);
    let cancelled = false;
    await withPablo({ binary, args, env, onUpdate: async ({ sessionId, update }, cx) => {
      if (!cancelled && update.sessionUpdate === 'agent_message_chunk') { cancelled = true; await cx.notify('session/cancel', { sessionId }); }
    } }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } } });
      const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
      const task = taskOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'cancel ACP' }] }));
      assert.equal(task.outcome.status, 'cancelled'); assert.deepEqual(task.accounting, cli.accounting); await waitUntil(() => closed === 2);
      stage = 'clean'; const next = await cx.request('session/new', { cwd, mcpServers: [] });
      const clean = taskOf(await cx.request('session/prompt', { sessionId: next.sessionId, prompt: [{ type: 'text', text: 'fresh task' }] }));
      assert.equal(clean.outcome.status, 'completed'); assert.equal(clean.outcome.output, 'Alpha βeta 🌱'); assert.equal(clean.accounting.model_calls, '1');
    });
    for (const path of (await readdir(cwd)).filter(p => p.endsWith('.jsonl'))) {
      const events = (await readFile(join(cwd, path), 'utf8')).trim().split('\n').map(s => JSON.parse(s));
      assert.equal(events.filter(e => e.type === 'run.finished').length, 1);
      if (events.at(-1).outcome.status === 'cancelled') assert(events.some(e => e.type === 'model.finished' && e.usage.input_tokens === 100));
    }
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('OR02 configured raw authentication uses only the selected header and private failures precede effects', { timeout: 20000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-or02-auth-'))); let calls = 0;
  const gateway = await server(async (req, res) => {
    await body(req); calls++; assert.equal(req.headers['x-api-key'], 'pablo-local-fixture'); assert.equal(req.headers.authorization, undefined);
    res.writeHead(200, { 'content-type': 'text/event-stream' }); res.end(wire(fixture.turns[2].events));
  });
  try {
    const entry = join(cwd, 'entry.toml');
    await writeFile(entry, preset(false, gateway.url).replace('exporter="otlp"', 'exporter="none"').replace('[options.shell]', 'auth_header="X-Api-Key"\nauth_scheme="raw"\n[options.shell]'));
    const base = ['--config', entry, '--bind', `workspace=${cwd}`];
    const inspected = JSON.parse((await exec(binary, ['config', 'explain', ...base], { env: cleanEnv(), timeout: 5000 })).stdout);
    assert.equal(inspected.config.options.model.auth_scheme, 'raw');
    const success = JSON.parse((await exec(binary, ['run', 'raw header', ...base, '--fixture-endpoint', gateway.url, '--json'], { env: { ...cleanEnv(), EXAMPLE_RESPONSES_TOKEN: 'invalid private token' }, timeout: 5000 })).stdout);
    assert.equal(success.outcome.status, 'completed'); assert.equal(calls, 1);
    const before = (await readdir(cwd)).sort();
    for (const key of [undefined, '', 'invalid private token']) for (const command of [['run', 'rejected'], ['acp', '--stdio']]) {
      const env = { ...cleanEnv(), OPENROUTER_API_KEY: 'private-router-key', AI_GATEWAY_API_KEY: 'private-vercel-key', ...(key === undefined ? {} : { EXAMPLE_RESPONSES_TOKEN: key }) };
      await assert.rejects(exec(binary, [...command, ...base], { env, timeout: 5000 }), error => {
        const e = error as any; assert.equal(e.code, 2); assert.equal(e.stdout, ''); assert.match(e.stderr, /config_credential_missing|config_credential_invalid/);
        assert(!e.stderr.includes('private')); return true;
      });
    }
    assert.deepEqual((await readdir(cwd)).sort(), before); assert.equal(calls, 1);
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('OR02 terminal usage remains exact and incomplete function arguments never execute', { timeout: 20000 }, async t => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-or02-usage-')));
  let response = '', calls = 0;
  const gateway = await server(async (req, res) => {
    await body(req); calls++; res.writeHead(200, { 'content-type': 'text/event-stream' }); res.end(response);
  });
  try {
    const entry = join(cwd, 'entry.toml');
    await writeFile(entry, preset(false, gateway.url).replace('exporter="otlp"', 'exporter="none"'));
    const args = ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url];
    const env = cleanEnv();
    for (const kind of ['null', 'zero', 'large', 'incomplete', 'duplicate_done']) await t.test(kind, async () => {
      const incomplete = kind === 'incomplete';
      const events = structuredClone(fixture.turns[incomplete ? 0 : 2].events);
      let expected: any;
      if (incomplete) {
        let fragment = false;
        for (const event of events) {
          if (event.type === 'response.function_call_arguments.delta') { event.delta = fragment ? '' : '{"path":'; fragment = true; }
          if (event.type === 'response.function_call_arguments.done') event.arguments = '{"path":';
          if (event.type === 'response.output_item.done' && event.item.type === 'function_call') {
            event.item.arguments = '{"path":'; event.item.status = 'incomplete';
          }
        }
        const terminal = events.at(-1); terminal.type = 'response.incomplete'; terminal.response.status = 'incomplete';
        terminal.response.completed_at = null; terminal.response.incomplete_details = { reason: 'max_output_tokens' };
        terminal.response.output.at(-1).arguments = '{"path":'; terminal.response.output.at(-1).status = 'incomplete';
        expected = { input_tokens: '100', output_tokens: '20', cache_read_input_tokens: '40', cache_write_input_tokens: null };
      } else {
        const usage = kind === 'null' ? null : { input_tokens: kind === 'large' ? 'EXACT_INPUT' : 0, output_tokens: 0, total_tokens: kind === 'large' ? 'EXACT_INPUT' : 0, input_tokens_details: { cached_tokens: 0 }, output_tokens_details: { reasoning_tokens: 0 } };
        events.at(-1).response.usage = usage;
        expected = { input_tokens: kind === 'null' ? null : kind === 'large' ? '9007199254740993' : '0', output_tokens: kind === 'null' ? null : '0', cache_read_input_tokens: kind === 'null' ? null : '0', cache_write_input_tokens: null };
      }
      response = wire(events).replaceAll('"EXACT_INPUT"', '9007199254740993');
      if (kind === 'duplicate_done') response += 'data: [DONE]\n\n'; // Logical EOF closes the body after the first sentinel.
      let task: any;
      if (incomplete) await assert.rejects(exec(binary, ['run', kind, ...args, '--json'], { env, timeout: 5000 }), error => {
        const e = error as any; assert.equal(e.code, 1); task = JSON.parse(e.stdout); return true;
      });
      else task = JSON.parse((await exec(binary, ['run', kind, ...args, '--json'], { env, timeout: 5000 })).stdout);
      assert.deepEqual(task.accounting.usage, expected); assert.equal(task.accounting.tool_calls, '0'); assert.equal(task.accounting.model_calls, '1');
      if (incomplete) assert.deepEqual(task.outcome, { status: 'limit_exceeded', limit: 'output_tokens' });
      else { assert.equal(task.outcome.status, 'completed'); assert.equal(task.outcome.output, 'Alpha βeta 🌱'); assert.equal(task.outcome.finish_reason, 'stop'); }
      await withPablo({ binary, args, env }, async cx => {
        await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } } });
        const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
        const result = await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: kind }] });
        assert.equal(result.stopReason, incomplete ? 'max_tokens' : 'end_turn');
        assert.deepEqual(taskOf(result).accounting, task.accounting); assert.deepEqual(taskOf(result).outcome, task.outcome);
      });
    });
    assert.equal(calls, 10);
    for (const path of (await readdir(cwd)).filter(p => p.endsWith('.jsonl'))) {
      const events = (await readFile(join(cwd, path), 'utf8')).trim().split('\n').map(s => JSON.parse(s));
      assert.equal(events.filter(e => e.type === 'run.finished').length, 1); assert.equal(events.filter(e => e.type === 'tool.started').length, 0);
    }
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('OR02 cancellation before the first tool closes delivery with unknown usage', { timeout: 15000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-or02-early-cancel-'))); let calls = 0, closed = 0;
  const gateway = await server(async (req, res) => {
    await body(req); calls++; res.on('close', () => closed++);
    res.writeHead(200, { 'content-type': 'text/event-stream' }); res.write(wire(fixture.turns[2].events.slice(0, 5), false));
  });
  try {
    const entry = join(cwd, 'entry.toml'); await writeFile(entry, preset(false, gateway.url).replace('exporter="otlp"', 'exporter="none"'));
    const args = ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url]; const env = cleanEnv();
    const child = spawn(binary, ['run', 'cancel first call', ...args, '--json'], { env }); let raw = ''; child.stdout.on('data', b => raw += b); child.stderr.resume();
    const exited = once(child, 'exit'); await waitUntil(() => calls === 1); child.kill('SIGINT'); assert.deepEqual(await exited, [130, null]); await waitUntil(() => closed === 1);
    const cli = JSON.parse(raw); assert.equal(cli.outcome.status, 'cancelled'); assert.equal(cli.accounting.tool_calls, '0'); assert.equal(cli.accounting.usage.input_tokens, null);
    let cancelled = false;
    await withPablo({ binary, args, env, onUpdate: async ({ sessionId, update }, cx) => {
      if (!cancelled && update.sessionUpdate === 'agent_message_chunk') { cancelled = true; await cx.notify('session/cancel', { sessionId }); }
    } }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } } });
      const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
      const result = taskOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'cancel first call' }] }));
      assert.deepEqual(result.outcome, cli.outcome); assert.deepEqual(result.accounting, cli.accounting);
    });
    await waitUntil(() => closed === 2);
    for (const path of (await readdir(cwd)).filter(p => p.endsWith('.jsonl'))) {
      const events = (await readFile(join(cwd, path), 'utf8')).trim().split('\n').map(s => JSON.parse(s));
      assert.equal(events.filter(e => e.type === 'run.finished').length, 1); assert.equal(events.filter(e => e.type === 'tool.started').length, 0);
    }
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});
