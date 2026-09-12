import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { cp, mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';

const exec = promisify(execFile);
const root = fileURLToPath(new URL('../', import.meta.url));
const binary = process.env.PABLO_PRESET_BINARY ?? join(root, 'target/debug/pablo');
const embedding = process.env.PABLO_PRESET_HOST ?? join(root, 'target/debug/examples/preset_host');
const bundle = join(root, 'presets/v1');
const wire = (delta: object, finish = 'stop') => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\ndata: [DONE]\n\n`;
const noisy = () => ({ ...cleanEnv(), OPENROUTER_API_KEY: 'unused-synthetic-provider', AI_GATEWAY_API_KEY: 'unused-synthetic-provider',
  PABLO_FIXTURE_ENDPOINT: 'http://127.0.0.1:1/unused', PABLO_MODEL: 'unused/model', PABLO_MAX_TOOL_CALLS: '1',
  OTEL_TRACES_EXPORTER: 'otlp', OTEL_EXPORTER_OTLP_ENDPOINT: 'http://127.0.0.1:1/unused', OTEL_EXPORTER_OTLP_HEADERS: 'secret=unused-synthetic-header',
  OTEL_RESOURCE_ATTRIBUTES: 'deployment.preset.version=wrong', OTEL_SDK_DISABLED: 'true' });
async function invoke(executable: string, args: string[], env = cleanEnv()) {
  try { return { code: 0, ...await exec(executable, args, { env, timeout: 30000, maxBuffer: 12 * 1024 * 1024 }) }; }
  catch (error) { const result = error as { code: number; stdout: string; stderr: string }; assert.equal(typeof result.code, 'number'); return result; }
}
async function peer(kind: 'mcp' | 'a2a', cwd: string) {
  await mkdir(cwd);
  const child = spawn(join(root, `.pablo/${kind}-fixture-venv/bin/python`),
    [join(root, `tests/fixtures/${kind}/${kind === 'mcp' ? 'http_server.py' : 'wire_server.py'}`), ...(kind === 'mcp' ? ['json', 'x-evidence-token'] : [])],
    { cwd, env: {}, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '', stderr = ''; let failure: Error | undefined;
  child.stdout.on('data', chunk => { stdout += chunk; }); child.stderr.on('data', chunk => { stderr += chunk; });
  child.on('error', error => { failure = error; });
  const closed = new Promise<void>(resolve => child.once('close', () => resolve()));
  const close = async () => {
    if (child.exitCode === null) child.kill('SIGTERM');
    const timer = setTimeout(() => child.kill('SIGKILL'), 3000);
    try { await closed; } finally { clearTimeout(timer); }
  };
  try {
    for (let i = 0; i < 500; i++) {
      if (failure) throw failure;
      assert.equal(child.exitCode, null, stderr);
      assert(stdout.length + stderr.length < 65536);
      if (kind === 'a2a' && stdout.includes('\n')) return { url: JSON.parse(stdout.split('\n')[0]).url as string, close };
      if (kind === 'mcp') {
        try { const url = await readFile(join(cwd, 'endpoint'), 'utf8'); if (/^http:\/\/127\.0\.0\.1:\d+\/mcp$/.test(url)) return { url, close }; } catch {}
      }
      await delay(10);
    }
    throw new Error(`${kind} readiness timeout`);
  } catch (error) { await close(); throw error; }
}

test('G04 versioned presets render with stable locked identity and reject unsupported options', { timeout: 30000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-g04-config-')));
  try {
    const manifest = JSON.parse(await readFile(join(bundle, 'manifest.json'), 'utf8'));
    for (const preset of ['base', 'production', 'development']) {
      const flags = ['--config', join(bundle, `${preset}.toml`), '--locked', '--bind', `workspace=${cwd}`, '--bind', `preset=${bundle}`];
      const clean = await invoke(binary, ['config', 'explain', ...flags]); assert.equal(clean.code, 0, clean.stderr);
      const noise = await invoke(binary, ['config', 'explain', ...flags], noisy()); assert.equal(noise.code, 0, noise.stderr);
      assert.deepEqual(JSON.parse(noise.stdout), JSON.parse(clean.stdout));
      const expected = JSON.parse(clean.stdout);
      assert.deepEqual({ fingerprint: expected.fingerprint, input_fingerprint: expected.input_fingerprint }, manifest.entries[preset].default);
      const rendered = await invoke(binary, ['config', 'render', ...flags]); assert.equal(rendered.code, 0, rendered.stderr);
      assert(!rendered.stdout.includes(cwd)); assert(!rendered.stdout.includes(bundle)); assert(!rendered.stdout.includes('unused-synthetic'));
      const path = join(cwd, `${preset}.toml`); await writeFile(path, rendered.stdout);
      const reloaded = await invoke(binary, ['config', 'explain', '--config', path, '--bind', `workspace=${cwd}`, '--bind', `preset=${bundle}`]);
      assert.equal(reloaded.code, 0, reloaded.stderr);
      const actual = JSON.parse(reloaded.stdout);
      assert.equal(actual.fingerprint, expected.fingerprint); assert.notEqual(actual.input_fingerprint, expected.input_fingerprint);
      assert.deepEqual(actual.config, expected.config);
      const options = actual.config.options;
      for (const name of ['run', 'models', 'routes', 'context', 'shell', 'filesystem', 'policy', 'limits', 'output', 'mcp', 'skills', 'children', 'a2a', 'interfaces', 'trace', 'otel']) assert(options[name], name);
      assert.equal(options.limits.max_model_calls, 'unlimited'); assert.equal(options.limits.max_tool_calls, 'unlimited');
      assert(Object.values(options.limits.filesystem).every(value => value === 'unlimited'));
      assert.equal(options.context.keep_recent_turns, 0);
      assert.equal(options.filesystem.write, preset === 'development');
      assert.equal(options.models.primary.id, 'z-ai/glm-5.3-flash'); assert.equal(options.models.secondary.id, 'zai/glm-5.3-flash');
      // Every real options namespace must reject an unknown property, even if a
      // rendered deployment otherwise resolves. A typo may never become a no-op.
      for (const namespace of ['context', 'shell', 'filesystem', 'limits', 'output', 'mcp', 'skills', 'children', 'a2a', 'interfaces', 'trace', 'otel']) {
        const invalid = join(cwd, 'invalid.toml');
        await writeFile(invalid, `schema_version=1\n[options.${namespace}]\nunsupported_setting=true\n`);
        const rejected = await invoke(binary, ['config', 'validate', '--config', invalid, '--bind', `workspace=${cwd}`]);
        assert.equal(rejected.code, 2, namespace); assert.match(rejected.stderr, /config_/); assert.equal(rejected.stdout, '');
      }
    }
    for (const setting of ['[options.interfaces]\ntui={}', '[options.interfaces]\nacp={}', '[options.children]\nmax_active=100',
      '[options.diagnostics]\nprobe="mcp"', '[options.compatibility]\nversion="any"', 'requires=[]']) {
      const invalid = join(cwd, 'unsupported.toml'); await writeFile(invalid, `schema_version=1\n${setting}\n`);
      const result = await invoke(binary, ['config', 'validate', '--config', invalid, '--bind', `workspace=${cwd}`]);
      assert.equal(result.code, 2); assert.match(result.stderr, /config_/); assert.equal(result.stdout, '');
    }
  } finally { await rm(cwd, { recursive: true, force: true }); }
});

for (const preset of ['base', 'production', 'development']) {
  test(`G04 ${preset}: rendered CLI, ACP and direct embedding execute all configured subsystems and deny shell`, { timeout: 120000 }, async () => {
    const cwd = await realpath(await mkdtemp(join(tmpdir(), `pablo-g04-${preset}-`)));
    let mcp: Awaited<ReturnType<typeof peer>> | undefined, a2a: Awaited<ReturnType<typeof peer>> | undefined;
    let gateway: Awaited<ReturnType<typeof server>> | undefined;
    let collector: Awaited<ReturnType<typeof server>> | undefined;
    const spans: Buffer[] = [];
    let step = 0, fallback = 0, summaries = 0, childCalls = 0, requestsSeen = 0, denial = false, writing = false;
    let localId = '', remoteId = '';
    const evidence = 'Invoice USD 123.45 due 2030-04-05; artifact invoice.json revision rev-7.';
    const summary = `Goal: verify invoice. ${evidence} Next: read Skill resource, query MCP, delegate local and remote review, return JSON. No writes performed.`;
    try {
      await writeFile(join(cwd, 'evidence.txt'), evidence + '\n' + 'raw-noise '.repeat(1000));
      mcp = await peer('mcp', join(cwd, 'mcp')); a2a = await peer('a2a', join(cwd, 'a2a'));
      await writeFile(join(cwd, 'mcp/evidence.txt'), 'MCP verified invoice rev-7');
      collector = await server(async (req, res) => {
        assert.equal(req.url, '/v1/traces'); assert.equal(req.headers['content-type'], 'application/x-protobuf');
        assert.equal(req.headers.authorization, 'Bearer synthetic-g04-collector');
        spans.push(await body(req)); res.end();
      });
      gateway = await server(async (req, res) => {
        requestsSeen++;
        assert.equal(req.headers.authorization, 'Bearer pablo-local-fixture');
        const request = JSON.parse((await body(req)).toString());
        const messages = request.messages;
        const user = messages.find((message: any) => message.role === 'user').content;
        const finish = (value: unknown) => { res.writeHead(200, { 'content-type': 'text/event-stream' }); res.end(wire({ content: JSON.stringify(value) })); };
        if (user.startsWith('{') && JSON.parse(user).task === 'review invoice') {
          childCalls++; assert.equal(request.model, 'zai/glm-5.3-flash'); assert(!request.tools?.length);
          assert.deepEqual(JSON.parse(user).context, [evidence]); finish({ answer: 'local verified rev-7' }); return;
        }
        if (request.model === 'z-ai/glm-5.3-flash') {
          fallback++; assert.equal(fallback, 1); res.writeHead(503, { 'content-type': 'application/json' }); res.end('{"error":{"message":"synthetic unavailable"}}'); return;
        }
        assert.equal(request.model, 'zai/glm-5.3-flash');
        assert(messages.some((message: any) => message.content?.includes('Distinguish verified facts from assumptions')));
        const call = (name: string, args: object) => { res.writeHead(200, { 'content-type': 'text/event-stream' }); res.end(wire({ tool_calls: [{ index: 0, id: `g04-${step}`, type: 'function', function: { name, arguments: JSON.stringify(args) } }] }, 'tool_calls')); };
        if (denial) { call('shell_run', { command: '/bin/rm forbidden-marker', cwd: '.' }); return; }
        if (writing) {
          if (messages.at(-1).role === 'tool') {
            const result = JSON.parse(messages.at(-1).content);
            assert.equal(result.filesystem.committed, true); finish({ answer: 'write committed' });
          } else call('fs_write', { path: 'created.txt', text: 'development write', expected_revision: null });
          return;
        }
        if (messages.at(-1).content?.startsWith('Create a concise handoff')) {
          summaries++; assert.equal(summaries, 1); assert(JSON.stringify(messages).includes(evidence)); assert.equal(request.tool_choice, 'none'); finish(summary); return;
        }
        if (summaries) { assert(messages.some((message: any) => message.content?.includes(summary))); assert(!JSON.stringify(messages).includes('raw-noise')); }
        const last = messages.at(-1).role === 'tool' ? JSON.parse(messages.at(-1).content) : undefined;
        switch (step++) {
          case 0: call('fs_read', { path: 'evidence.txt' }); break;
          case 1:
            assert(last.filesystem.text.startsWith(evidence));
            res.writeHead(400, { 'content-type': 'application/json' }); res.end('{"error":{"code":"context_length_exceeded","message":"synthetic forced compaction"}}'); break;
          case 2: call('skill_read', { skill: 'preset/evidence', path: 'checklist.txt' }); break;
          case 3:
            assert.equal(last.skill.resource.text, 'Preserve amount, currency, due date, artifact path and revision.\n');
            call(request.tools.find((tool: any) => tool.function.name.startsWith('mcp_')).function.name, {}); break;
          case 4:
            assert.equal(last.mcp.content.structured.text, 'MCP verified invoice rev-7');
            call('subagent', { action: 'spawn', request: { input: 'review invoice', context: [evidence], capabilities: { tools: [], skills: [], mcp_servers: [], model_route: ['secondary'] }, output_schema: { type: 'object' } } }); break;
          case 5:
            localId = last.subagent.agent.agent_id;
            call('subagent', { action: 'wait', agent_ids: [localId], mode: 'all', timeout_ms: 5000 }); break;
          case 6:
            assert.equal(last.subagent.settled[0].outcome.status, 'completed'); assert.equal(last.subagent.settled[0].validation.status, 'valid');
            assert.deepEqual(JSON.parse(last.subagent.settled[0].outcome.output), { answer: 'local verified rev-7' });
            call('subagent', { action: 'spawn_remote', remote_request: { remote: 'reviewer', parts: [{ text: 'assembly' }], accepted_output_modes: ['text/plain', 'application/octet-stream'], stream: true } }); break;
          case 7:
            remoteId = last.subagent.agent.agent_id;
            call('subagent', { action: 'wait', agent_ids: [remoteId], mode: 'all', timeout_ms: 5000 }); break;
          case 8:
            assert.equal(last.subagent.settled[0].outcome.status, 'completed');
            assert.equal(last.subagent.settled[0].remote.result.artifacts[0].parts[0].text, 'first');
            assert.equal(last.subagent.settled[0].accounting.model_calls, '0');
            call('shell_run', { command: '/usr/bin/printf verified', cwd: '.' }); break;
          case 9: assert.equal(last.shell.stdout, 'verified'); finish({ answer: evidence }); break;
          default: throw new Error(`unexpected G04 step ${step}`);
        }
      });
      const bindings = ['--bind', `workspace=${cwd}`, '--bind', `preset=${bundle}`];
      // Instantiate the documented collector endpoint; authority/policies and
      // every other shipped setting remain the original composed preset.
      const instance = join(cwd, 'config'); await cp(bundle, instance, { recursive: true });
      const base = await readFile(join(instance, 'base.toml'), 'utf8');
      await writeFile(join(instance, 'base.toml'), base.replace('http://localhost:4318/v1/traces', collector.url + '/v1/traces'));
      const sourceFlags = ['--config', join(instance, `${preset}.toml`), '--profile', 'observed', '--locked', ...bindings];
      const source = await invoke(binary, ['config', 'explain', ...sourceFlags]); assert.equal(source.code, 0, source.stderr);
      const sourceNoisy = await invoke(binary, ['config', 'explain', ...sourceFlags], noisy()); assert.equal(sourceNoisy.code, 0, sourceNoisy.stderr);
      assert.deepEqual(JSON.parse(sourceNoisy.stdout), JSON.parse(source.stdout));
      const rendered = await invoke(binary, ['config', 'render', ...sourceFlags]);
      assert.equal(rendered.code, 0, rendered.stderr);
      const entry = join(cwd, 'rendered.toml'); await writeFile(entry, rendered.stdout);
      const flags = ['--config', entry, ...bindings, '--fixture-endpoint', gateway.url, '--fixture-mcp-endpoint', `evidence=${mcp.url}`, '--fixture-a2a-endpoint', `reviewer=${a2a.url}`];
      const explained = await invoke(binary, ['config', 'explain', ...flags]); assert.equal(explained.code, 0, explained.stderr);
      const fingerprint = JSON.parse(explained.stdout).fingerprint;
      assert.equal(fingerprint, JSON.parse(source.stdout).fingerprint);
      assert.notEqual(JSON.parse(explained.stdout).input_fingerprint, JSON.parse(source.stdout).input_fingerprint);
      for (const host of ['cli', 'acp', 'embedding']) {
        for (const scenario of ['complete', 'deny', 'write']) {
          const denied = scenario === 'deny' || (scenario === 'write' && preset !== 'development');
          step = fallback = summaries = childCalls = requestsSeen = 0; denial = scenario === 'deny'; writing = scenario === 'write';
          await writeFile(join(cwd, 'forbidden-marker'), 'must survive denial');
          const env = { ...(host === 'cli' ? cleanEnv() : noisy()), PABLO_EVIDENCE_TOKEN: 'synthetic-http-token', PABLO_OTLP_HEADERS: 'authorization=Bearer%20synthetic-g04-collector' };
          const exportedBefore = spans.length;
          const input = writing ? 'Write a development artifact' : denied ? 'Attempt forbidden shell command' : 'Verify the invoice and preserve exact task-relevant details';
          let task: any;
          if (host === 'acp') {
            await withPablo({ binary, args: flags, env }, async cx => {
              await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true, 'pablo/output-v1': true, 'pablo/output-repair-v1': true } } });
              const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
              task = taskOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: input }] }));
            });
          } else {
            const result = await invoke(host === 'cli' ? binary : embedding, [...(host === 'cli' ? ['run'] : []), input, ...flags], env);
            assert.equal(result.code, host === 'cli' && denied ? 1 : 0, result.stderr); task = JSON.parse(result.stdout);
          }
          assert.equal(task.outcome.status, denied ? 'policy_denied' : 'completed');
          assert.equal(task.accounting.model_calls, String(requestsSeen), 'fallback, compaction and child calls share exact root accounting');
          assert(spans.length > exportedBefore, `${host} must flush explicit OTLP despite ambient disablement`);
          assert.equal(fallback, 1); assert.equal(summaries, scenario === 'complete' ? 1 : 0); assert.equal(childCalls, scenario === 'complete' ? 1 : 0);
          if (!denied) { assert.deepEqual(JSON.parse(task.outcome.output), { answer: writing ? 'write committed' : evidence }); assert.equal(task.output_validation.status, 'valid'); }
          if (writing && !denied) { assert.equal(await readFile(join(cwd, 'created.txt'), 'utf8'), 'development write'); await rm(join(cwd, 'created.txt')); }
          else assert(!(await readdir(cwd)).includes('created.txt'));
          assert.equal(await readFile(join(cwd, 'forbidden-marker'), 'utf8'), 'must survive denial');
          const raw = await readFile(join(cwd, `trace-${task.session_id}.jsonl`), 'utf8');
          const events = raw.trim().split('\n').map(line => JSON.parse(line));
          assert.equal(events[0].deployment.fingerprint, fingerprint);
          assert.equal(events.at(-1).type, 'run.finished'); assert.equal(events.at(-1).agent.depth, 0);
          assert.deepEqual(events.map(event => event.root_seq), events.map((_, i) => i + 1));
          assert.equal(events.filter(event => event.type === 'run.finished').length, scenario === 'complete' ? 3 : 1);
          assert.equal(events.filter(event => event.type === 'shell.started').length, scenario === 'complete' ? 1 : 0);
          for (const secret of [cwd, evidence, 'synthetic-http-token', 'unused-synthetic-provider']) assert(!raw.includes(secret));
          await rm(join(cwd, `trace-${task.session_id}.jsonl`));
        }
      }
      const requests = (await readFile(join(cwd, 'mcp/requests.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
      assert.equal(requests.filter(request => request.rpc === 'tools/call').length, 3);
      assert.equal(requests.filter(request => request.method === 'DELETE').length, 9);
      const exported = Buffer.concat(spans);
      assert(exported.includes(Buffer.from(fingerprint))); assert(exported.includes(Buffer.from('deployment.preset.version')));
      for (const forbidden of [evidence, 'synthetic-g04-collector', 'unused-synthetic-provider', 'unused-synthetic-header']) assert(!exported.includes(Buffer.from(forbidden)));
      assert(!(await readdir(cwd)).includes('inert.bin'));
    } finally {
      try { if (gateway) await gateway.close(); } finally {
        try { if (collector) await collector.close(); } finally {
          try { if (a2a) await a2a.close(); } finally { if (mcp) await mcp.close(); await rm(cwd, { recursive: true, force: true }); }
        }
      }
    }
  });
}
