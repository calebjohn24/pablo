import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { inspectProject, renderContext, renderStatus } from './project.mjs';

const SCRIPT = fileURLToPath(new URL('./project.mjs', import.meta.url));
const CONTEXT = '# Full design\n\nDESIGN_BODY_MUST_NOT_APPEAR_IN_HANDOFF\n';
const TIME = '2026-09-05T01:28:09Z';
const entry = (number) => ({
  id: `LOG-${String(number).padStart(4, '0')}`,
  timestamp: TIME,
  checkpoint: 'C1.0',
  summary: `Work entry ${number}`,
  changes: [`Change ${number}`],
  decisions: ['D001'],
  checks: [{ command: 'node --test', result: 'passed', evidence: 'docs/project/evidence/c1.0.md' }],
  problems: [],
  handoff: 'Start C1.1 and stop after its acceptance checks.',
});

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'pablo-project-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const write = (path, content) => {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), content);
  };
  write('docs/context.md', CONTEXT);
  write('AGENTS.md', '# Instructions\n\nOne checkpoint per session.\n');
  write('docs/project/brain.md', '# Brain\n\n### D001 — Work in checkpoints\n\nUse [the design](../context.md).\n');
  write('docs/project/backlog.md', '# Backlog\n');
  write('docs/project/evidence/c1.0.md', '# Verification\n\nTests passed.\n');
  write('docs/project/cycles/001-first-spike.md', '# Cycle\n\n## C1.0: Project memory\n\nCheck records.\n\n## C1.1: Runtime\n\nProve one streamed response.\n\n## Later details\n\nLATER_SECTION_MUST_NOT_APPEAR\n');
  const state = {
    schema_version: 1,
    cycle: { id: 'C1', title: 'Focused first spike', spec: 'docs/project/cycles/001-first-spike.md' },
    source_context: { path: 'docs/context.md', sha256: createHash('sha256').update(CONTEXT).digest('hex'), reviewed_at: TIME },
    documents: { brain: 'docs/project/brain.md', backlog: 'docs/project/backlog.md', instructions: 'AGENTS.md' },
    updated_at: TIME,
    active_checkpoint: null,
    next_checkpoint: 'C1.1',
    next_action: 'Start the runtime foundation.',
    latest_log_entry: 'LOG-0001',
    checkpoints: [
      { id: 'C1.0', title: 'Project memory', spec: 'docs/project/cycles/001-first-spike.md#c10-project-memory', depends_on: [], status: 'done', blockers: [], verification: ['LOG-0001'] },
      { id: 'C1.1', title: 'Runtime', spec: 'docs/project/cycles/001-first-spike.md#c11-runtime', depends_on: ['C1.0'], status: 'pending', blockers: [], verification: [] },
    ],
  };
  const logs = [entry(1)];
  const save = () => {
    write('docs/project/state.json', `${JSON.stringify(state, null, 2)}\n`);
    write('docs/project/log.jsonl', `${logs.map((record) => JSON.stringify(record)).join('\n')}\n`);
  };
  save();
  return { root, state, logs, write, save };
}

test('a completed checkpoint leaves the next dependency-ready checkpoint selected', (t) => {
  const { root } = fixture(t);
  const project = inspectProject(root);
  assert.deepEqual(project.errors, []);
  assert.match(renderStatus(project), /Progress: 1\/2/);
  assert.match(renderStatus(project), /C1\.1 \[ready\] Runtime/);
  const context = renderContext(project);
  assert.match(context, /Prove one streamed response/);
  assert.doesNotMatch(context, /DESIGN_BODY_MUST_NOT_APPEAR|LATER_SECTION_MUST_NOT_APPEAR/);
});

test('active work resumes instead of selecting another checkpoint', (t) => {
  const data = fixture(t);
  data.state.checkpoints[1].status = 'in_progress';
  data.state.active_checkpoint = 'C1.1';
  data.state.next_checkpoint = null;
  data.save();
  const project = inspectProject(data.root);
  assert.deepEqual(project.errors, []);
  assert.match(renderStatus(project), /Active: C1\.1/);
});

