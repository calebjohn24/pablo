import { createHash } from 'node:crypto';
import { readFile, writeFile, mkdir, rename } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

export const lock = JSON.parse(await readFile(new URL('../../tests/fixtures/collector/lock.json', import.meta.url), 'utf8'));
export async function collectorBinary() {
  const key = `${process.platform}_${process.arch === 'x64' ? 'amd64' : process.arch}`;
  const asset = lock.assets[key];
  if (!asset) throw new Error(`Collector fixture does not support ${key}`);
  const cache = fileURLToPath(new URL(`../../.cache/collector/${lock.version}/${key}/`, import.meta.url));
  await mkdir(cache, { recursive: true });
  const archive = join(cache, 'collector.tar.gz');
  let bytes;
  try { bytes = await readFile(archive); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (!bytes) {
    process.stderr.write(`Downloading pinned OpenTelemetry Collector ${lock.version} for ${key}\n`);
    const response = await fetch(asset.url, { signal: AbortSignal.timeout(120000) });
    if (!response.ok) throw new Error(`Collector download failed: HTTP ${response.status}`);
    bytes = Buffer.from(await response.arrayBuffer());
  }
  if (createHash('sha256').update(bytes).digest('hex') !== asset.sha256) throw new Error('Collector archive checksum mismatch');
  await writeFile(`${archive}.tmp`, bytes, { mode: 0o600 });
  await rename(`${archive}.tmp`, archive);
  // Re-extract from the verified archive, rather than trusting a cached executable.
  await promisify(execFile)('tar', ['-xzf', archive, '-C', cache, 'otelcol']);
  return join(cache, 'otelcol');
}
