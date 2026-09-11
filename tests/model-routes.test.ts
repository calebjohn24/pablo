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
import { Ajv2020 } from 'ajv/dist/2020.js';
const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const ajv = new Ajv2020({ strict: false });
ajv.addSchema(JSON.parse(await readFile(new URL('../docs/project/schemas/deployment-v1.schema.json', import.meta.url), 'utf8')));
const checkExplanation = ajv.compile<any>(JSON.parse(await readFile(new URL('../docs/project/schemas/resolved-deployment-v1.schema.json', import.meta.url), 'utf8')));
const base = await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml', import.meta.url), 'utf8');
const frame = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;

test('F01 executable explains ordered routes offline and single-entry CLI ACP match the selected gateway', { timeout: 15000 }, async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-routes-')));
  const requests: any[] = []; const expected = 'fresh file evidence 🌱';
  const gateway = await server(async (req, res) => {
    assert.equal(req.headers.authorization, 'Bearer pablo-local-fixture');
    const request = JSON.parse((await body(req)).toString()); requests.push(request);
    assert.equal(request.model, 'z-ai/glm-5.3-flash');
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (request.messages.at(-1).role === 'tool') {
      assert.equal(JSON.parse(request.messages.at(-1).content).filesystem.text, expected);
      res.end(frame({ content: expected }, 'stop') + 'data: [DONE]\n\n');
    } else res.end(frame({ tool_calls: [{ index: 0, id: 'route_read', type: 'function', function: { name: 'fs_read', arguments: '{"path":"evidence.txt"}' } }] }, 'tool_calls') + 'data: [DONE]\n\n');
  });
  try {
    const entry = join(cwd, 'entry.toml'); await writeFile(entry, base); await writeFile(join(cwd, 'evidence.txt'), expected);
    const args = ['--config', entry, '--bind', `workspace=${cwd}`]; const env = { ...cleanEnv(), ROUTE_ROUTER_KEY: 'invalid private key' };
    const explained = JSON.parse((await exec(binary, ['config', 'explain', ...args], { env })).stdout);
    assert(checkExplanation(explained), ajv.errorsText(checkExplanation.errors));
    assert.equal(explained.contract_revision, 'c3.19');
    assert.deepEqual(explained.model_route.entries.map((e: any) => e.name), ['primary', 'secondary', 'third']);
    assert.deepEqual(explained.model_route.entries.map((e: any) => e.credential), ['router', 'vercel', 'responses']);
    assert.equal(explained.model_route.selected_entry, 'primary'); assert.equal(explained.model_route.execution_available, true);
    assert(!JSON.stringify(explained).includes('invalid private key'));
    await exec(binary, ['config', 'validate', ...args], { env });
    const rendered = (await exec(binary, ['config', 'render', ...args], { env })).stdout; await writeFile(join(cwd, 'rendered.toml'), rendered);
    const reloaded = JSON.parse((await exec(binary, ['config', 'explain', '--config', join(cwd, 'rendered.toml'), '--bind', `workspace=${cwd}`], { env })).stdout);
    assert.equal(reloaded.fingerprint, explained.fingerprint); assert.deepEqual(reloaded.model_route, explained.model_route);
    // All route credentials are checked before any request on the live path.
    await assert.rejects(exec(binary, ['run', 'not admitted', ...args], { env, timeout: 5000 }), error => {
      const e = error as any; assert.equal(e.code, 2); assert.match(e.stderr, /config_credential_invalid/); assert(!e.stderr.includes('private')); return true;
    });
    assert.equal(requests.length, 0);
    await writeFile(entry, base.replace('entries=[{model="primary"},{model="secondary"},{model="third"}]', 'entries=[{model="primary"}]'));
    const configured = [...args, '--fixture-endpoint', gateway.url];
    for (const override of [['--model', 'other-model'], ['--provider', 'vercel']]) await assert.rejects(exec(binary, ['run', 'conflicting selection', ...configured, ...override], { env }), error => {
      const e = error as any; assert.equal(e.code, 2); assert.match(e.stderr, /config_conflict/); return true;
    });
    assert.equal(requests.length, 0);
    const cli = JSON.parse((await exec(binary, ['run', 'read file', ...configured, '--json'], { env })).stdout);
    assert.equal(cli.outcome.output, expected); assert.equal(cli.accounting.model_calls, '2'); assert.equal(cli.accounting.tool_calls, '1');
    const legacy = JSON.parse((await exec(binary, ['run', 'read file', '--provider', 'openrouter', '--workspace', cwd, '--no-shell', '--max-model-calls', '2', '--max-tool-calls', '1', '--json'], { env: { ...cleanEnv(), PABLO_FIXTURE_ENDPOINT: gateway.url } })).stdout);
    assert.deepEqual(legacy.outcome, cli.outcome); assert.deepEqual(legacy.accounting, cli.accounting); assert.deepEqual(requests.slice(0, 2), requests.slice(2, 4));
    await withPablo({ binary, args: configured, env }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true, 'pablo/task-v1': true } } });
      for (let i = 0; i < 2; i++) {
        const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
        const task = taskOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'read file' }] }));
        assert.deepEqual(task.outcome, cli.outcome); assert.deepEqual(task.accounting, cli.accounting);
      }
    });
    assert.equal(requests.length, 8);
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
});
