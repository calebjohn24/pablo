#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { lstatSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { basename, join, resolve, sep } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { TARGETS } from './package.mjs';

function fail(message) {
  throw new Error(message);
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function parseArgs(argv) {
  const values = {};
  let run = false;
  for (let index = 0; index < argv.length; index += 1) {
    const name = argv[index];
    if (name === '--run') {
      run = true;
      continue;
    }
    if (!name.startsWith('--') || !argv[index + 1]) fail(`invalid argument: ${name}`);
    values[name.slice(2)] = argv[index + 1];
    index += 1;
  }
  return { values, run };
}

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

function inspectTree(root) {
  const files = [];
  function walk(path) {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const child = join(path, entry.name);
      const stat = lstatSync(child);
      if (stat.isSymbolicLink()) fail(`archive contains a symlink: ${child}`);
      if (stat.isDirectory()) walk(child);
      else if (stat.isFile()) files.push(child);
      else fail(`archive contains a non-file entry: ${child}`);
    }
  }
  walk(root);
  return files;
}

export function verifyRelease({ version, target, archive, checksum, run = false }) {
  if (!TARGETS[target]) fail(`unsupported release target: ${target}`);
  const archiveName = `pablo-${version}-${target}.tar.gz`;
  if (basename(archive) !== archiveName) fail(`archive filename must be ${archiveName}`);
  if (basename(checksum) !== `${archiveName}.sha256`) fail(`checksum filename must be ${archiveName}.sha256`);
  const checksumFields = readFileSync(checksum, 'utf8').trim().split(/\s+/);
  if (checksumFields.length !== 2 || checksumFields[1] !== archiveName || !/^[0-9a-f]{64}$/.test(checksumFields[0])) {
    fail('checksum file must contain the archive SHA-256 and exact filename');
  }
  const archiveHash = sha256(readFileSync(archive));
  if (archiveHash !== checksumFields[0]) fail('archive checksum mismatch');

  const workdir = mkdtempSync(join(tmpdir(), 'pablo-release-verify-'));
  try {
    const listing = execFileSync('tar', ['-tzf', archive], { encoding: 'utf8' }).trim().split(/\r?\n/);
    const rootName = `pablo-${version}-${target}`;
    if (listing.length === 0 || listing.some((path) => path.startsWith('/') || path.split('/').includes('..'))) {
      fail('archive contains an unsafe path');
    }
    if (listing.some((path) => path !== rootName && !path.startsWith(`${rootName}/`))) {
      fail('archive contains an entry outside its versioned root');
    }
    execFileSync('tar', ['-xzf', archive, '-C', workdir]);
    const root = join(workdir, rootName);
    const files = inspectTree(root);
    const required = [
      'bin/pablo',
      'NOTICE',
      'THIRD_PARTY_NOTICES.json',
      'manifest/source.json',
      'manifest/build.json',
      'manifest/target.json',
    ];
    for (const path of required) {
      if (!files.includes(join(root, path))) fail(`archive is missing ${path}`);
    }

    const source = readJson(join(root, 'manifest/source.json'));
    const build = readJson(join(root, 'manifest/build.json'));
    const targetManifest = readJson(join(root, 'manifest/target.json'));
    if (source.version !== version || source.format !== 1) fail('source manifest identity mismatch');
    if (!/^[0-9a-f]{40}$/.test(source.revision) || !/^[0-9a-f]{40}$/.test(source.git_tree)) {
      fail('source manifest does not contain Git object identities');
    }
    if (!/^[0-9a-f]{64}$/.test(source.source_fingerprint_sha256)
      || !/^[0-9a-f]{64}$/.test(source.cargo_lock_sha256)) {
      fail('source manifest does not contain SHA-256 identities');
    }
    if (build.format !== 1 || build.profile !== 'release' || build.locked_dependencies !== true) {
      fail('build manifest does not describe the locked release profile');
    }
    const binary = readFileSync(join(root, 'bin/pablo'));
    if (build.binary_sha256 !== sha256(binary)) fail('binary hash does not match the build manifest');
    if (targetManifest.format !== 1 || targetManifest.triple !== target) fail('target manifest identity mismatch');
    if (targetManifest.native_execution_accepted !== false) {
      fail('new package must leave native acceptance to its target checkpoint');
    }
    if (!Array.isArray(targetManifest.runtime_requirements) || targetManifest.runtime_requirements.length === 0) {
      fail('target manifest is missing runtime requirements');
    }

    const notices = readJson(join(root, 'THIRD_PARTY_NOTICES.json'));
    if (notices.format !== 1 || !Array.isArray(notices.packages) || notices.packages.length === 0) {
      fail('third-party notice inventory is empty');
    }
    for (const pkg of notices.packages) {
      if (!pkg.name || !pkg.version || (!pkg.license_expression && pkg.bundled_license_files.length === 0)) {
        fail('third-party package lacks license identity');
      }
      for (const license of pkg.bundled_license_files) {
        const licensePath = resolve(root, license.archive_path);
        if (!licensePath.startsWith(`${root}${sep}`)) fail('license path escapes the archive root');
        if (sha256(readFileSync(licensePath)) !== license.sha256) fail(`license hash mismatch for ${pkg.name}`);
      }
    }

    let versionOutput = null;
    if (run) {
      versionOutput = execFileSync(join(root, 'bin/pablo'), ['--version'], {
        encoding: 'utf8',
        env: { PATH: '/usr/bin:/bin' },
      }).trim();
      const number = version.slice(1);
      if (versionOutput !== `pablo ${number}` && !versionOutput.startsWith(`pablo ${number} `)) {
        fail(`binary version mismatch: ${versionOutput}`);
      }
    }
    return {
      archive: archiveName,
      archive_sha256: archiveHash,
      binary_sha256: build.binary_sha256,
      source_revision: source.revision,
      target,
      third_party_packages: notices.packages.length,
      native_execution: run,
      version_output: versionOutput,
    };
  } finally {
    rmSync(workdir, { recursive: true, force: true });
  }
}

function main() {
  const { values, run } = parseArgs(process.argv.slice(2));
  for (const required of ['version', 'target', 'archive', 'checksum']) {
    if (!values[required]) fail(`--${required} is required`);
  }
  const result = verifyRelease({
    version: values.version,
    target: values.target,
    archive: resolve(values.archive),
    checksum: resolve(values.checksum),
    run,
  });
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`release verify: ${error.message}\n`);
    process.exitCode = 1;
  }
}
