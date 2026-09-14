#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  chmodSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';
import { sourceFingerprint } from '../lib/source-fingerprint.mjs';

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = resolve(SCRIPT_DIR, '../..');

export const TARGETS = Object.freeze({
  'aarch64-apple-darwin': {
    architecture: 'arm64',
    operating_system: 'macOS',
    binary_format: 'Mach-O',
    runtime_requirements: ['macOS 11.0 or newer'],
    signing: 'Unsigned and not notarized',
  },
  'x86_64-apple-darwin': {
    architecture: 'x86_64',
    operating_system: 'macOS',
    binary_format: 'Mach-O',
    runtime_requirements: ['macOS 10.15 or newer'],
    signing: 'Unsigned and not notarized',
  },
  'aarch64-unknown-linux-gnu': {
    architecture: 'arm64',
    operating_system: 'Linux',
    binary_format: 'ELF',
    runtime_requirements: ['64-bit Linux', 'glibc 2.35 or newer'],
    signing: 'Unsigned',
  },
  'x86_64-unknown-linux-gnu': {
    architecture: 'x86_64',
    operating_system: 'Linux',
    binary_format: 'ELF',
    runtime_requirements: ['64-bit Linux', 'glibc 2.35 or newer'],
    signing: 'Unsigned',
  },
});

function fail(message) {
  throw new Error(message);
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function jsonBytes(value) {
  return Buffer.from(`${JSON.stringify(value, null, 2)}\n`);
}

function command(command, args, options = {}) {
  return execFileSync(command, args, {
    cwd: REPOSITORY_ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'],
    ...options,
  }).trim();
}

function workspaceVersion() {
  const cargo = readFileSync(join(REPOSITORY_ROOT, 'Cargo.toml'), 'utf8');
  const section = cargo.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/)?.[1];
  const version = section?.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) fail('could not read workspace.package.version from Cargo.toml');
  return version;
}

function parseArgs(argv) {
  const values = {};
  const flags = new Set();
  for (let index = 0; index < argv.length; index += 1) {
    const name = argv[index];
    if (name === '--native') {
      flags.add(name);
      continue;
    }
    if (!name.startsWith('--')) fail(`unexpected argument: ${name}`);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith('--')) fail(`${name} requires a value`);
    values[name.slice(2)] = value;
    index += 1;
  }
  return { values, flags };
}

function safePath(root, path) {
  const resolved = resolve(root, path);
  if (resolved !== root && !resolved.startsWith(`${root}${sep}`)) {
    fail(`path escapes package source: ${path}`);
  }
  return resolved;
}

function dependencyClosure(metadata) {
  const workspace = new Set(metadata.workspace_members);
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const root = metadata.packages.find((item) => item.name === 'pablo' && workspace.has(item.id));
  if (!root) fail('cargo metadata did not contain the pablo workspace package');

  const seen = new Set();
  const queue = [root.id];
  while (queue.length > 0) {
    const id = queue.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dependency of nodes.get(id)?.deps ?? []) {
      if (dependency.dep_kinds.some((kind) => kind.kind !== 'dev')) queue.push(dependency.pkg);
    }
  }
  return metadata.packages
    .filter((item) => seen.has(item.id) && !workspace.has(item.id))
    .sort((left, right) => `${left.name}\0${left.version}`.localeCompare(`${right.name}\0${right.version}`));
}

function licenseCandidates(pkg) {
  const root = dirname(pkg.manifest_path);
  const names = new Set();
  if (pkg.license_file) names.add(pkg.license_file);
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (entry.isFile() && /^(licen[sc]e|copying|notice|unlicense)(\.|-|$)/i.test(entry.name)) {
      names.add(entry.name);
    }
  }
  return [...names].sort().map((name) => {
    const path = safePath(root, name);
    const bytes = readFileSync(path);
    return { name: relative(root, path), bytes, sha256: sha256(bytes) };
  });
}

function thirdPartyFiles(metadata) {
  const files = new Map();
  const packages = dependencyClosure(metadata).map((pkg) => {
    const licenses = licenseCandidates(pkg).map((license) => {
      const archivePath = `licenses/${license.sha256}.txt`;
      files.set(archivePath, license.bytes);
      return { source_name: license.name, sha256: license.sha256, archive_path: archivePath };
    });
    if (!pkg.license && licenses.length === 0) {
      fail(`${pkg.name} ${pkg.version} has neither license metadata nor a bundled notice`);
    }
    return {
      name: pkg.name,
      version: pkg.version,
      license_expression: pkg.license,
      authors: pkg.authors,
      repository: pkg.repository,
      source: pkg.source,
      bundled_license_files: licenses,
    };
  });
  files.set('THIRD_PARTY_NOTICES.json', jsonBytes({
    format: 1,
    scope: 'normal and build dependencies resolved for the packaged pablo executable',
    packages,
  }));
  return files;
}

