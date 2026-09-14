import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import test from 'node:test';
import { buildReleaseIndex } from './index.mjs';
import { packageRelease, TARGETS } from './package.mjs';
import { verifyRelease } from './verify.mjs';

const REVISION = '1'.repeat(40);
const TREE = '2'.repeat(40);
const FINGERPRINT = '3'.repeat(64);
const HOST_TARGET = ({
  'darwin:arm64': 'aarch64-apple-darwin',
  'darwin:x64': 'x86_64-apple-darwin',
  'linux:arm64': 'aarch64-unknown-linux-gnu',
  'linux:x64': 'x86_64-unknown-linux-gnu',
})[`${process.platform}:${process.arch}`];

function fixtureMetadata(root) {
  const dependency = join(root, 'dependency');
  mkdirSync(dependency);
  writeFileSync(join(dependency, 'Cargo.toml'), '[package]\nname="fixture-dependency"\nversion="1.2.3"\n');
  writeFileSync(join(dependency, 'LICENSE-MIT'), 'fixture dependency license\n');
  return {
    packages: [
      { id: 'path+pablo', name: 'pablo', version: '0.0.1', manifest_path: join(root, 'pablo-Cargo.toml') },
      {
        id: 'registry+fixture',
        name: 'fixture-dependency',
        version: '1.2.3',
        license: 'MIT',
        license_file: null,
        authors: ['Fixture Author'],
        repository: 'https://example.invalid/fixture',
        source: 'registry+https://example.invalid/index',
        manifest_path: join(dependency, 'Cargo.toml'),
      },
    ],
    workspace_members: ['path+pablo'],
    resolve: {
      nodes: [
        { id: 'path+pablo', deps: [{ pkg: 'registry+fixture', dep_kinds: [{ kind: null }] }] },
        { id: 'registry+fixture', deps: [] },
      ],
    },
  };
}

test('release packaging is deterministic, complete, checksummed, installable, and removable', async () => {
  assert(HOST_TARGET, `unsupported test host: ${process.platform} ${process.arch}`);
  const root = mkdtempSync(join(tmpdir(), 'pablo-package-test-'));
  try {
    const binary = join(root, 'pablo');
    writeFileSync(binary, '#!/bin/sh\nprintf "pablo 0.0.1\\n"\n', { mode: 0o755 });
    const metadata = fixtureMetadata(root);
    const common = {
      version: 'v0.0.1',
      target: HOST_TARGET,
      binary,
      sourceRevision: REVISION,
      sourceTree: TREE,
      sourceEpoch: '1700000000',
      sourceFingerprint: FINGERPRINT,
      rustc: ['rustc fixture', 'host: aarch64-apple-darwin'],
      cargo: 'cargo fixture',
      node: 'v24.20.0',
      runner: 'fixture',
      native: true,
      allowDirty: true,
      metadata,
    };
    const candidates = join(root, 'candidates');
    const first = await packageRelease({ ...common, outDir: candidates });
    const second = await packageRelease({ ...common, outDir: join(root, 'repeat') });
    assert.deepEqual(readFileSync(first.archivePath), readFileSync(second.archivePath));
    assert.deepEqual(readFileSync(first.checksumPath), readFileSync(second.checksumPath));

    const result = verifyRelease({
      version: common.version,
      target: common.target,
      archive: first.archivePath,
      checksum: first.checksumPath,
      run: true,
    });
    assert.equal(result.source_revision, REVISION);
    assert.equal(result.third_party_packages, 1);
    assert.equal(result.version_output, 'pablo 0.0.1');

    for (const target of Object.keys(TARGETS).filter((candidate) => candidate !== HOST_TARGET)) {
      await packageRelease({ ...common, target, outDir: candidates });
    }
    const release = await buildReleaseIndex({
      version: common.version,
      inputDir: candidates,
      outDir: join(root, 'release'),
    });
    assert.equal(release.artifacts.length, 4);
    assert.equal(release.source.revision, REVISION);
    assert.match(readFileSync(join(root, 'release', 'SHA256SUMS'), 'utf8'), /aarch64-apple-darwin/);

    const prefix = join(root, 'prefix');
    const env = { PATH: '/usr/bin:/bin', TMPDIR: join(root, 'tmp') };
    mkdirSync(env.TMPDIR);
    execFileSync('sh', [join(process.cwd(), 'install.sh'), '--version', common.version, '--prefix', prefix,
      '--archive', first.archivePath, '--checksum', first.checksumPath], { env });
    assert.equal(execFileSync(join(prefix, 'bin/pablo'), ['--version'], { encoding: 'utf8', env }).trim(), 'pablo 0.0.1');
    execFileSync('sh', [join(process.cwd(), 'install.sh'), '--prefix', prefix, '--remove'], { env });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('release verification rejects a corrupt checksum before extraction', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pablo-package-corrupt-'));
  try {
    const binary = join(root, 'pablo');
    writeFileSync(binary, '#!/bin/sh\nprintf "pablo 0.0.1\\n"\n', { mode: 0o755 });
    const result = await packageRelease({
      version: 'v0.0.1', target: 'aarch64-apple-darwin', binary, outDir: join(root, 'dist'),
      sourceRevision: REVISION, sourceTree: TREE, sourceEpoch: '1700000000', sourceFingerprint: FINGERPRINT,
      rustc: ['fixture'], cargo: 'fixture', node: 'fixture', allowDirty: true, metadata: fixtureMetadata(root),
    });
    writeFileSync(result.checksumPath, `${'0'.repeat(64)}  ${result.archiveName}\n`);
    assert.throws(() => verifyRelease({
      version: 'v0.0.1', target: 'aarch64-apple-darwin', archive: result.archivePath,
      checksum: result.checksumPath,
    }), /archive checksum mismatch/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
