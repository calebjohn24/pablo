#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { readFileSync, realpathSync, statSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const STATE = 'docs/project/state.json';
const LOG = 'docs/project/log.jsonl';
const STATUSES = new Set(['pending', 'in_progress', 'blocked', 'done']);
const RESULTS = new Set(['passed', 'failed', 'not_run']);
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const text = (value) => typeof value === 'string' && value.trim().length > 0;
const strings = (value) => Array.isArray(value) && value.every(text);
const timestamp = (value) => typeof value === 'string'
  && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/.test(value)
  && Number.isFinite(Date.parse(value))
  && new Date(value).toISOString() === value.replace(/(?<!\.\d{3})Z$/, '.000Z');
const slug = (heading) => heading.toLowerCase().replace(/[^\p{L}\p{N}_\-\s]/gu, '').replace(/\s/g, '-');

// Only repository-local, regular, non-secret files can be referenced by records.
function localFile(root, reference) {
  if (!text(reference) || isAbsolute(reference) || reference.includes('://')) {
    throw new Error('expected a repository-relative file reference');
  }
  const [name, anchor, ...extra] = reference.split('#');
  if (!name || extra.length || anchor === '') throw new Error('invalid file/anchor reference');
  const candidate = resolve(root, name);
  const inside = (path) => {
    const rel = relative(realpathSync(root), path);
    return rel !== '' && rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
  };
  if (!inside(candidate)) throw new Error('reference escapes the repository');
  const path = realpathSync(candidate);
  if (!inside(path)) throw new Error('symlink reference escapes the repository');
  if (relative(root, path).split(sep).some((part) => part === '.git' || /^\.env(?:\.|$)/.test(part))) {
    throw new Error('private environment and Git internals cannot be project references');
  }
  if (!statSync(path).isFile()) throw new Error('reference must name a regular file');
  return { path, name, anchor };
}

function headings(markdown) {
  let fenced = false;
  return markdown.split('\n').flatMap((line, index) => {
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    const match = !fenced && /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
    return match ? [{ index, level: match[1].length, anchor: slug(match[2]) }] : [];
  });
}

function section(markdown, anchor) {
  const entries = headings(markdown);
  const start = entries.findIndex((entry) => entry.anchor === anchor);
  if (start < 0) throw new Error(`missing Markdown heading #${anchor}`);
  const end = entries.slice(start + 1).find((entry) => entry.level <= entries[start].level);
  return markdown.split('\n').slice(entries[start].index, end?.index).join('\n').trim();
}