function writeString(buffer, offset, length, value) {
  const bytes = Buffer.from(value);
  if (bytes.length > length) fail(`tar field is too long: ${value}`);
  bytes.copy(buffer, offset);
}

function writeOctal(buffer, offset, length, value) {
  const octal = value.toString(8);
  if (octal.length > length - 1) fail(`tar numeric field is too large: ${value}`);
  writeString(buffer, offset, length, `${octal.padStart(length - 1, '0')}\0`);
}

function splitTarPath(path) {
  if (Buffer.byteLength(path) <= 100) return { name: path, prefix: '' };
  for (let index = path.lastIndexOf('/'); index > 0; index = path.lastIndexOf('/', index - 1)) {
    const prefix = path.slice(0, index);
    const name = path.slice(index + 1);
    if (Buffer.byteLength(prefix) <= 155 && Buffer.byteLength(name) <= 100) return { name, prefix };
  }
  fail(`archive path is too long for ustar: ${path}`);
}

function tarHeader(path, size, mode, mtime, type) {
  const header = Buffer.alloc(512);
  const parts = splitTarPath(path);
  writeString(header, 0, 100, parts.name);
  writeOctal(header, 100, 8, mode);
  writeOctal(header, 108, 8, 0);
  writeOctal(header, 116, 8, 0);
  writeOctal(header, 124, 12, size);
  writeOctal(header, 136, 12, mtime);
  header.fill(0x20, 148, 156);
  writeString(header, 156, 1, type);
  writeString(header, 257, 6, 'ustar\0');
  writeString(header, 263, 2, '00');
  writeString(header, 265, 32, 'root');
  writeString(header, 297, 32, 'root');
  writeString(header, 345, 155, parts.prefix);
  const checksum = header.reduce((sum, byte) => sum + byte, 0).toString(8).padStart(6, '0');
  writeString(header, 148, 8, `${checksum}\0 `);
  return header;
}

function deterministicTarGzip(entries, mtime) {
  const chunks = [];
  for (const entry of entries) {
    const bytes = entry.type === '5' ? Buffer.alloc(0) : entry.bytes;
    chunks.push(tarHeader(entry.path, bytes.length, entry.mode, mtime, entry.type));
    if (bytes.length > 0) {
      chunks.push(bytes);
      const padding = (512 - (bytes.length % 512)) % 512;
      if (padding > 0) chunks.push(Buffer.alloc(padding));
    }
  }
  chunks.push(Buffer.alloc(1024));
  const gzip = gzipSync(Buffer.concat(chunks), { level: 9, mtime: 0 });
  gzip.writeUInt32LE(0, 4);
  gzip[9] = 3;
  return gzip;
}

function archiveEntries(rootName, files) {
  const directories = new Set([rootName]);
  for (const path of files.keys()) {
    const parts = `${rootName}/${path}`.split('/');
    for (let index = 1; index < parts.length; index += 1) {
      directories.add(parts.slice(0, index).join('/'));
    }
  }
  const entries = [...directories].map((path) => ({ path: `${path}/`, mode: 0o755, type: '5' }));
  for (const [path, value] of files) {
    entries.push({ path: `${rootName}/${path}`, bytes: value.bytes ?? value, mode: value.mode ?? 0o644, type: '0' });
  }
  return entries.sort((left, right) => left.path.localeCompare(right.path));
}

function defaultGitValue(args, name, gitArgs) {
  return args[name] ?? command('git', gitArgs);
}

