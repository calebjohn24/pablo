// Explicit, credentialed smoke. Ordinary tests never invoke this script.
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const binary = resolve(root, process.argv[2] ?? 'target/debug/pablo');
const workspace = await mkdtemp(join(tmpdir(), 'pablo-live-'));
const trace = join(root, '.pablo', 'traces', `c1.2a-live-${randomUUID()}.jsonl`);
const marker = `pablo-evidence-${randomUUID()}`;
const env = { ...process.env };
delete env.PABLO_FIXTURE_ENDPOINT;
try {
  await mkdir(dirname(trace), { recursive: true });
  await writeFile(join(workspace, 'evidence.txt'), `${marker}\n`);
  const started = performance.now();
  // The executable reads the key privately. It is never an argument or JS value.
  const { stdout } = await promisify(execFile)(binary, [
    'run', 'Use the shell to read evidence.txt in the workspace. Return the exact file contents and nothing else.',
    '--workspace', workspace, '--trace', trace, '--timeout', '60',
  ], { cwd: root, env, timeout: 65_000, maxBuffer: 1024 * 1024 });
  const raw = await readFile(trace, 'utf8');
  const events = raw.trim().split('\n').map((line) => JSON.parse(line));
  const last = events.at(-1);
  assert(stdout.includes(marker), 'answer must contain actual evidence');
  assert(events.some((e) => e.type === 'shell.started'), 'model must execute shell');
  assert.equal(last.outcome.status, 'completed');
  assert.equal(events.filter((e) => e.type === 'run.finished').length, 1);
  assert(!raw.includes(marker), 'trace must redact file contents');
  const summary = {
    result: 'passed', timestamp: new Date().toISOString(), binary: relative(root, binary),
    elapsed_seconds: Number(((performance.now() - started) / 1000).toFixed(2)),
    model: events.find((e) => e.type === 'model.started').model, trace: relative(root, trace),
    model_calls: events.filter((e) => e.type === 'model.started').length,
    shell_calls: events.filter((e) => e.type === 'shell.started').length,
    usage: last.outcome.usage,
    checks: ['actual evidence in answer', 'shell executed', 'one completed outcome', 'redacted trace'],
  };
  await writeFile(`${trace}.summary.json`, `${JSON.stringify(summary, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  console.log(JSON.stringify(summary, null, 2));
} catch (error) {
  console.error('Live smoke failed.');
  // CLI errors are closed codes, never raw HTTP errors or response bodies.
  if (typeof error.stderr === 'string' && error.stderr) console.error(error.stderr.trim());
  else console.error(error.message);
  process.exitCode = 1;
} finally {
  await rm(workspace, { recursive: true, force: true });
}