test('blocked work remains explicit and provides the blocked specification as context', (t) => {
  const data = fixture(t);
  Object.assign(data.state.checkpoints[1], { status: 'blocked', blockers: ['Await fixture access; resume when the fixture is supplied.'] });
  data.state.next_checkpoint = null;
  data.save();
  const project = inspectProject(data.root);
  assert.deepEqual(project.errors, []);
  assert.match(renderContext(project), /Blocker: Await fixture access/);
  assert.match(renderContext(project), /Prove one streamed response/);
});

const invalidCases = [
  ['unsupported state version', (data) => { data.state.schema_version = 2; }, /schema_version/],
  ['malformed checkpoint collection', (data) => { data.state.checkpoints = [null, 'bad']; }, /checkpoint must be an object/],
  ['duplicate checkpoint IDs', (data) => { data.state.checkpoints.push(structuredClone(data.state.checkpoints[0])); }, /IDs must be nonempty and unique/],
  ['unknown dependencies', (data) => { data.state.checkpoints[1].depends_on = ['C9.0']; }, /unknown dependency/],
  ['dependency cycles', (data) => { data.state.checkpoints[0].depends_on = ['C1.1']; }, /dependency cycle/],
  ['multiple active checkpoints', (data) => { data.state.checkpoints.forEach((task) => { task.status = 'in_progress'; }); }, /at most one/],
  ['stale active pointer', (data) => { data.state.active_checkpoint = 'C1.1'; }, /active_checkpoint must match/],
  ['unready next checkpoint', (data) => { data.state.next_checkpoint = 'C1.0'; }, /next_checkpoint must select/],
  ['blocked without explanation', (data) => { data.state.checkpoints[1].status = 'blocked'; }, /needs a blocker/],
  ['unresolved blocker on done work', (data) => { data.state.checkpoints[0].blockers = ['Still failing']; }, /unresolved blockers/],
  ['unknown status', (data) => { data.state.checkpoints[1].status = 'complete'; }, /invalid status/],
  ['done without verification', (data) => { data.state.checkpoints[0].verification = []; }, /needs completion evidence/],
  ['verification for another checkpoint', (data) => { data.logs[0].checkpoint = 'C1.1'; }, /must reference a log entry for this checkpoint/],
  ['failed completion checks', (data) => { data.logs[0].checks[0].result = 'failed'; }, /passing checks/],
  ['unrun completion checks', (data) => { data.logs[0].checks[0].result = 'not_run'; }, /passing checks/],
  ['completion without an evidence file', (data) => { data.logs[0].checks[0].evidence = null; }, /evidence paths/],
  ['missing evidence file', (data) => { data.logs[0].checks[0].evidence = 'missing.txt'; }, /evidence: ENOENT/],
  ['missing specification anchor', (data) => { data.state.checkpoints[1].spec += '-missing'; }, /missing Markdown heading/],
  ['stale log pointer', (data) => { data.state.latest_log_entry = 'LOG-0000'; }, /latest_log_entry must match/],
  ['duplicate log IDs', (data) => { data.logs.push(structuredClone(data.logs[0])); }, /log IDs must be unique/],
  ['log chronology', (data) => { data.logs.push({ ...entry(2), timestamp: '2026-09-04T00:00:00Z' }); }, /chronological/],
  ['invalid calendar timestamp', (data) => { data.state.updated_at = '2026-02-30T01:00:00Z'; }, /UTC ISO timestamp/],
  ['state predating logged work', (data) => { data.state.updated_at = '2026-09-04T00:00:00Z'; }, /predates the latest log/],
  ['unknown decision ID', (data) => { data.logs[0].decisions = ['D999']; }, /unknown brain decision/],
  ['missing next action', (data) => { data.state.next_action = ''; }, /next_action/],
];

for (const [name, mutate, expected] of invalidCases) {
  test(`rejects ${name}`, (t) => {
    const data = fixture(t);
    mutate(data);
    data.save();
    assert.match(inspectProject(data.root).errors.join('\n'), expected);
  });
}

test('detects source-context drift without rewriting the reviewed hash', (t) => {
  const data = fixture(t);
  data.write('docs/context.md', `${CONTEXT}\nChanged release boundary.\n`);
  const before = readFileSync(join(data.root, 'docs/project/state.json'), 'utf8');
  assert.match(inspectProject(data.root).errors.join('\n'), /source context changed/);
  assert.equal(readFileSync(join(data.root, 'docs/project/state.json'), 'utf8'), before);
});

