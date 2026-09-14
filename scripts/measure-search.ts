/** C3.34a fs.search baseline: validated offline corpora through the real ACP binary. */
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, open, readFile, readdir, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { cpus, release, tmpdir, totalmem } from 'node:os';
import { dirname, join, relative, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { taskOf, withPablo } from '../examples/acp-client.ts';
import { body, cleanEnv, server } from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import { sourceFingerprint } from './lib/source-fingerprint.mjs';

type SearchMatch = { path: string; line: number; text: string };
type Traversal = {
  entries: number;
  directories: number;
  regular_files: number;
  content_bytes: number;
  largest_file_bytes: number;
};
type Workload = {
  name: string;
  path: string;
  query: string;
  expected: SearchMatch[];
  traversal: Traversal;
  future_selected_content_bytes?: number;
};
type ResourceSample = { rss_kib: number; descriptors: number | null };

const exec = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const binary = resolve(process.env.PABLO_MEASURE_BINARY ?? join(root, 'target/release/pablo'));
const destination = resolve(process.argv[2] ?? join(root, `.pablo/measurements/c3.34a-search-${process.platform}-${process.arch}.json`));
const measuredSamples = parseCount('PABLO_SEARCH_SAMPLES', 30, 1, 100);
const warmups = parseCount('PABLO_SEARCH_WARMUP', 5, 0, 20);
const workspace = await realpath(await mkdtemp(join(tmpdir(), 'pablo-search-baseline-')));
const needle = '__pablo_search_needle__';
const frame = (delta: object, finish: string | null = null) =>
  `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
const finish = frame({}, 'stop') + 'data: [DONE]\n\n';

function parseCount(name: string, fallback: number, minimum: number, maximum: number): number {
  const raw = process.env[name];
  if (raw === undefined) return fallback;
  assert(/^\d+$/.test(raw), `${name} must be an integer`);
  const value = Number(raw);
  assert(Number.isSafeInteger(value) && value >= minimum && value <= maximum, `${name} is out of range`);
  return value;
}

function stats(values: number[]) {
  assert(values.length > 0);
  const sorted = values.toSorted((left, right) => left - right);
  const at = (percentile: number) => sorted[Math.max(0, Math.ceil(sorted.length * percentile) - 1)];
  return { n: sorted.length, min: sorted[0], p50: at(0.5), p95: at(0.95), max: sorted.at(-1) };
}

async function writeRepeated(path: string, pattern: Buffer, bytes: number) {
  const file = await open(path, 'wx', 0o600);
  try {
    let written = 0;
    while (written < bytes) {
      const length = Math.min(pattern.length, bytes - written);
      let offset = 0;
      while (offset < length) {
        const result = await file.write(pattern, offset, length - offset);
        assert(result.bytesWritten > 0, 'fixture write made no progress');
        offset += result.bytesWritten;
      }
      written += offset;
    }
  } finally {
    await file.close();
  }
}

function patternedChunk(bytes: number, line: string): Buffer {
  const source = Buffer.from(line);
  const result = Buffer.allocUnsafe(bytes);
  for (let offset = 0; offset < result.length; offset += source.length) {
    source.copy(result, offset, 0, Math.min(source.length, result.length - offset));
  }
  return result;
}

async function inspect(directory: string): Promise<Traversal> {
  const traversal: Traversal = { entries: 0, directories: 1, regular_files: 0, content_bytes: 0, largest_file_bytes: 0 };
  async function walk(path: string) {
    const entries = (await readdir(path, { withFileTypes: true })).toSorted((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
    for (const entry of entries) {
      traversal.entries++;
      const child = join(path, entry.name);
      if (entry.isDirectory()) {
        traversal.directories++;
        await walk(child);
      } else if (entry.isFile()) {
        const size = (await stat(child)).size;
        traversal.regular_files++;
        traversal.content_bytes += size;
        traversal.largest_file_bytes = Math.max(traversal.largest_file_bytes, size);
      }
    }
  }
  await walk(directory);
  return traversal;
}

async function buildCorpora(): Promise<Workload[]> {
  const workloads: Omit<Workload, 'traversal'>[] = [];

  const small = join(workspace, 'many-small');
  await mkdir(small);
  for (let index = 0; index < 512; index++) {
    const name = `${index}`.padStart(4, '0') + '.txt';
    const text = index === 511 ? `${needle}\n${'x'.repeat(2030)}\n` : `${'x'.repeat(2047)}\n`;
    await writeFile(join(small, name), text);
  }
  workloads.push({
    name: 'many_small_1m_late_match', path: 'many-small', query: needle,
    expected: [{ path: 'many-small/0511.txt', line: 1, text: needle }],
  });

  const large = join(workspace, 'large-short-lines');
  await mkdir(large);
  await writeRepeated(join(large, 'large.txt'), patternedChunk(1 << 20, 'ordinary line\n'), 256 << 20);
  workloads.push({ name: 'large_short_lines_256m_no_match', path: 'large-short-lines', query: needle, expected: [] });

  const long = join(workspace, 'long-line');
  await mkdir(long);
  await writeRepeated(join(long, 'long.txt'), Buffer.alloc(1 << 20, 'x'), 32 << 20);
  workloads.push({ name: 'long_line_32m_no_match', path: 'long-line', query: needle, expected: [] });

  const dense = join(workspace, 'dense-match');
  await mkdir(dense);
  const denseLines = Array.from({ length: 100 }, (_, index) => `${needle} result ${index}`);
  await writeFile(join(dense, 'matches.txt'), denseLines.join('\n') + '\n');
  workloads.push({
    name: 'dense_100_matches_one_file', path: 'dense-match', query: needle,
    expected: denseLines.map((text, index) => ({ path: 'dense-match/matches.txt', line: index + 1, text })),
  });

  const selected = join(workspace, 'selective-tree');
  await mkdir(join(selected, 'generated'), { recursive: true });
  await mkdir(join(selected, 'src'));
  const selectedChunk = patternedChunk(128 << 10, 'ordinary generated content\n');
  for (let index = 0; index < 90; index++) {
    await writeRepeated(join(selected, 'generated', `${index}`.padStart(3, '0') + '.txt'), selectedChunk, 128 << 10);
  }
  for (let index = 0; index < 10; index++) {
    const name = `${index}`.padStart(3, '0') + '.txt';
    const text = index === 9 ? `${needle}\n${'s'.repeat((128 << 10) - needle.length - 1)}` : 's'.repeat(128 << 10);
    await writeFile(join(selected, 'src', name), text);
  }
  workloads.push({
    name: 'selective_tree_12m_late_match', path: 'selective-tree', query: needle,
    expected: [{ path: 'selective-tree/src/009.txt', line: 1, text: needle }],
    future_selected_content_bytes: 10 * (128 << 10),
  });

  const deep = join(workspace, 'deep-wide');
  await mkdir(deep);
  for (let index = 0; index < 256; index++) {
    await writeFile(join(deep, `${index}`.padStart(3, '0') + '.txt'), 'ordinary\n');
  }
  let nested = deep;
  for (let depth = 0; depth < 40; depth++) {
    nested = join(nested, 'z');
    await mkdir(nested);
  }
  await writeFile(join(nested, 'match.txt'), `${needle}\n`);
  workloads.push({
    name: 'deep_40_wide_256_late_match', path: 'deep-wide', query: needle,
    expected: [{ path: `deep-wide/${'z/'.repeat(40)}match.txt`, line: 1, text: needle }],
  });

  return Promise.all(workloads.map(async workload => ({ ...workload, traversal: await inspect(join(workspace, workload.path)) })));
}

function validate(result: unknown, workload: Workload) {
  const tool = result as { status?: unknown; filesystem?: { kind?: unknown; path?: unknown; matches?: unknown; truncated?: unknown } };
  assert.equal(tool.status, 'completed');
  assert.equal(tool.filesystem?.kind, 'search');
  assert.equal(tool.filesystem.path, workload.path);
  assert.deepEqual(tool.filesystem.matches, workload.expected);
  assert.equal(tool.filesystem.truncated, false);
}

function parseCpuSeconds(value: string): number {
  const [dayPart, timePart] = value.includes('-') ? value.split('-', 2) : ['0', value];
  const units = timePart.split(':').map(Number);
  assert(units.length >= 2 && units.length <= 3 && units.every(Number.isFinite), `unexpected ps time ${value}`);
  let seconds = 0;
  for (const unit of units) seconds = seconds * 60 + unit;
  return Number(dayPart) * 86400 + seconds;
}

async function processAccounting(pid: number): Promise<{ rss_kib: number; cpu_seconds: number }> {
  const { stdout } = await exec('ps', ['-o', 'rss=', '-o', 'time=', '-p', String(pid)], { encoding: 'utf8' });
  const fields = stdout.trim().split(/\s+/);
  assert(fields.length === 2, `unexpected ps output for ${pid}`);
  return { rss_kib: Number(fields[0]), cpu_seconds: parseCpuSeconds(fields[1]) };
}

async function processSample(pid: number): Promise<ResourceSample> {
  const accounting = await processAccounting(pid);
  return { rss_kib: accounting.rss_kib, descriptors: await descriptorCount(pid) };
}

async function descriptorCount(pid: number): Promise<number | null> {
  try {
    if (process.platform === 'linux') return (await readdir(`/proc/${pid}/fd`)).length;
    if (process.platform === 'darwin') {
      const { stdout } = await exec('lsof', ['-a', '-p', String(pid), '-Fn'], { encoding: 'utf8', maxBuffer: 4 << 20 });
      return stdout.split('\n').filter(line => /^f(?:\d+|cwd|rtd|txt)$/.test(line)).length;
    }
  } catch {
    return null;
  }
  return null;
}

async function resourceProbe(pid: number, run: () => Promise<void>) {
  const idle = await processSample(pid);
  const observed: ResourceSample[] = [idle];
  let stopped = false;
  const polling = (async () => {
    while (!stopped) {
      await delay(20);
      try { observed.push(await processSample(pid)); } catch { break; }
    }
  })();
  try { await run(); } finally { stopped = true; await polling; }
  const peakRss = Math.max(...observed.map(sample => sample.rss_kib));
  const descriptors = observed.flatMap(sample => sample.descriptors === null ? [] : [sample.descriptors]);
  return {
    idle_rss_kib: idle.rss_kib,
    peak_rss_kib: peakRss,
    incremental_peak_rss_kib: Math.max(0, peakRss - idle.rss_kib),
    descriptor_high_water: descriptors.length ? Math.max(...descriptors) : null,
    samples: observed.length,
  };
}

const workloads = await buildCorpora();
let current = workloads[0];
let requests = 0;
let validatedResultBytes: number | undefined;
const gateway = await server(async (request, response) => {
  const payload = JSON.parse((await body(request)).toString());
  requests++;
  response.writeHead(200, { 'content-type': 'text/event-stream' });
  if (payload.messages.at(-1)?.role === 'tool') {
    const result = JSON.parse(payload.messages.at(-1).content);
    validate(result, current);
    validatedResultBytes = Buffer.byteLength(JSON.stringify(result));
    response.end(frame({ content: 'verified' }) + finish);
  } else {
    response.end(frame({ tool_calls: [{ index: 0, id: 'search_baseline', type: 'function', function: {
      name: 'fs_search', arguments: JSON.stringify({ path: current.path, query: current.query, max_matches: 100 }),
    } }] }, 'tool_calls') + 'data: [DONE]\n\n');
  }
});

try {
  const measurements: Record<string, unknown> = {};
  async function runOnce(workload: Workload, probeResources: boolean) {
    current = workload;
    requests = 0;
    validatedResultBytes = undefined;
    let child: ChildProcessWithoutNullStreams | undefined;
    let activeToolStarted: number | undefined;
    let activeToolMs: number | undefined;
    let diagnostics = '';
    return withPablo({
      binary,
      args: ['--no-shell', '--model', 'fixture/search-baseline', '--max-model-calls', '2', '--max-tool-calls', '1'],
      env: { ...cleanEnv(), PABLO_FIXTURE_ENDPOINT: `${gateway.url}/v1/chat/completions` },
      onSpawn: spawned => { child = spawned; },
      onDiagnostic: text => { diagnostics += text; },
      onUpdate: notification => {
        const meta = notification._meta?.['pablo/v2'] as { timestamp_unix_micros?: number } | undefined;
        const timestamp = meta?.timestamp_unix_micros;
        if (notification.update.sessionUpdate === 'tool_call' && typeof timestamp === 'number') {
          activeToolStarted = timestamp;
        }
        if (notification.update.sessionUpdate === 'tool_call_update' && notification.update.status === 'completed' &&
            typeof timestamp === 'number' && activeToolStarted !== undefined) {
          activeToolMs = (timestamp - activeToolStarted) / 1000;
        }
      },
    }, async context => {
      const initialized = await context.request('initialize', {
        protocolVersion: 1,
        clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } },
        clientInfo: { name: 'pablo-search-baseline', version: 'c3.34a' },
      });
      assert.equal(initialized.agentCapabilities?._meta?.['pablo/v2'], true);
      assert(child?.pid);
      const pid = child.pid;
      const { sessionId } = await context.request('session/new', { cwd: workspace, mcpServers: [] });
      const cpuBefore = (await processAccounting(pid)).cpu_seconds;
      let elapsed = 0;
      const execute = async () => {
        const started = performance.now();
        const response = await context.request('session/prompt', {
          sessionId,
          prompt: [{ type: 'text', text: 'Run the fixed filesystem search baseline.' }],
        });
        elapsed = performance.now() - started;
        const task = taskOf(response);
        assert.equal(task.outcome.status, 'completed');
        assert.equal(task.accounting.model_calls, '2');
        assert.equal(task.accounting.tool_calls, '1');
        assert.equal(requests, 2);
      };
      const resources = probeResources ? await resourceProbe(pid, execute) : undefined;
      if (!probeResources) await execute();
      const cpuAfter = (await processAccounting(pid)).cpu_seconds;
      assert.equal(diagnostics, '');
      assert(activeToolMs !== undefined && activeToolMs >= 0, 'missing native tool duration');
      assert(validatedResultBytes !== undefined && validatedResultBytes > 0, 'missing serialized tool result size');
      const measuredToolMs = activeToolMs;
      const measuredResultBytes = validatedResultBytes;
      return {
        prompt_ms: elapsed,
        tool_ms: measuredToolMs,
        process_cpu_ms: Math.max(0, cpuAfter - cpuBefore) * 1000,
        result_bytes: measuredResultBytes,
        resources,
      };
    });
  }

  for (const workload of workloads) {
    for (let index = 0; index < warmups; index++) await runOnce(workload, false);
    const promptMs: number[] = [];
    const toolMs: number[] = [];
    const processCpuMs: number[] = [];
    const resultBytes = new Set<number>();
    for (let index = 0; index < measuredSamples; index++) {
      const sample = await runOnce(workload, false);
      promptMs.push(sample.prompt_ms);
      toolMs.push(sample.tool_ms);
      processCpuMs.push(sample.process_cpu_ms);
      resultBytes.add(sample.result_bytes);
    }
    const probe = await runOnce(workload, true);
    resultBytes.add(probe.result_bytes);
    assert.equal(resultBytes.size, 1, 'deterministic result size changed across samples');
    measurements[workload.name] = {
      prompt_ms: stats(promptMs),
      tool_ms: stats(toolMs),
      process_cpu_ms: stats(processCpuMs),
      prompt_samples_ms: promptMs,
      tool_samples_ms: toolMs,
      process_cpu_samples_ms: processCpuMs,
      serialized_tool_result_bytes: [...resultBytes][0],
      resources: probe.resources,
      traversal: workload.traversal,
      future_selected_content_bytes: workload.future_selected_content_bytes,
    };
  }

  const binaryBytes = await readFile(binary);
  const harnessBytes = await readFile(fileURLToPath(import.meta.url));
  const report = {
    checkpoint: 'C3.34a',
    timestamp: new Date().toISOString(),
    platform: { os: process.platform, arch: process.arch, kernel: release(), cpu: cpus()[0]?.model ?? 'unknown', logical_cpus: cpus().length, memory_bytes: totalmem() },
    source_sha256: await sourceFingerprint(root),
    binary_sha256: createHash('sha256').update(binaryBytes).digest('hex'),
    binary_bytes: binaryBytes.length,
    harness_sha256: createHash('sha256').update(harnessBytes).digest('hex'),
    method: {
      measured_samples: measuredSamples,
      warmups,
      cache: 'warm filesystem; no forced eviction',
      task: 'one fresh ACP process and session per sample; two loopback HTTP/SSE model calls and one actual fs.search',
      timing: 'client session/prompt request through negotiated terminal response; native tool start-to-finish timestamps; fixture creation and session/new excluded',
      cpu: 'target process ps CPU delta around each prompt; resolution is platform ps resolution',
      memory: 'separate validated probe sampled target-process RSS every approximately 20 ms; incremental peak subtracts pre-probe RSS',
      descriptors: 'same probe; /proc/<pid>/fd on Linux or lsof on macOS; null when unavailable',
      traversal: 'known fixture metadata for the current exhaustive walk; entries include files and descendant directories, directories includes search root',
      scanner_retention_model: 'current implementation retains the complete candidate file; largest_file_bytes is the lower-bound content term, while measured RSS includes the full runtime',
      result_validation: 'every tool result is checked by the loopback provider before the final model response',
      privacy: 'report contains aggregate fixture sizes and result counts, without temporary paths or file contents',
    },
    measurements,
  };
  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, JSON.stringify(report, null, 2) + '\n');
  process.stdout.write(JSON.stringify({
    destination: relative(root, destination),
    checkpoint: report.checkpoint,
    samples: measuredSamples,
    warmups,
    measurements: Object.fromEntries(Object.entries(measurements).map(([name, value]) => {
      const item = value as { prompt_ms: unknown; tool_ms: unknown; resources: unknown; traversal: unknown };
      return [name, { prompt_ms: item.prompt_ms, tool_ms: item.tool_ms, resources: item.resources, traversal: item.traversal }];
    })),
  }) + '\n');
} finally {
  await gateway.close();
  await rm(workspace, { recursive: true, force: true });
}