export async function packageRelease(options) {
  const versionNumber = workspaceVersion();
  const version = options.version;
  if (version !== `v${versionNumber}`) fail(`version must match Cargo.toml exactly: v${versionNumber}`);
  const target = TARGETS[options.target];
  if (!target) fail(`unsupported release target: ${options.target}`);
  if (!isAbsolute(options.binary)) fail('--binary must be an absolute path');
  if (!isAbsolute(options.outDir)) fail('--out-dir must be an absolute path');
  const binary = readFileSync(options.binary);
  if (!statSync(options.binary).isFile()) fail('--binary must name a regular file');

  const sourceRevision = defaultGitValue(options, 'sourceRevision', ['rev-parse', 'HEAD']);
  const sourceTree = defaultGitValue(options, 'sourceTree', ['rev-parse', 'HEAD^{tree}']);
  const sourceEpochText = options.sourceEpoch ?? command('git', ['show', '-s', '--format=%ct', sourceRevision]);
  const sourceEpoch = Number(sourceEpochText);
  if (!Number.isSafeInteger(sourceEpoch) || sourceEpoch <= 0) fail('--source-epoch must be a positive integer');

  if (!options.allowDirty) {
    const dirty = command('git', ['status', '--porcelain', '--untracked-files=no']);
    if (dirty) fail('refusing to package a tracked dirty source tree');
  }

  const metadata = options.metadata ?? JSON.parse(command('cargo', [
    'metadata', '--locked', '--format-version', '1', '--filter-platform', options.target,
  ]));
  const files = thirdPartyFiles(metadata);
  files.set('bin/pablo', { bytes: binary, mode: 0o755 });
  files.set('LICENSE', readFileSync(join(REPOSITORY_ROOT, 'LICENSE')));
  files.set('NOTICE', readFileSync(join(REPOSITORY_ROOT, 'NOTICE')));

  const cargoLock = readFileSync(join(REPOSITORY_ROOT, 'Cargo.lock'));
  const binaryHash = sha256(binary);
  const dynamicLibraries = options.dynamicLibraries
    ? readFileSync(options.dynamicLibraries, 'utf8').split(/\r?\n/).filter(Boolean)
    : [];
  const sourceManifest = {
    format: 1,
    repository: 'https://github.com/calebjohn24/pablo',
    version,
    revision: sourceRevision,
    git_tree: sourceTree,
    source_date_epoch: sourceEpoch,
    source_fingerprint_sha256: options.sourceFingerprint ?? await sourceFingerprint(REPOSITORY_ROOT),
    cargo_lock_sha256: sha256(cargoLock),
  };
  const buildManifest = {
    format: 1,
    profile: 'release',
    locked_dependencies: true,
    command: `cargo build --release --locked --target ${options.target} -p pablo --bin pablo`,
    rustc: options.rustc ?? command('rustc', ['-Vv']).split(/\r?\n/),
    cargo: options.cargo ?? command('cargo', ['-V']),
    node: options.node ?? process.version,
    runner: options.runner ?? null,
    native_build: Boolean(options.native),
    binary_sha256: binaryHash,
    archive: { format: 'ustar+gzip', owner: 'root:root', mtime: sourceEpoch, gzip_mtime: 0 },
  };
  const targetManifest = {
    format: 1,
    triple: options.target,
    ...target,
    dynamic_libraries: dynamicLibraries,
    native_execution_accepted: false,
    native_acceptance_checkpoint: ({
      'aarch64-apple-darwin': 'C3.35',
      'x86_64-unknown-linux-gnu': 'C3.36',
      'x86_64-apple-darwin': 'C3.37',
      'aarch64-unknown-linux-gnu': 'C3.38',
    })[options.target],
  };
  files.set('manifest/source.json', jsonBytes(sourceManifest));
  files.set('manifest/build.json', jsonBytes(buildManifest));
  files.set('manifest/target.json', jsonBytes(targetManifest));

  const rootName = `pablo-${version}-${options.target}`;
  const archiveName = `${rootName}.tar.gz`;
  const bytes = deterministicTarGzip(archiveEntries(rootName, files), sourceEpoch);
  mkdirSync(options.outDir, { recursive: true });
  const archivePath = join(options.outDir, archiveName);
  const checksumPath = `${archivePath}.sha256`;
  writeFileSync(archivePath, bytes);
  writeFileSync(checksumPath, `${sha256(bytes)}  ${archiveName}\n`);
  chmodSync(archivePath, 0o644);
  chmodSync(checksumPath, 0o644);
  return { archivePath, checksumPath, archiveName, archiveSha256: sha256(bytes), binarySha256: binaryHash };
}

async function main() {
  const { values, flags } = parseArgs(process.argv.slice(2));
  for (const required of ['version', 'target', 'binary', 'out-dir']) {
    if (!values[required]) fail(`--${required} is required`);
  }
  const result = await packageRelease({
    version: values.version,
    target: values.target,
    binary: resolve(values.binary),
    outDir: resolve(values['out-dir']),
    sourceRevision: values['source-revision'],
    sourceTree: values['source-tree'],
    sourceEpoch: values['source-epoch'],
    runner: values.runner,
    dynamicLibraries: values['dynamic-libraries'],
    native: flags.has('--native'),
  });
  process.stdout.write(`${JSON.stringify({
    archive: relative(REPOSITORY_ROOT, result.archivePath),
    checksum: relative(REPOSITORY_ROOT, result.checksumPath),
    archive_sha256: result.archiveSha256,
    binary_sha256: result.binarySha256,
  })}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`release package: ${error.message}\n`);
    process.exitCode = 1;
  });
}
