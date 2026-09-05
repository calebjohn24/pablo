/** Explicit paid C1.4 acceptance; never imported by ordinary tests. */
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { RequestError, type SessionNotification } from '@agentclientprotocol/sdk';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [binaryArg = 'target/debug/pablo', model, ...extra] = process.argv.slice(2);
if (extra.length || (model !== undefined && !/^[\w.-]+\/[\w./:-]+$/.test(model))) {
  console.error('Usage: node scripts/smoke-live-acp.ts [BINARY] [PROVIDER/MODEL]');
  process.exit(2);
}
const binary = resolve(root, binaryArg);
const trace = join(root, '.pablo', 'traces', `c1.4-live-acp-${randomUUID()}.jsonl`);
const env = { ...process.env };
// Only Pablo reads the root credential file, privately. Never source it or let
// an inherited fixture endpoint/environment key substitute for live acceptance.
delete env.PABLO_FIXTURE_ENDPOINT;
delete env.AI_GATEWAY_API_KEY;
delete env.VERCEL_AI_GATEWAY;
let workspace: string | undefined;
let stage = 'setup';
try {
  workspace = await realpath(await mkdtemp(join(tmpdir(), 'pablo-live-acp-')));
  const marker = `pablo-evidence-${randomUUID()}`;
  await writeFile(join(workspace, 'evidence.txt'), `${marker}\n`, { mode: 0o600 });
  await mkdir(dirname(trace), { recursive: true, mode: 0o700 });
  const updates: SessionNotification[] = [];
  let raw = ''; let diagnostics = ''; let exitCode: number | null = null;
  let exitSignal: string | null = null;
  const started = performance.now();
  stage = 'live ACP request';
  const response = await withPablo({
    binary, env,
    args: ['--env-file', join(root, '.env'), '--trace', trace,
      '--timeout', '90', '--tool-timeout', '10', '--max-tool-calls', '1', '--max-model-calls', '2',
      ...(model ? ['--model', model] : [])],
    onUpdate: notification => { updates.push(notification); },
    onDiagnostic: text => { diagnostics = (diagnostics + text).slice(-65536); },
    onSpawn: child => {
      // Bound even initialization/transport failures outside the run deadline.
      const terminate = setTimeout(() => child.kill('SIGTERM'), 100_000);
      const kill = setTimeout(() => child.kill('SIGKILL'), 110_000);
      child.once('error', () => { clearTimeout(terminate); clearTimeout(kill); });
      child.once('exit', (code, signal) => {
        exitCode = code; exitSignal = signal;
        clearTimeout(terminate); clearTimeout(kill);
      });
      child.stdout.on('data', (chunk: Buffer) => {
        if (raw.length + chunk.length > 8 * 1024 * 1024) child.kill('SIGTERM');
        else raw += chunk.toString();
      });
    },
  }, async cx => {
    const init = await cx.request('initialize', {
      protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true } },
      clientInfo: { name: 'pablo-live-acceptance', version: 'c1.4' },
    });
    assert.equal(init.protocolVersion, 1);
    assert.equal(init.agentCapabilities?._meta?.['pablo/v1'], true);
    const { sessionId } = await cx.request('session/new', { cwd: workspace!, mcpServers: [] });
    return cx.request('session/prompt', {
      sessionId, prompt: [{ type: 'text', text:
        'Use exactly one shell call to run cat evidence.txt with cwd \".\". Then return the exact file contents and nothing else. Do not read any other files.' }],
    });
  });
  stage = 'live outcome and tool evidence';
  assert.equal(exitCode, 0);
  assert.equal(exitSignal, null);
  assert.equal(response.stopReason, 'end_turn');
  const outcome = outcomeOf(response);
  assert.equal(outcome.status, 'completed');
  assert(outcome.status === 'completed');
  assert(outcome.output.includes(marker), 'final answer contains unseen file evidence');
  const streamed = updates.flatMap(({ update }) => update.sessionUpdate === 'agent_message_chunk'
    && update.content.type === 'text' ? [update.content.text] : []).join('');
  assert.equal(streamed, outcome.output);
  const calls = updates.filter(({ update }) => update.sessionUpdate === 'tool_call');
  assert.equal(calls.length, 1);
  const results = updates.flatMap(({ update }) => update.sessionUpdate === 'tool_call_update'
    && update.status === 'completed' ? [update.rawOutput as any] : []);
  assert.equal(results.length, 1);
  assert.equal(results[0].shell.stdout, `${marker}\n`);
  assert.equal(results[0].shell.exit_code, 0);

  stage = 'native trace, usage and cleanup';
  const traceRaw = await readFile(trace, 'utf8');
  const records = traceRaw.trim().split('\n').map(line => JSON.parse(line));
  const last = records.at(-1);
  assert.equal(last.type, 'run.finished');
  assert.equal(last.outcome.status, 'completed');
  assert.equal(records.filter(e => e.type === 'run.finished').length, 1);
  const models = records.filter(e => e.type === 'model.started');
  const finished = records.filter(e => e.type === 'model.finished');
  const shells = records.filter(e => e.type === 'shell.started');
  assert.equal(models.length, 2);
  assert.equal(finished.length, 2);
  assert.equal(shells.length, 1);
  assert(models.every(e => e.provider === 'vercel' && e.model === (model ?? 'google/gemini-3.8-flash')));
  assert(models[0].seq < shells[0].seq && shells[0].seq < models[1].seq);
  for (const shell of shells) {
    assert.throws(() => process.kill(shell.process_id, 0), { code: 'ESRCH' });
    assert.throws(() => process.kill(-shell.process_id, 0), { code: 'ESRCH' });
  }
  for (const field of ['input_tokens', 'output_tokens', 'cache_read_input_tokens', 'cache_write_input_tokens'] as const) {
    const counts = finished.map(e => e.usage[field]);
    assert(counts.every(n => n === null || (Number.isSafeInteger(n) && n >= 0)));
    assert.equal(outcome.usage[field], counts.some(n => n === null) ? null : counts.reduce((a, b) => a + b, 0));
  }
  assert(outcome.usage.input_tokens !== null && outcome.usage.input_tokens > 0);
  assert(outcome.usage.output_tokens !== null && outcome.usage.output_tokens > 0);
  assert.deepEqual(last.outcome.usage, outcome.usage);
  let previous = 0;
  for (const notification of updates) {
    const meta = notification._meta?.['pablo/v1'] as any;
    assert(meta.seq_start > previous && meta.seq_end >= meta.seq_start);
    previous = meta.seq_end;
    const native = records.find(e => e.seq === meta.seq_end);
    assert(native);
    for (const field of ['run_id', 'session_id', 'trace_id', 'span_id', 'trace_flags']) assert.equal(meta[field], native[field]);
  }
  const terminal = response._meta?.['pablo/v1'] as any;
  assert.equal(terminal.trace_id, last.trace_id);
  assert.equal(terminal.run_id, last.run_id);
  assert(!traceRaw.includes(marker), 'native trace redacts evidence');
  assert(!diagnostics.includes(marker), 'diagnostics exclude evidence');
  assert.equal((await stat(trace)).mode & 0o777, 0o600);
  const messages = raw.trim().split('\n').map(line => JSON.parse(line));
  assert(messages.every(m => m.jsonrpc === '2.0'));
  assert.equal(messages.filter(m => m.result?.stopReason).length, 1);
  assert.equal(messages.at(-1).result.stopReason, 'end_turn');
  await rm(workspace, { recursive: true, force: true });
  await assert.rejects(stat(workspace), { code: 'ENOENT' });
  workspace = undefined;

  const summary = {
    result: 'passed', checkpoint: 'C1.4', timestamp: new Date().toISOString(),
    binary: relative(root, binary), model: models[0].model, provider: models[0].provider,
    credential_source: 'root .env parsed privately by executable',
    elapsed_seconds: Number(((performance.now() - started) / 1000).toFixed(2)),
    trace: relative(root, trace), model_calls: models.length, shell_calls: shells.length,
    usage: outcome.usage, per_call_usage: finished.map(e => e.usage),
    checks: ['ACP v1 negotiation', 'model/shell/model with actual temporary evidence',
      'streamed answer matches typed outcome', 'reported usage sums across calls',
      'ordered ACP/native correlation', 'one completed outcome and protocol-only stdout',
      'private redacted trace', 'shell process and group gone', 'ACP process exited cleanly', 'workspace removed'],
  };
  await writeFile(`${trace}.summary.json`, `${JSON.stringify(summary, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  console.log(JSON.stringify(summary, null, 2));
} catch (error) {
  // Never print SDK errors, assertions, model output, HTTP bodies, or stderr.
  // Only allow the runtime's closed failure fields into a diagnostic summary.
  const native = error instanceof RequestError ? (error.data as any)?.['pablo/v1']?.outcome : undefined;
  const code = ['provider_rejected', 'provider_transport', 'malformed_stream'].includes(native?.code) ? native.code : undefined;
  console.error(JSON.stringify({ result: 'failed', stage, code, trace: relative(root, trace) }));
  process.exitCode = 1;
} finally {
  if (workspace) await rm(workspace, { recursive: true, force: true });
}
