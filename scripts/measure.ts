/** Reproducible Pablo release measurements. No live provider, no pass/fail performance ceilings. */
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { cpus, release, tmpdir, totalmem } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';
import { body, cleanEnv, server } from '../tests/fixtures/telemetry.ts';

// @ts-expect-error Dependency-free Node helper is JavaScript.
import { sourceFingerprint } from './lib/source-fingerprint.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const count = Number(process.argv[2] ?? 30);
assert(Number.isInteger(count) && count >= 10 && count <= 1000, 'sample count must be 10–1000');
const binary = resolve(process.env.PABLO_MEASURE_BINARY ?? join(root, 'target/release/pablo'));
const direct = resolve(process.env.PABLO_MEASURE_DIRECT ?? join(root, 'target/release/examples/measure'));
const reuse = process.env.PABLO_MEASURE_REUSE !== '0';
const destination = resolve(process.argv[3] ?? join(root, `.pablo/measurements/c1.7-${process.platform}-${process.arch}.json`));
const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-measure-')));
const env = cleanEnv();
const exec = promisify(execFile);
const summary = (samples: number[]) => {
  assert(samples.length > 0 && samples.every(n => Number.isFinite(n) && n >= 0));
  const sorted = samples.toSorted((a, b) => a - b);
  const at = (p: number) => sorted[Math.max(0, Math.ceil(p * sorted.length) - 1)];
  return { n: samples.length, min: sorted[0], p50: at(.5), p95: at(.95), p99: at(.99), max: sorted.at(-1)! };
};
async function processRun(path: string, args: string[], extraEnv: NodeJS.ProcessEnv = {}) {
  const start = performance.now();
  const child = spawn(path, args, { env: { ...env, ...extraEnv }, cwd });
  let stdout = ''; let stderr = '';
  child.stdout.on('data', b => { stdout += b; }); child.stderr.on('data', b => { stderr += b; });
  const timer = setTimeout(() => child.kill('SIGKILL'), 15000);
  try { const [code, signal] = await once(child, 'exit'); assert.equal(code, 0, stderr); assert.equal(signal, null); }
  finally { clearTimeout(timer); }
  return { stdout, wallMs: performance.now() - start };
}
let requestTimes: number[] = [];
let firstDeltaSentAt = 0;
const firstDeltaTask = "Measure first text delivery.";
const chunk = '0123456789abcdef';
const output = chunk.repeat(32);
const frame = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
const gateway = await server(async (req, res) => {
  requestTimes.push(performance.now());
  const request = JSON.parse((await body(req)).toString());
  assert.equal(request.model, 'fixture/measure');
  res.writeHead(200, { 'content-type': 'text/event-stream' });
  if (request.messages.at(-1).content === firstDeltaTask) {
    firstDeltaSentAt = performance.now();
    res.write(frame({ content: 'first' }));
    await delay(40); // Isolate delivery from terminal flushing and burst batching.
    res.end(frame({}, 'stop') + 'data: [DONE]\n\n');
  } else if (request.messages.at(-1).role === 'tool') {
    const result = JSON.parse(request.messages.at(-1).content);
    assert.equal(result.shell.stdout, 'measure'); assert.equal(result.shell.exit_code, 0);
    res.end(Array.from({ length: 32 }, () => frame({ content: chunk })).join('') + frame({}, 'stop') + 'data: [DONE]\n\n');
  } else {
    res.end(frame({ tool_calls: [{ index: 0, id: 'measure_call', type: 'function', function: {
      name: 'shell_run', arguments: JSON.stringify({ command: 'printf measure', cwd: '.' }),
    } }] }, 'tool_calls') + 'data: [DONE]\n\n');
  }
});
const endpoint = `${gateway.url}/v1/chat/completions`;
const task = 'Run the fixed measurement task.';
const options = ['--model', 'fixture/measure', '--max-model-calls', '2', '--max-tool-calls', '1'];
try {
  const measurements: Record<string, number[]> = Object.fromEntries([
    'version_process_ms', 'cli_provider_ready_ms', 'cli_total_ms', 'core_run_ms', 'core_host_total_ms',
    'acp_initialize_ms', 'acp_prompt_ms', 'acp_total_ms', 'acp_first_text_ms', 'idle_rss_kib', 'acp_first_delta_delivery_ms',
  ].map(name => [name, []]));
  for (let sample = 0; sample < count + 5; sample++) {
    const record = (name: string, value: number) => { if (sample >= 5) measurements[name].push(value); };
    const version = await processRun(binary, ['--version']);
    record('version_process_ms', version.wallMs);
    // Rotate independent paths to reduce systematic order bias.
    const paths = ['core', 'cli', 'acp', 'first_delta'];
    for (let index = 0; index < paths.length; index++) {
      const path = paths[(sample + index) % paths.length]; requestTimes = [];
      const started = performance.now();
      if (path === 'core') {
        const result = await processRun(direct, ['http', endpoint, task, '--workspace', cwd, ...options]);
        const parsed = JSON.parse(result.stdout);
        assert.equal(parsed.outcome.output, output); assert.equal(parsed.events, 41);
        record('core_run_ms', parsed.run_ms); record('core_host_total_ms', result.wallMs);
      } else if (path === 'cli') {
        const result = await processRun(binary, ['run', task, '--workspace', cwd, ...options], { PABLO_FIXTURE_ENDPOINT: endpoint });
        assert.equal(result.stdout, `${output}\n`);
        record('cli_provider_ready_ms', requestTimes[0] - started); record('cli_total_ms', result.wallMs);
      } else if (path === 'first_delta') {
        let deliveredAt: number | undefined;
        await withPablo({ binary, args: [...options, '--no-shell'], env: { ...env, PABLO_FIXTURE_ENDPOINT: endpoint },
          onUpdate: ({ update }) => { if (deliveredAt === undefined && update.sessionUpdate === 'agent_message_chunk') deliveredAt = performance.now(); },
        }, async cx => {
          await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true } } });
          const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
          const result = outcomeOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: firstDeltaTask }] }));
          assert(result.status === 'completed' && result.output === 'first');
        });
        assert(deliveredAt !== undefined);
        record('acp_first_delta_delivery_ms', deliveredAt - firstDeltaSentAt);
      } else {
        let pid = 0; let firstText: number | undefined; let promptStart = 0;
        let initializedAt = 0; let promptMs = 0; let measuredExit: number | null = null;
        await withPablo({ binary, args: options, env: { ...env, PABLO_FIXTURE_ENDPOINT: endpoint },
          onSpawn: child => { pid = child.pid!; child.once('exit', code => { measuredExit = code; }); },
          onUpdate: ({ update }) => { if (firstText === undefined && update.sessionUpdate === 'agent_message_chunk') firstText = performance.now(); },
        }, async cx => {
          const init = await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true } } });
          assert.equal(init.protocolVersion, 1); initializedAt = performance.now();
          // Idle after initialize, before session/prompt or provider creation.
          await delay(50);
          const rss = Number((await exec('ps', ['-o', 'rss=', '-p', String(pid)], { env })).stdout.trim());
          assert(rss > 0); record('idle_rss_kib', rss);
          const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
          promptStart = performance.now();
          const response = await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: task }] });
          promptMs = performance.now() - promptStart;
          const outcome = outcomeOf(response); assert(outcome.status === 'completed' && outcome.output === output);
        });
        assert.equal(measuredExit, 0); assert(firstText !== undefined);
        record('acp_initialize_ms', initializedAt - started);
        record('acp_prompt_ms', promptMs); record('acp_first_text_ms', firstText - promptStart);
        // Exclude deliberate idle sampling and session creation time from total.
        record('acp_total_ms', performance.now() - started - (promptStart - initializedAt));
      }
      assert.equal(requestTimes.length, path === 'first_delta' ? 1 : 2, `${path}: expected model calls`);
    }
  }
  if (reuse) {
    measurements.acp_warm_prompt_ms = [];
    measurements.acp_warm_first_text_ms = [];
    measurements.warm_idle_rss_kib = [];
    // Keep each connection below its 128-message lifetime bound, including warm-ups.
    for (let base = 0; base < count; base += 40) {
      let pid = 0; let firstText: number | undefined;
      await withPablo({ binary, args: options, env: { ...env, PABLO_FIXTURE_ENDPOINT: endpoint },
        onSpawn: child => { pid = child.pid!; },
        onUpdate: ({ update }) => { if (firstText === undefined && update.sessionUpdate === 'agent_message_chunk') firstText = performance.now(); },
      }, async cx => {
        await cx.request('initialize', { protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true } } });
        for (let index = 0; index < Math.min(40, count - base) + 5; index++) {
          const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
          requestTimes = []; firstText = undefined;
          const started = performance.now();
          const result = outcomeOf(await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: task }] }));
          const elapsed = performance.now() - started;
          assert(result.status === 'completed' && result.output === output);
          assert.equal(requestTimes.length, 2); assert(firstText !== undefined);
          if (index >= 5) {
            measurements.acp_warm_prompt_ms.push(elapsed);
            measurements.acp_warm_first_text_ms.push(firstText - started);
            const rss = Number((await exec('ps', ['-o', 'rss=', '-p', String(pid)], { env })).stdout.trim());
            assert(rss > 0); measurements.warm_idle_rss_kib.push(rss);
          }
        }
      });
    }
  }
  const events = JSON.parse((await processRun(direct, ['events', cwd, String(count)])).stdout);
  const stats = Object.fromEntries(Object.entries(measurements).map(([name, values]) => [name, summary(values)]));
  const stripped = join(cwd, "pablo-stripped");
  await copyFile(binary, stripped);
  await exec("strip", [stripped], { env });
  const report = {
    schema_version: 1, checkpoint: 'C1.7', timestamp: new Date().toISOString(), source_sha256: process.env.PABLO_MEASURE_SOURCE_SHA256 ?? await sourceFingerprint(root),
    harness_sha256: createHash('sha256').update(await readFile(fileURLToPath(import.meta.url))).digest('hex'),
    platform: { os: process.platform, arch: process.arch, kernel: release(), cpu: cpus()[0].model, logical_cpus: cpus().length, memory_bytes: totalmem(),
      node: process.version, rust: (await exec('rustc', ['--version'])).stdout.trim(),
      environment: process.env.PABLO_MEASURE_ENVIRONMENT ?? 'local host' },
    build: { profile: process.env.PABLO_MEASURE_BUILD ?? (await readFile(join(root, 'Cargo.toml'), 'utf8')).split('[profile.release]')[1].trim(), binary_bytes: (await stat(binary)).size, stripped_binary_bytes: (await stat(stripped)).size, strip_method: 'platform strip on a copy; timings use original release executable',
      binary_sha256: createHash('sha256').update(await readFile(binary)).digest('hex') },
    method: { samples: count, warmup: 5, cache: 'warm filesystem; no forced cache eviction',
      workload: 'two local HTTP/SSE calls, one real printf shell, 32 x 16-byte output deltas',
      startup: 'Node monotonic spawn to first loopback provider request arrival; separate --version process wall time',
      baseline: 'direct core run_with_tools in measurement host; SDK/provider/tool construction excluded from core_run_ms',
      acp: 'official TS SDK; one new process/session/prompt per sample; prompt includes per-run provider/tool/SDK setup; total excludes deliberate RSS wait and session creation',
      first_delta: 'server write of one text delta to TS client update; local HTTP/SSE and ACP; provider waits 40 ms before finishing',
      reuse: reuse ? 'up to 40 measured independent sessions per process after five warm-up tasks; prompt timing excludes session creation; same HTTP/shell workload' : 'not supported by selected baseline binary; omitted',
      rss: 'ps RSS in KiB 50 ms after ACP initialize; one fresh process per sample',
      events: 'normalized ProviderEvent yield to inline TextDelta sink entry; includes measurement mutex/clock; 1000 x 32-byte deltas per run',
      trace: 'same event workload with/without metadata-only buffered JSONL; includes writer flush but excludes file creation; no fsync',
      limits: 'single-run warm-cache microbenchmarks; no network/TLS/model latency; no concurrency or remote provider claim' },
    stats, events,
    comparisons: {
      acp_prompt_minus_core_p50_ms: stats.acp_prompt_ms.p50 - stats.core_run_ms.p50,
      acp_prompt_over_core_p50_ratio: stats.acp_prompt_ms.p50 / stats.core_run_ms.p50,
      acp_total_minus_core_host_p50_ms: stats.acp_total_ms.p50 - stats.core_host_total_ms.p50,
      trace_p50_overhead_percent: 100 * (events.trace_ms.p50 / events.no_trace_ms.p50 - 1),
    },
    samples: measurements,
  };
  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  console.log(JSON.stringify({ file: destination, platform: report.platform, binary_bytes: report.build.binary_bytes, stripped_binary_bytes: report.build.stripped_binary_bytes, stats, event_latency_ns: events.event_latency_ns,
    trace_ms: events.trace_ms, no_trace_ms: events.no_trace_ms, comparisons: report.comparisons }, null, 2));
} finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
