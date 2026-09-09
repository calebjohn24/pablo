import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { cleanEnv, server } from './fixtures/telemetry.ts';

const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const corpus = fileURLToPath(new URL('../docs/project/fixtures/c3-deployment/', import.meta.url));
async function invoke(args: string[], env = cleanEnv()) {
  try { return { code: 0, ...await exec(binary, args, { env, timeout: 5000, maxBuffer: 9 * 1024 * 1024 }) }; }
  catch (error) {
    const result = error as { code: number; stdout: string; stderr: string };
    assert.equal(typeof result.code, 'number');
    return result;
  }
}

test('offline inspection preserves the golden config, renders deployably and ignores ambient secrets/exporters', async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-inspection-')));
  let requests = 0;
  const receiver = await server((_req, res) => { requests++; res.end(); });
  try {
    const env = { ...cleanEnv(), AI_GATEWAY_API_KEY: 'synthetic-private-key',
      OTEL_TRACES_EXPORTER: 'otlp', OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url,
      PABLO_FIXTURE_ENDPOINT: receiver.url, OTEL_EXPORTER_OTLP_HEADERS: 'private=synthetic-private-header' };
    const flags = ['--config', join(corpus, 'production.toml'), '--bind', `workspace=${cwd}`];
    const validation = await invoke(['config', 'validate', ...flags], env);
    assert.equal(validation.code, 0, validation.stderr);
    const explained = await invoke(['config', 'explain', ...flags], env);
    assert.equal(explained.code, 0, explained.stderr);
    const golden = JSON.parse(await readFile(join(corpus, 'production.resolved.json'), 'utf8'));
    assert.deepEqual(JSON.parse(explained.stdout), golden);
    const rendered = await invoke(['config', 'render', ...flags], env);
    assert.equal(rendered.code, 0, rendered.stderr);
    assert(!rendered.stdout.includes('synthetic-private'));
    assert(!rendered.stdout.includes(cwd));
    const entry = join(cwd, 'rendered.toml');
    await writeFile(entry, rendered.stdout);
    const reloaded = await invoke(['config', 'explain', '--config', entry, '--bind', `workspace=${cwd}`], env);
    assert.equal(reloaded.code, 0, reloaded.stderr);
    const actual = JSON.parse(reloaded.stdout);
    assert.deepEqual(actual.config, golden.config);
    assert.equal(actual.fingerprint, golden.fingerprint);
    assert.notEqual(actual.input_fingerprint, golden.input_fingerprint);
    assert.equal(requests, 0);
  } finally { await receiver.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('inspection errors are bounded exit 2 diagnostics with empty stdout including --json', async () => {
  for (const args of [
    ['config', 'validate'],
    ['config', 'explain', '--config', join(corpus, 'invalid/unknown-option.toml'), '--json'],
    ['config', 'validate', '--config', join(corpus, 'production.toml')],
    ['config', 'render', '--config', join(corpus, 'production.toml'), '--bind', 'workspace=/tmp', '--bind', 'workspace=/tmp'],
  ]) {
    const result = await invoke(args);
    assert.equal(result.code, 2);
    assert.equal(result.stdout, '');
    assert.match(result.stderr, /config_/);
    assert(result.stderr.length < 1100);
  }
});

test('only declared environment values enter inspection and changed files affect the next invocation', async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-config-env-')));
  try {
    const entry = join(cwd, 'entry.toml');
    const text = `schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="SYNTHETIC_ABSENT"}]\n[environment.TEST_CALLS]\noption="limits.max_tool_calls"\nrequired=true\n`;
    await writeFile(entry, text);
    const args = ['config', 'explain', '--config', entry, '--bind', `workspace=${cwd}`];
    assert.equal((await invoke(args)).code, 2);
    const first = await invoke(args, { ...cleanEnv(), TEST_CALLS: '3' });
    assert.equal(first.code, 0, first.stderr);
    assert.equal(JSON.parse(first.stdout).config.options.limits.max_tool_calls, 3);
    await writeFile(entry, text + '\n[options.shell]\nenabled=false\n');
    const second = await invoke(args, { ...cleanEnv(), TEST_CALLS: '3' });
    assert.equal(second.code, 0, second.stderr);
    assert.notEqual(JSON.parse(first.stdout).fingerprint, JSON.parse(second.stdout).fingerprint);
    const invalid = await invoke(args, { ...cleanEnv(), TEST_CALLS: 'synthetic-private-invalid' });
    assert.equal(invalid.code, 2);
    assert.equal(invalid.stdout, '');
    assert(!invalid.stderr.includes('synthetic-private-invalid'));
  } finally { await rm(cwd, { recursive: true, force: true }); }
});
