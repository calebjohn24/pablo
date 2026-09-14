#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { TARGETS } from './package.mjs';
import { verifyRelease } from './verify.mjs';

function fail(message) {
  throw new Error(message);
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    if (!argv[index]?.startsWith('--') || !argv[index + 1]) fail(`invalid argument: ${argv[index]}`);
    values[argv[index].slice(2)] = argv[index + 1];
  }
  return values;
}

function walk(root) {
  const files = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) files.push(...walk(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

function readArchiveJson(archive, path) {
  return JSON.parse(execFileSync('tar', ['-xOzf', archive, path], { encoding: 'utf8' }));
}

export async function buildReleaseIndex({ version, inputDir, outDir, requiredTargets = Object.keys(TARGETS) }) {
  const files = walk(inputDir);
  const artifacts = [];
  for (const target of requiredTargets.sort()) {
    const archiveName = `pablo-${version}-${target}.tar.gz`;
    const matches = files.filter((path) => basename(path) === archiveName);
    const checksumMatches = files.filter((path) => basename(path) === `${archiveName}.sha256`);
    if (matches.length !== 1 || checksumMatches.length !== 1) fail(`expected one archive and checksum for ${target}`);
    const verified = verifyRelease({
      version,
      target,
      archive: matches[0],
      checksum: checksumMatches[0],
    });
    const root = `pablo-${version}-${target}`;
    const source = readArchiveJson(matches[0], `${root}/manifest/source.json`);
    const build = readArchiveJson(matches[0], `${root}/manifest/build.json`);
    const targetManifest = readArchiveJson(matches[0], `${root}/manifest/target.json`);
    artifacts.push({
      target,
      archive: archiveName,
      checksum: `${archiveName}.sha256`,
      archive_sha256: verified.archive_sha256,
      binary_sha256: verified.binary_sha256,
      source,
      build,
      target_manifest: targetManifest,
      source_path: matches[0],
      checksum_path: checksumMatches[0],
    });
  }

  const identityFields = ['revision', 'git_tree', 'source_fingerprint_sha256', 'cargo_lock_sha256'];
  for (const field of identityFields) {
    const values = new Set(artifacts.map((artifact) => artifact.source[field]));
    if (values.size !== 1) fail(`release candidates disagree on source.${field}`);
  }
  if (artifacts.some((artifact) => artifact.build.native_build !== true)) {
    fail('release index only accepts artifacts built on their native architecture');
  }

  mkdirSync(outDir, { recursive: true });
  for (const artifact of artifacts) {
    copyFileSync(artifact.source_path, join(outDir, artifact.archive));
    copyFileSync(artifact.checksum_path, join(outDir, artifact.checksum));
  }
  const checksums = artifacts
    .sort((left, right) => left.archive.localeCompare(right.archive))
    .map((artifact) => `${artifact.archive_sha256}  ${artifact.archive}`)
    .join('\n');
  writeFileSync(join(outDir, 'SHA256SUMS'), `${checksums}\n`);
  const first = artifacts[0].source;
  const manifest = {
    format: 1,
    version,
    source: Object.fromEntries(identityFields.map((field) => [field, first[field]])),
    native_execution_accepted: false,
    artifacts: artifacts.map(({ source_path, checksum_path, source, build, target_manifest, ...artifact }) => ({
      ...artifact,
      build_runner: build.runner,
      native_build: build.native_build,
      runtime_requirements: target_manifest.runtime_requirements,
      signing: target_manifest.signing,
      native_acceptance_checkpoint: target_manifest.native_acceptance_checkpoint,
    })),
  };
  writeFileSync(join(outDir, 'release-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

async function main() {
  const values = parseArgs(process.argv.slice(2));
  for (const required of ['version', 'input-dir', 'out-dir']) {
    if (!values[required]) fail(`--${required} is required`);
  }
  const result = await buildReleaseIndex({
    version: values.version,
    inputDir: resolve(values['input-dir']),
    outDir: resolve(values['out-dir']),
  });
  process.stdout.write(`${JSON.stringify({ version: result.version, artifacts: result.artifacts.length })}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`release index: ${error.message}\n`);
    process.exitCode = 1;
  });
}
