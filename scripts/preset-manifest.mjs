/** Offline bundle provenance. No environment files, credentials or live calls. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const bundle = join(root, 'presets/v1'), exec = promisify(execFile);
const binary = process.env.PABLO_PRESET_BINARY ?? join(root, 'target/debug/pablo');
assert(process.argv.length === 2 || (process.argv.length === 3 && process.argv[2] === '--write'));
const sha = data => createHash('sha256').update(data).digest('hex');
const files = {};
async function walk(relative = '') {
  for (const entry of (await readdir(join(bundle, relative), { withFileTypes: true })).sort((a,b) => a.name.localeCompare(b.name))) {
    const path = relative ? `${relative}/${entry.name}` : entry.name;
    if (entry.isDirectory()) await walk(path);
    else if (entry.isFile() && path !== 'manifest.json') files[path] = sha(await readFile(join(bundle, path)));
    else assert(entry.isFile(), 'bundle must contain only regular files');
  }
}
await walk();
const entries = {};
for (const entry of ['base', 'production', 'development']) {
  entries[entry] = {};
  for (const profile of ['default', 'observed']) {
    const { stdout } = await exec(binary, ['config', 'explain', '--config', join(bundle, `${entry}.toml`), '--locked',
      '--bind', `workspace=${root}`, '--bind', `preset=${bundle}`, ...(profile === 'observed' ? ['--profile', profile] : [])], { env: {}, timeout: 5000 });
    const resolved = JSON.parse(stdout);
    entries[entry][profile] = { fingerprint: resolved.fingerprint, input_fingerprint: resolved.input_fingerprint };
  }
}
const manifest = { bundle_version: 1, deployment_schema_version: 1, contract_revision: 'c3.26',
  target_gates: ['C3.35', 'C3.36', 'C3.37', 'C3.38'], files, entries };
const path = join(bundle, 'manifest.json');
if (process.argv[2] === '--write') await writeFile(path, JSON.stringify(manifest, null, 2) + '\n');
else assert.deepEqual(JSON.parse(await readFile(path, 'utf8')), manifest, 'bundle changed: review then run node scripts/preset-manifest.mjs --write');
console.log(`Preset v1 manifest ${process.argv[2] === '--write' ? 'written' : 'verified'}: ${Object.keys(files).length} files, 6 configurations.`);