test('rejects a brain exceeding the context budget and broken local links', (t) => {
  const data = fixture(t);
  data.write('docs/project/brain.md', `${'A line\n'.repeat(201)}\n[missing](absent.md)\n`);
  const errors = inspectProject(data.root).errors.join('\n');
  assert.match(errors, /brain exceeds 200 lines/);
  assert.match(errors, /link absent.md: ENOENT/);
});

test('reports malformed JSON and malformed JSONL with actionable locations', (t) => {
  const data = fixture(t);
  data.write('docs/project/state.json', '{broken');
  assert.match(inspectProject(data.root).errors.join('\n'), /state.json: invalid/);
  data.save();
  data.write('docs/project/log.jsonl', `${JSON.stringify(data.logs[0])}\n{broken\n`);
  assert.match(inspectProject(data.root).errors.join('\n'), /log.jsonl:2: invalid JSON/);
});

test('rejects references to credentials, outside paths, and outside symlinks', (t) => {
  const data = fixture(t);
  data.write('.env', 'SECRET_SENTINEL=never-print-this\n');
  for (const ref of ['.env', '../outside.md']) {
    data.state.documents.brain = ref;
    data.save();
    const errors = inspectProject(data.root).errors.join('\n');
    assert.match(errors, /private environment|escapes the repository/);
    assert.doesNotMatch(errors, /never-print-this/);
  }
  symlinkSync(SCRIPT, join(data.root, 'outside.md'));
  data.state.documents.brain = 'outside.md';
  data.save();
  assert.match(inspectProject(data.root).errors.join('\n'), /symlink reference escapes/);
});

test('context includes only the latest three log entries and bounds large excerpts', (t) => {
  const data = fixture(t);
  data.logs.push(entry(2), entry(3), entry(4));
  data.logs[3].summary = 'x'.repeat(10000);
  data.state.latest_log_entry = 'LOG-0004';
  data.save();
  const project = inspectProject(data.root);
  assert.deepEqual(project.errors, []);
  const context = renderContext(project);
  assert.doesNotMatch(context, /Work entry 1/);
  assert.match(context, /Work entry 2/);
  assert.match(context, /Excerpt shortened/);
  assert.ok(context.length < 10000);
});

function snapshot(root, prefix = '') {
  return readdirSync(join(root, prefix)).sort().flatMap((name) => {
    const path = join(prefix, name);
    const stat = statSync(join(root, path));
    return stat.isDirectory() ? snapshot(root, path) : [[path, stat.mtimeMs, readFileSync(join(root, path)).toString('base64')]];
  });
}

test('real CLI commands work outside the repository and do not mutate files', (t) => {
  const data = fixture(t);
  data.write('scripts/project.mjs', '');
  copyFileSync(SCRIPT, join(data.root, 'scripts/project.mjs'));
  data.write('.env', 'PRIVATE_KEY=never-print-this\n');
  const before = snapshot(data.root);
  for (const command of ['status', 'context', 'check']) {
    const result = spawnSync(process.execPath, [join(data.root, 'scripts/project.mjs'), command], { cwd: tmpdir(), encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stderr, '');
    assert.match(result.stdout, command === 'check' ? /Project records are consistent/ : /C1 — Focused first spike/);
    assert.doesNotMatch(result.stdout, /never-print-this/);
  }
  assert.deepEqual(snapshot(data.root), before);
  const badCommand = spawnSync(process.execPath, [join(data.root, 'scripts/project.mjs'), 'finish'], { encoding: 'utf8' });
  assert.equal(badCommand.status, 2);
  assert.match(badCommand.stderr, /Usage:/);
  data.state.checkpoints[0].verification = [];
  data.save();
  for (const command of ['status', 'context', 'check']) {
    const invalid = spawnSync(process.execPath, [join(data.root, 'scripts/project.mjs'), command], { encoding: 'utf8' });
    assert.equal(invalid.status, 1);
    assert.equal(invalid.stdout, '');
    assert.match(invalid.stderr, /done checkpoint needs completion evidence/);
  }
});
