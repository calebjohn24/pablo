import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const repository = path.resolve(import.meta.dirname, "..");
const installer = path.join(repository, "install.sh");

function targetTriple() {
  if (process.platform === "darwin" && process.arch === "arm64") return "aarch64-apple-darwin";
  if (process.platform === "darwin" && process.arch === "x64") return "x86_64-apple-darwin";
  if (process.platform === "linux" && process.arch === "arm64") return "aarch64-unknown-linux-gnu";
  if (process.platform === "linux" && process.arch === "x64") return "x86_64-unknown-linux-gnu";
  throw new Error(`unsupported test platform ${process.platform}/${process.arch}`);
}

function fixture(root, version, marker = version) {
  const target = targetTriple();
  const tag = `v${version}`;
  const archiveRoot = `pablo-${tag}-${target}`;
  const source = path.join(root, `${archiveRoot}-source`, archiveRoot);
  fs.mkdirSync(path.join(source, "bin"), { recursive: true });
  const binary = path.join(source, "bin", "pablo");
  fs.writeFileSync(binary, `#!/bin/sh\nif [ "\${1:-}" = "--version" ]; then printf '%s\\n' 'pablo ${version}'; else printf '%s\\n' '${marker}'; fi\n`);
  fs.chmodSync(binary, 0o755);
  const archive = path.join(root, `${archiveRoot}.tar.gz`);
  execFileSync("tar", ["-czf", archive, "-C", path.dirname(source), archiveRoot]);
  const digest = createHash("sha256").update(fs.readFileSync(archive)).digest("hex");
  const checksum = `${archive}.sha256`;
  fs.writeFileSync(checksum, `${digest}  ${path.basename(archive)}\n`);
  return { archive, checksum, tag, version, marker };
}

function run(args, options = {}) {
  return spawnSync("sh", [installer, ...args], {
    cwd: repository,
    encoding: "utf8",
    env: { ...process.env, ...options.env },
  });
}

function installArgs(item, prefix, extra = []) {
  return [
    "--version",
    item.tag,
    "--prefix",
    prefix,
    "--archive",
    item.archive,
    "--checksum",
    item.checksum,
    ...extra,
  ];
}

test("installs, receipt-verifies updates, explicitly replaces, and removes", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pablo-installer-happy-"));
  try {
    const prefix = path.join(root, "prefix");
    const unrelated = path.join(prefix, "share", "keep.txt");
    fs.mkdirSync(path.dirname(unrelated), { recursive: true });
    fs.writeFileSync(unrelated, "keep\n");

    const first = fixture(root, "0.0.1", "first");
    const installed = run(installArgs(first, prefix));
    assert.equal(installed.status, 0, installed.stderr);
    const binary = path.join(prefix, "bin", "pablo");
    assert.equal(execFileSync(binary, [], { encoding: "utf8" }).trim(), "first");

    const collision = run(installArgs(first, prefix));
    assert.notEqual(collision.status, 0);
    assert.match(collision.stderr, /already exists/);
    assert.equal(execFileSync(binary, [], { encoding: "utf8" }).trim(), "first");

    const second = fixture(root, "0.0.2", "second");
    const updated = run(installArgs(second, prefix, ["--update"]));
    assert.equal(updated.status, 0, updated.stderr);
    assert.equal(execFileSync(binary, [], { encoding: "utf8" }).trim(), "second");

    const sameVersion = run(installArgs(second, prefix, ["--update"]));
    assert.notEqual(sameVersion.status, 0);
    assert.match(sameVersion.stderr, /already installed/);

    const third = fixture(root, "0.0.3", "third");
    const replaced = run(installArgs(third, prefix, ["--replace"]));
    assert.equal(replaced.status, 0, replaced.stderr);
    assert.equal(execFileSync(binary, [], { encoding: "utf8" }).trim(), "third");

    const removed = run(["--prefix", prefix, "--remove"]);
    assert.equal(removed.status, 0, removed.stderr);
    assert.equal(fs.existsSync(binary), false);
    assert.equal(fs.readFileSync(unrelated, "utf8"), "keep\n");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("rejects a corrupt checksum before installing content", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pablo-installer-checksum-"));
  try {
    const item = fixture(root, "0.0.1");
    fs.writeFileSync(item.checksum, `${"0".repeat(64)}  ${path.basename(item.archive)}\n`);
    const prefix = path.join(root, "prefix");
    const result = run(installArgs(item, prefix));
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /checksum verification failed/);
    assert.equal(fs.existsSync(path.join(prefix, "bin", "pablo")), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("refuses a pablo command outside the selected prefix", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pablo-installer-command-"));
  try {
    const item = fixture(root, "0.0.1");
    const commands = path.join(root, "commands");
    fs.mkdirSync(commands);
    const existing = path.join(commands, "pablo");
    fs.writeFileSync(existing, "#!/bin/sh\nexit 0\n");
    fs.chmodSync(existing, 0o755);
    const prefix = path.join(root, "prefix");
    const result = run(installArgs(item, prefix), {
      env: { PATH: `${commands}${path.delimiter}${process.env.PATH}` },
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /outside the selected prefix/);
    assert.equal(fs.existsSync(path.join(prefix, "bin", "pablo")), false);
    assert.equal(fs.readFileSync(existing, "utf8"), "#!/bin/sh\nexit 0\n");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("refuses removal when the installed binary changed", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "pablo-installer-remove-"));
  try {
    const item = fixture(root, "0.0.1");
    const prefix = path.join(root, "prefix");
    const installed = run(installArgs(item, prefix));
    assert.equal(installed.status, 0, installed.stderr);
    const binary = path.join(prefix, "bin", "pablo");
    fs.appendFileSync(binary, "# changed\n");
    const next = fixture(root, "0.0.2");
    const updated = run(installArgs(next, prefix, ["--update"]));
    assert.notEqual(updated.status, 0);
    assert.match(updated.stderr, /changed; refusing to update/);
    assert.equal(fs.existsSync(binary), true);
    const removed = run(["--prefix", prefix, "--remove"]);
    assert.notEqual(removed.status, 0);
    assert.match(removed.stderr, /changed; refusing to remove/);
    assert.equal(fs.existsSync(binary), true);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
