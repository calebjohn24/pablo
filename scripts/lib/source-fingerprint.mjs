import { createHash } from 'node:crypto';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';

/** Covers executable, fixtures, toolchain and measurement code, not changing handoff records. */
export async function sourceFingerprint(root) {
  const paths = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'package.json', 'package-lock.json', 'tsconfig.json', '.nvmrc',
    'LICENSE', 'NOTICE', 'install.sh',
    'docs/project/schemas/deployment-v1.schema.json', 'docs/project/schemas/resolved-deployment-v1.schema.json',
    'docs/project/schemas/deployment-defaults-v1.json', 'docs/acp-lock.json', 'docs/protocol-compatibility.json', 'docs/protocol-pins.json', 'docs/acp-extensions.json',
    'docs/pablo-acp-v1.schema.json', 'docs/pablo-acp-v2.schema.json', 'docs/pablo-task.schema.json',
    'docs/pablo-acp-notifications-v1.schema.json'];
  async function walk(path) {
    for (const entry of await readdir(join(root, path), { withFileTypes: true })) {
      if (entry.name === '__pycache__') continue;
      const child = `${path}/${entry.name}`;
      if (entry.isDirectory()) await walk(child);
      else if (entry.isFile()) paths.push(child);
      else throw new Error(`Unexpected non-file in source tree: ${child}`);
    }
  }
  for (const path of ['crates', 'examples', 'scripts', 'tests', 'vendor', 'presets', 'docs/project/fixtures/c3-compatibility', 'docs/project/fixtures/c3-open-responses', 'docs/project/fixtures/c3-deployment']) await walk(path);
  const hash = createHash('sha256');
  for (const path of paths.sort()) { hash.update(path); hash.update('\0'); hash.update(await readFile(join(root, path))); hash.update('\0'); }
  return hash.digest('hex');
}
