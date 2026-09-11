/** Matched offline full-preset startup, with alternating baseline/current order. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
// @ts-expect-error Dependency-free source fingerprint helper.
import { sourceFingerprint } from './lib/source-fingerprint.mjs';
const root = fileURLToPath(new URL('../', import.meta.url)), exec = promisify(execFile);
const binaries = { baseline: join(root, '.pablo/measurements/c3.32-baseline/pablo'), current: join(root, 'target/release/pablo') };
const bundle = join(root, 'presets/v1');
const manifest = JSON.parse(await readFile(join(bundle, 'manifest.json'), 'utf8'));
const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-g04-measure-')));
const sha = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
const stats = (samples: number[]) => {
  const sorted = samples.toSorted((a,b) => a-b);
  return { n: samples.length, p50: sorted[14], p95: sorted[28], max: sorted[29], samples };
};
const modes: Record<string, unknown> = {};
try {
  for (const preset of ['base', 'production', 'development']) {
    for (const command of ['explain', 'doctor']) {
      const samples = { baseline: [] as number[], current: [] as number[] };
      for (let i = 0; i < 35; i++) {
        for (const variant of (i % 2 ? ['current', 'baseline'] : ['baseline', 'current']) as (keyof typeof binaries)[]) {
          const started = performance.now();
          const { stdout } = await exec(binaries[variant], [...(command === 'doctor' ? ['doctor', '--json'] : ['config', 'explain']),
            '--config', join(bundle, `${preset}.toml`), '--locked', '--bind', `workspace=${cwd}`, '--bind', `preset=${bundle}`],
          { cwd, env: { OPENROUTER_API_KEY: 'synthetic-g04', AI_GATEWAY_API_KEY: 'synthetic-g04', PABLO_EVIDENCE_TOKEN: 'synthetic-g04' }, timeout: 5000, maxBuffer: 1024 * 1024 });
          const elapsed = performance.now() - started, result = JSON.parse(stdout);
          assert.equal(command === 'doctor' ? result.configuration.fingerprint : result.fingerprint, manifest.entries[preset].default.fingerprint);
          if (command === 'doctor') { assert.equal(result.exit_code, 0); assert.equal(result.probe_result, 'not probed'); assert(elapsed < 1000 && result.duration_ms < 1000); }
          if (i >= 5) samples[variant].push(elapsed);
        }
      }
      modes[`${preset}.${command}`] = { baseline_ms: stats(samples.baseline), current_ms: stats(samples.current) };
    }
  }
  assert.deepEqual(await readdir(cwd), [], 'offline full-preset inspection must create no traces or task effects');
  const report = { checkpoint: 'C3.32', timestamp: new Date().toISOString(), source_sha256: await sourceFingerprint(root),
    harness_sha256: sha(await readFile(fileURLToPath(import.meta.url))), platform: `${process.platform}/${process.arch}`,
    baseline_binary_sha256: sha(await readFile(binaries.baseline)), current_binary_sha256: sha(await readFile(binaries.current)),
    manifest_sha256: sha(await readFile(join(bundle, 'manifest.json'))),
    method: '30 fresh processes after 5 warmups per variant/preset/command; alternating baseline/current order; same complete versioned presets, synthetic credentials, no live probes or task effects. Warm local filesystem; measures offline configuration/doctor startup, not live provider or combined-task throughput.', modes };
  await writeFile(join(root, '.pablo/measurements/c3.32-presets.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
} finally { await rm(cwd, { recursive: true, force: true }); }