export function inspectProject(root = ROOT) {
  root = realpathSync(root);
  const errors = [];
  const expect = (condition, message) => { if (!condition) errors.push(message); };
  const documents = new Map();
  function reference(label, ref, markdown = false) {
    try {
      const file = localFile(root, ref);
      if (markdown && !file.name.endsWith('.md')) throw new Error('expected a Markdown document');
      if (markdown || file.anchor) {
        const content = readFileSync(file.path, 'utf8');
        if (file.anchor) section(content, file.anchor);
        if (markdown) documents.set(file.name, content);
      }
      return file;
    } catch (error) {
      errors.push(`${label}: ${error.code ?? error.message}`);
      return null;
    }
  }
  function readJSON(path) {
    try { return JSON.parse(readFileSync(localFile(root, path).path, 'utf8')); }
    catch (error) { errors.push(`${path}: invalid or unreadable JSON (${error.code ?? error.name})`); return null; }
  }
  const state = readJSON(STATE);
  const logs = [];
  try {
    readFileSync(localFile(root, LOG).path, 'utf8').split('\n').forEach((line, index) => {
      if (!line.trim()) return;
      try { logs.push(JSON.parse(line)); }
      catch { errors.push(`${LOG}:${index + 1}: invalid JSON`); }
    });
  } catch (error) { errors.push(`${LOG}: unreadable (${error.code ?? error.message})`); }
  if (!object(state)) {
    expect(false, 'state must be an object');
    return { errors, state, logs, documents };
  }
  expect(state.schema_version === 1, 'state.schema_version must be 1');
  expect(timestamp(state.updated_at), 'state.updated_at must be a UTC ISO timestamp');
  expect(text(state.next_action), 'state.next_action must describe how to continue');
  expect(object(state.cycle) && text(state.cycle.id) && text(state.cycle.title), 'state.cycle must have an id and title');
  reference('cycle specification', state.cycle?.spec, true);
  expect(object(state.documents), 'state.documents must be an object');
  for (const key of ['brain', 'backlog', 'instructions']) reference(`documents.${key}`, state.documents?.[key], true);
  const brain = documents.get(state.documents?.brain) ?? '';
  expect(brain.trimEnd().split('\n').length <= 200, 'brain exceeds 200 lines; curate it and link detailed context');
  const decisions = [...brain.matchAll(/^### (D\d{3,})\b/gm)].map((match) => match[1]);
  expect(new Set(decisions).size === decisions.length, 'brain contains duplicate decision IDs');

  const source = state.source_context;
  expect(object(source), 'state.source_context must be an object');
  const contextFile = reference('source_context.path', source?.path);
  expect(typeof source?.path === 'string' && source.path.endsWith('.md'), 'source_context.path must name a Markdown file');
  expect(/^[a-f0-9]{64}$/.test(source?.sha256 ?? ''), 'source_context.sha256 must be a SHA-256 digest');
  expect(timestamp(source?.reviewed_at), 'source_context.reviewed_at must be a UTC ISO timestamp');
  if (contextFile) {
    const digest = createHash('sha256').update(readFileSync(contextFile.path)).digest('hex');
    expect(digest === source?.sha256, 'source context changed; review the brain and cycle plan before updating its hash');
  }
  if (timestamp(source?.reviewed_at) && timestamp(state.updated_at)) {
    expect(Date.parse(source.reviewed_at) <= Date.parse(state.updated_at), 'context review is newer than state.updated_at');
  }

  expect(Array.isArray(state.checkpoints) && state.checkpoints.length > 0, 'state.checkpoints must be a nonempty array');
  const checkpoints = Array.isArray(state.checkpoints) ? state.checkpoints.filter(object) : [];
  expect(checkpoints.length === state.checkpoints?.length, 'each checkpoint must be an object');
  const byId = new Map();
  for (const task of checkpoints) {
    expect(text(task.id) && !byId.has(task.id), 'checkpoint IDs must be nonempty and unique');
    if (text(task.id)) byId.set(task.id, task);
    expect(text(task.title), `${task.id}: title is required`);
    expect(STATUSES.has(task.status), `${task.id}: invalid status`);
    expect(strings(task.depends_on), `${task.id}: depends_on must be an array of IDs`);
    expect(strings(task.blockers), `${task.id}: blockers must be an array of descriptions`);
    expect(strings(task.verification), `${task.id}: verification must be an array of log IDs`);
    expect(task.status !== 'blocked' || task.blockers?.length > 0, `${task.id}: blocked checkpoint needs a blocker and unblock condition`);
    expect(task.status === 'blocked' || task.blockers?.length === 0, `${task.id}: unresolved blockers require blocked status`);
    expect(typeof task.spec === 'string' && task.spec.includes('#'), `${task.id}: spec must select a checkpoint heading`);
    reference(`${task.id} specification`, task.spec, true);
  }
  const dependencies = (task) => Array.isArray(task.depends_on) ? task.depends_on : [];
  const satisfied = (task) => dependencies(task).every((id) => byId.get(id)?.status === 'done');
  for (const task of checkpoints) {
    expect(new Set(dependencies(task)).size === dependencies(task).length, `${task.id}: duplicate dependencies`);
    for (const id of dependencies(task)) expect(byId.has(id), `${task.id}: unknown dependency ${id}`);
    if (['done', 'in_progress'].includes(task.status)) expect(satisfied(task), `${task.id}: dependencies must be done before work or completion`);
  }
  const visited = new Set();
  const visiting = new Set();
  function visit(id) {
    if (visiting.has(id)) { errors.push(`dependency cycle includes ${id}`); return; }
    if (visited.has(id) || !byId.has(id)) return;
    visiting.add(id);
    dependencies(byId.get(id)).forEach(visit);
    visiting.delete(id);
    visited.add(id);
  }
  byId.forEach((_, id) => visit(id));
  const active = checkpoints.filter((task) => task.status === 'in_progress');
  expect(active.length <= 1, 'at most one checkpoint may be in_progress');
  expect(state.active_checkpoint === (active[0]?.id ?? null), 'active_checkpoint must match the in_progress checkpoint, or be null');
  const ready = checkpoints.filter((task) => task.status === 'pending' && satisfied(task));
  if (active.length) {
    expect(state.next_checkpoint === null, 'next_checkpoint must be null while a checkpoint is active');
  } else if (ready.length) {
    expect(ready.some((task) => task.id === state.next_checkpoint), 'next_checkpoint must select a ready pending checkpoint');
  } else {
    expect(state.next_checkpoint === null, 'next_checkpoint must be null when no pending checkpoint is ready');
  }

  const logById = new Map();
  let previousNumber = 0;
  let previousTime = -Infinity;
  expect(logs.length > 0, 'work log must contain at least one entry');
  for (const entry of logs) {
    if (!object(entry)) { errors.push('each log entry must be an object'); continue; }
    const match = typeof entry.id === 'string' && /^LOG-(\d{4,})$/.exec(entry.id);
    const number = match ? Number(match[1]) : NaN;
    expect(Number.isSafeInteger(number) && number > previousNumber && !logById.has(entry.id), 'log IDs must be unique, increasing LOG-0001-style identifiers');
    previousNumber = number;
    logById.set(entry.id, entry);
    expect(timestamp(entry.timestamp), `${entry.id}: timestamp must be UTC ISO`);
    expect(Date.parse(entry.timestamp) >= previousTime, `${entry.id}: log timestamps must be chronological`);
    previousTime = Date.parse(entry.timestamp);
    expect(byId.has(entry.checkpoint), `${entry.id}: unknown checkpoint`);
    for (const field of ['summary', 'handoff']) expect(text(entry[field]), `${entry.id}: ${field} is required`);
    for (const field of ['changes', 'decisions', 'problems']) expect(strings(entry[field]), `${entry.id}: ${field} must be an array of strings`);
    for (const id of Array.isArray(entry.decisions) ? entry.decisions : []) expect(decisions.includes(id), `${entry.id}: unknown brain decision ${id}`);
    expect(Array.isArray(entry.checks), `${entry.id}: checks must be an array`);
    for (const check of Array.isArray(entry.checks) ? entry.checks : []) {
      if (!object(check)) { errors.push(`${entry.id}: each check must be an object`); continue; }
      expect(text(check.command) && RESULTS.has(check.result), `${entry.id}: check needs a command and passed/failed/not_run result`);
      expect(check.evidence === null || text(check.evidence), `${entry.id}: check evidence must be a path or null`);
      if (text(check.evidence)) reference(`${entry.id} evidence`, check.evidence);
    }
  }
  expect(state.latest_log_entry === logs.at(-1)?.id && logById.has(state.latest_log_entry), 'latest_log_entry must match the last log entry');
  if (timestamp(state.updated_at) && logs.length) expect(Date.parse(state.updated_at) >= previousTime, 'state.updated_at predates the latest log entry');
  for (const task of checkpoints) {
    const refs = Array.isArray(task.verification) ? task.verification : [];
    expect(new Set(refs).size === refs.length, `${task.id}: duplicate verification references`);
    if (task.status === 'done') expect(refs.length > 0, `${task.id}: done checkpoint needs completion evidence`);
    for (const id of refs) {
      const entry = logById.get(id);
      expect(entry?.checkpoint === task.id, `${task.id}: verification ${id} must reference a log entry for this checkpoint`);
      if (task.status === 'done' && entry) {
        const checks = Array.isArray(entry.checks) ? entry.checks : [];
        expect(checks.length > 0 && checks.every((check) => object(check) && check.result === 'passed' && text(check.evidence)), `${task.id}: completion evidence must contain passing checks with evidence paths`);
        expect(Array.isArray(entry.problems) && entry.problems.length === 0, `${task.id}: completion evidence contains unresolved problems`);
      }
    }
  }

  // Check links in the small governed documents, not the historical architecture brief.
  for (const [name, content] of documents) {
    for (const match of content.matchAll(/\[[^\]\n]*\]\(([^)\s]+)\)/g)) {
      const target = match[1];
      if (/^[a-z][a-z\d+.-]*:/i.test(target)) continue;
      const [path, anchor] = target.split('#');
      const local = path ? relative(root, resolve(root, dirname(name), path)) : name;
      reference(`${name} link ${target}`, `${local}${anchor ? `#${anchor}` : ''}`);
    }
  }
  return { errors, state, logs, documents, ready };
}

export function renderStatus({ state, ready }) {
  const lines = [
    `${state.cycle.id} — ${state.cycle.title}`,
    `Progress: ${state.checkpoints.filter((task) => task.status === 'done').length}/${state.checkpoints.length} checkpoints done`,
    `Active: ${state.active_checkpoint ?? 'none'}`,
    `Next: ${state.next_checkpoint ?? 'none'}`,
  ];
  for (const task of state.checkpoints) {
    const status = ready.some((candidate) => candidate.id === task.id) ? 'ready' : task.status;
    lines.push(`  ${task.id} [${status}] ${task.title}`);
    for (const blocker of task.blockers) lines.push(`    Blocker: ${blocker}`);
  }
  lines.push(`Next action: ${state.next_action}`, `Records: ${state.updated_at}; latest entry ${state.latest_log_entry}`);
  return lines.join('\n');
}

function excerpt(value, maxLines, maxCharacters, ref) {
  const lines = value.trim().split('\n');
  const clipped = lines.slice(0, maxLines).join('\n').slice(0, maxCharacters);
  return clipped + (lines.length > maxLines || clipped.length < value.trim().length ? `\n[Excerpt shortened; read ${ref} for the full text.]` : '');
}

export function renderContext(project) {
  const { state, logs, documents } = project;
  const parts = [renderStatus(project), '## Brain', excerpt(documents.get(state.documents.brain), 200, 16000, state.documents.brain)];
  const selected = state.checkpoints.find((task) => task.id === (state.active_checkpoint ?? state.next_checkpoint))
    ?? state.checkpoints.find((task) => task.status === 'blocked');
  if (selected) {
    const [path, anchor] = selected.spec.split('#');
    parts.push(`## Selected specification: ${selected.spec}`, excerpt(section(documents.get(path), anchor), 120, 12000, selected.spec));
  }
  parts.push('## Recent work');
  for (const entry of logs.slice(-3)) {
    const details = [
      `${entry.id} | ${entry.timestamp} | ${entry.checkpoint}`,
      entry.summary,
      ...entry.changes.map((change) => `- ${change}`),
      ...entry.checks.map((check) => `Check: ${check.result} — ${check.command}${check.evidence ? ` (${check.evidence})` : ''}`),
      ...entry.problems.map((problem) => `Problem: ${problem}`),
      `Decisions: ${entry.decisions.join(', ') || 'none'}`,
      `Handoff: ${entry.handoff}`,
    ].join('\n');
    parts.push(excerpt(details, 40, 4000, `${LOG} entry ${entry.id}`));
  }
  return parts.join('\n\n');
}

export function runCLI(args, root = ROOT, io = { out: console.log, err: console.error }) {
  if (args.length !== 1 || !['status', 'context', 'check'].includes(args[0])) {
    io.err('Usage: node scripts/project.mjs <status|context|check>');
    return 2;
  }
  try {
    const project = inspectProject(root);
    if (project.errors.length) {
      io.err(`Project records need attention:\n${project.errors.map((error) => `- ${error}`).join('\n')}`);
      return 1;
    }
    if (args[0] === 'check') io.out(`Project records are consistent (${project.state.checkpoints.length} checkpoints, ${project.logs.length} log entries). This checks records, not execution evidence.`);
    else io.out(args[0] === 'status' ? renderStatus(project) : renderContext(project));
    return 0;
  } catch (error) {
    io.err(`Cannot inspect project records: ${error.code ?? error.name}`);
    return 1;
  }
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = runCLI(process.argv.slice(2));
}
