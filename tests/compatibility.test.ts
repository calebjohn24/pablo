import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFile, readdir, mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { Ajv2020 } from 'ajv/dist/2020.js';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { existsSync } from 'node:fs';
import { withPablo, taskOf } from '../examples/acp-client.ts';
import * as c2 from '../docs/project/fixtures/c3-compatibility/c2-acp-client.ts';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
const root = fileURLToPath(new URL('../', import.meta.url));
const binary = process.env.PABLO_COMPATIBILITY_BINARY ?? join(root, 'target/debug/pablo');
const ajv = new Ajv2020({ strict: false, validateFormats: false });
const legacySchema = ajv.compile<Record<string, any>>(JSON.parse(await readFile(join(root, 'docs/project/fixtures/c3-compatibility/c2-acp.schema.json'), 'utf8')));
const modernSchema = ajv.compile<Record<string, any>>(JSON.parse(await readFile(join(root, 'docs/pablo-acp-v2.schema.json'), 'utf8')));

test('K01 frozen C2 client/schema and v2 client negotiate distinct metadata without silent version reuse', { timeout: 20000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-k01-clients-')));
  let denied = false, requests = 0;
  const gateway = await server(async (req, res) => {
    requests++; const request = JSON.parse((await body(req)).toString());
    const delta = denied ? { tool_calls: [{ index: 0, id: 'denied', type: 'function', function: { name: 'fs_write', arguments: '{"path":"forbidden.txt","text":"must not write","expected_revision":null}' } }] } : { content: 'compatible answer' };
    assert.equal(request.model, 'zai/glm-5.3-flash');
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    res.end(`data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: denied ? 'tool_calls' : 'stop' }] })}\n\ndata: [DONE]\n\n`);
  });
  try {
    for (const version of ['c2', 'v2', 'generic']) {
      for (denied of [false, true]) {
        const legacy = version === 'c2', key = legacy ? 'pablo/v1' : 'pablo/v2';
        const validate = legacy ? legacySchema : modernSchema;
        const host = legacy ? c2.withPablo : withPablo;
        let updates = 0;
        await host({ binary, args: ['--no-shell'], env: { ...cleanEnv(), PABLO_FIXTURE_ENDPOINT: gateway.url },
          onUpdate: update => {
            updates++;
            if (version === 'generic') assert.equal(update._meta, undefined);
            else { assert(validate(update._meta?.[key]), ajv.errorsText(validate.errors)); assert.equal(update._meta?.[legacy ? 'pablo/v2' : 'pablo/v1'], undefined); }
          } }, async cx => {
          const init = await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: version === 'generic' ? {} : { [key]: true, [legacy ? 'pablo/task-v1' : 'pablo/task-v2']: true } } });
          assert.equal(init.agentCapabilities?._meta?.['pablo/v1'], true); assert.equal(init.agentCapabilities?._meta?.['pablo/v2'], true);
          const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
          const response = await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'C2 compatibility task' }] });
          assert.equal(response.stopReason, denied ? 'refusal' : 'end_turn');
          if (version === 'generic') { assert.equal(response._meta, undefined); return; }
          assert(validate(response._meta?.[key]), ajv.errorsText(validate.errors));
          const task = legacy ? c2.taskOf(response) : taskOf(response);
          assert.equal(task.schema_version, legacy ? 'c2.3' : 'c3.33');
          assert.equal(task.outcome.status, denied ? 'policy_denied' : 'completed');
          assert.equal(task.accounting.model_calls, '1');
          if (!legacy && !denied) {
            const invalid = structuredClone(response._meta?.[key]) as any;
            invalid.task.output_repair = { schema_version: 'output-repair-v1', status: 'not_needed', attempts: 0, validation_attempts: 0, feedback_bytes: 0, previous_error_count: 0 };
            assert(!validate(invalid), 'repair must carry validation');
            invalid.task.output_validation = { schema_version: 'output-validation-v1', schema_sha256: 'sha256:' + '0'.repeat(64), status: 'unvalidated', diagnostics: [] };
            assert(validate(invalid), ajv.errorsText(validate.errors));
            for (const status of ['available', 'pending', 'started']) {
              invalid.task.output_repair.status = status;
              assert(!validate(invalid), 'terminal task cannot expose intermediate repair state');
            }
          }
        });
        if (!denied) assert(updates > 0);
      }
    }
    assert.equal(requests, 6);
    await assert.rejects(readFile(join(cwd, 'forbidden.txt')), { code: 'ENOENT' });
    const entry = join(cwd, 'entry.toml');
    await writeFile(entry, 'schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n');
    await c2.withPablo({ binary, args: ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', gateway.url], env: cleanEnv() }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true, 'pablo/task-v1': true } } });
      await assert.rejects(cx.request('session/new', { cwd, mcpServers: [] }), (error: any) => error.code === -32602 && JSON.stringify(error.data).includes('pablo/v2'));
    });
    await withPablo({ binary, env: cleanEnv() }, async cx => {
      await assert.rejects(cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true, 'pablo/compaction-v1': true } } }), (error: any) => error.code === -32602 && JSON.stringify(error.data).includes('pablo/v2'));
      // Failed negotiation does not consume initialize; a supported retry succeeds.
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true, 'pablo/v2': true, 'pablo/task-v2': true } } });
    });
    assert.equal(requests, 6, 'migration failures must precede model delivery');
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('K01 unused adapters open no network descriptors, listeners, MCP process or trace before a prompt', { timeout: 10000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-k01-idle-')));
  let requests = 0;
  const receiver = await server((_req, res) => { requests++; res.end(); });
  try {
    const entry = join(cwd, 'entry.toml');
    await writeFile(entry, `schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="UNREAD_KEY"}]
[options.children]
enabled=true
[options.mcp.servers.local]
transport="stdio"
command="/bin/sh"
args=["-c","touch must-not-launch"]
[options.mcp.servers.http]
transport="http"
url="https://unused.example.test/mcp"
required=false
[options.a2a.remotes.remote]
card_url="https://agent.example.test/.well-known/agent-card.json"
endpoint="https://agent.example.test/rpc"
[options.trace]
path={base="workspace",path="trace-{session_id}.jsonl"}
`);
    let pid: number | undefined;
    await withPablo({ binary, env: cleanEnv(), args: ['--config', entry, '--bind', `workspace=${cwd}`, '--fixture-endpoint', receiver.url,
      '--fixture-mcp-endpoint', `http=${receiver.url}/mcp`, '--fixture-a2a-endpoint', `remote=${receiver.url}/rpc`],
      onSpawn: child => { pid = child.pid; } }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true } } });
      await cx.request('session/new', { cwd, mcpServers: [] });
      assert.equal(requests, 0);
      const lsof = ['/usr/sbin/lsof', '/usr/bin/lsof', '/bin/lsof'].find(existsSync);
      assert(lsof, 'K01 idle-descriptor acceptance requires lsof');
      const result = await promisify(execFile)(lsof, ['-nP', '-a', '-p', String(pid), '-i'], { timeout: 5000 }).catch((error: any) => {
        assert.equal(error.code, 1); return { stdout: error.stdout as string, stderr: error.stderr as string };
      });
      assert.equal(result.stdout, '', 'unused adapters must not own IP sockets');
      const children = await promisify(execFile)('/usr/bin/pgrep', ['-P', String(pid)], { timeout: 5000 }).catch((error: any) => {
        assert.equal(error.code, 1); return { stdout: error.stdout as string };
      });
      assert.equal(children.stdout, '', 'idle host must not spawn an MCP or child process');
    });
    assert.equal(requests, 0);
    assert.deepEqual(await readdir(cwd), ['entry.toml']);
  } finally { await receiver.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('K01 deterministic bounded ACP byte mutations terminate with bounded responses and no task startup', { timeout: 10000 }, async () => {
  const seed = Buffer.from('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}');
  const frames = [seed]; let state = 0x5041424c;
  for (let index = 0; index < 96; index++) {
    state ^= state << 13; state ^= state >>> 17; state ^= state << 5;
    const offset = (state >>> 0) % seed.length;
    const bytes = Buffer.from(seed);
    bytes[offset] ^= ((state >>> 16) & 255) | 1;
    frames.push(index % 3 === 0 ? bytes.subarray(0, offset) : bytes);
  }
  const child = spawn(binary, ['acp', '--stdio', '--no-shell', '--no-filesystem'], { env: cleanEnv(), stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = '', stderr = '';
  let initialized!: () => void;
  const ready = new Promise<void>(resolve => { initialized = resolve; });
  child.stdout.on('data', chunk => { stdout += chunk; if (stdout.includes('\n')) initialized(); }); child.stderr.on('data', chunk => { stderr += chunk; });
  child.stdin.on('error', () => {});
  const close = new Promise<void>((resolve, reject) => { child.once('error', reject); child.once('close', code => { try { assert(code === 0 || code === 1); resolve(); } catch (error) { reject(error); } }); });
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  try {
    child.stdin.write(Buffer.concat([seed, Buffer.from('\n')]));
    await Promise.race([ready, close]);
    assert(stdout.includes('\n'), 'valid seed must initialize before malformed ingress');
    child.stdin.end(Buffer.concat(frames.slice(1).flatMap(frame => [frame, Buffer.from('\n')])));
    await close;
    assert(stdout.length < 256 * 1024 && stderr.length < 65536);
    const messages = stdout.trim().split('\n').map(line => JSON.parse(line));
    assert(messages.length > 0 && messages.length <= 128);
    assert(messages.every(message => message.result?.protocolVersion === 1 || message.error));
    assert(!stdout.includes('run.started') && !stdout.includes('session/update'));
  } finally { clearTimeout(timer); if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await close; } }
});
