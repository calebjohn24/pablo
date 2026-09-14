/** A single-task reference client, using the official stable ACP v1 SDK. */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { Readable, Writable } from 'node:stream';
import { fileURLToPath } from 'node:url';
import { client, ndJsonStream, RequestError, type ClientContext, type PromptResponse, type SessionNotification } from '@agentclientprotocol/sdk';

export type RunOutcome =
  | { status: 'completed'; output: string; finish_reason: 'stop'; usage: {
      input_tokens: number | null; output_tokens: number | null;
      cache_read_input_tokens: number | null; cache_write_input_tokens: number | null;
    } }
  | { status: 'cancelled' | 'timed_out' }
  | { status: 'policy_denied'; rule: string | { configured: { id: string } } }
  | { status: 'limit_exceeded'; limit: string }
  | { status: 'failed'; code: string; delivery: string };

export function outcomeOf(response: PromptResponse): RunOutcome {
  return parseOutcome(metadataOf(response));
}
function metadataOf(response: PromptResponse): unknown {
  return response._meta?.['pablo/v2'] ?? response._meta?.['pablo/v1'];
}

export interface OutputValidation {
  schema_version: 'output-validation-v1'; schema_sha256: string;
  status: 'unvalidated' | 'valid' | 'invalid';
  diagnostics: {code: string; instance_path: string; schema_path: string}[];
}
export interface OutputRepair { schema_version: 'output-repair-v1'; status: 'available'|'pending'|'started'|'not_needed'|'succeeded'|'failed'|'blocked'; attempts: number; validation_attempts: number; feedback_bytes: number; previous_error_count: number; }
export interface TaskResult {
  output_repair?: OutputRepair;
  schema_version: 'c2.3' | 'c3.13' | 'c3.14' | 'c3.33'; output_validation?: OutputValidation; run_id: string; session_id: string; trace_id: string;
  outcome: RunOutcome; error: null;
  accounting: { model_calls: string; tool_calls: string;
    usage: Record<'input_tokens' | 'output_tokens' | 'cache_read_input_tokens' | 'cache_write_input_tokens', string | null>;
    cost_microusd: string | null; charged_tokens: string | null; charged_cost_microusd: string | null };
}
function parseTask(value: unknown): TaskResult {
  const t = value as TaskResult;
  const decimal = (v: unknown): boolean => typeof v === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(v) && BigInt(v) <= 18446744073709551615n;
  const nullable = (v: unknown): boolean => v === null || decimal(v);
  if (!t || !['c2.3','c3.13','c3.14','c3.33'].includes(t.schema_version) || t.error !== null ||
      ![t.run_id, t.session_id, t.trace_id].every(v => typeof v === 'string' && v.length > 0) ||
      !t.accounting || !decimal(t.accounting.model_calls) || !decimal(t.accounting.tool_calls) ||
      !t.accounting.usage || !['input_tokens','output_tokens','cache_read_input_tokens','cache_write_input_tokens'].every(k => nullable((t.accounting.usage as Record<string, unknown>)[k])) ||
      !['cost_microusd','charged_tokens','charged_cost_microusd'].every(k => nullable((t.accounting as unknown as Record<string, unknown>)[k]))) throw new Error('Invalid Pablo task accounting');
  const v=t.output_validation;
  if (t.schema_version==='c3.13'||t.schema_version==='c3.14'||(t.schema_version==='c3.33'&&v!==undefined)) {
    if (!v || v.schema_version!=='output-validation-v1' || !/^sha256:[0-9a-f]{64}$/.test(v.schema_sha256) || !['unvalidated','valid','invalid'].includes(v.status) ||
      !Array.isArray(v.diagnostics) || v.diagnostics.length>8 || Buffer.byteLength(JSON.stringify(v.diagnostics))>2048 ||
      v.diagnostics.some(d=>!['malformed_json','json_bytes','validation_work','json_structure','schema_violation'].includes(d.code) || typeof d.instance_path!=='string' || typeof d.schema_path!=='string' || Buffer.byteLength(d.instance_path)>256 || Buffer.byteLength(d.schema_path)>256) ||
      (v.status==='invalid')!==(v.diagnostics.length>0) || (v.status==='valid'&&t.outcome.status!=='completed') ||
      (v.status==='invalid'&&(t.outcome.status!=='failed'||t.outcome.code!=='output_validation_failed'))) throw new Error('Invalid Pablo output validation');
  } else if (v!==undefined) throw new Error('Unexpected Pablo output validation');
  const r=t.output_repair;
  if(t.schema_version==='c3.14'||(t.schema_version==='c3.33'&&r!==undefined)) {
    if(!v || !r || r.schema_version!=='output-repair-v1' || !['not_needed','succeeded','failed','blocked'].includes(r.status) ||
      ![r.attempts,r.validation_attempts,r.feedback_bytes,r.previous_error_count].every(n=>Number.isInteger(n)&&n>=0) || r.attempts>1 || r.validation_attempts>2 || r.feedback_bytes>4096 || r.previous_error_count>8 ||
      (r.status==='succeeded'&&(r.attempts!==1||r.validation_attempts!==2||v?.status!=='valid'))) throw new Error('Invalid Pablo output repair');
  } else if(r!==undefined) throw new Error('Unexpected Pablo output repair');
  return t;
}
/** Exact counts are decimal strings; use BigInt when arithmetic is needed. */
export function taskOf(response: PromptResponse): TaskResult {
  const task = parseTask((metadataOf(response) as Record<string, unknown>)?.task);
  parseOutcome(metadataOf(response));
  return task;
}

/** Parse structured output only after the runtime's local validation succeeded. */
export function structuredOf(response: PromptResponse): unknown {
  const task=taskOf(response);
  if(task.output_validation?.status!=='valid'||task.outcome.status!=='completed') throw new Error('Output is not validated');
  return JSON.parse(task.outcome.output);
}

function parseOutcome(value: unknown): RunOutcome {
  const meta = value as Record<string, unknown> | undefined;
  const task = meta?.task ? parseTask(meta.task) : undefined;
  const outcome = (task?.outcome ?? meta?.outcome) as RunOutcome | undefined;
  if (!outcome || typeof outcome !== 'object') throw new Error('Missing negotiated Pablo outcome');
  switch (outcome.status) {
    case 'completed':
      if (typeof outcome.output !== 'string' || outcome.finish_reason !== 'stop' || !outcome.usage ||
          !['input_tokens', 'output_tokens', 'cache_read_input_tokens', 'cache_write_input_tokens'].every(key => {
            const value = (outcome.usage as Record<string, unknown>)[key];
            return value === null || (typeof value === 'number' && (task ? Number.isInteger(value) : Number.isSafeInteger(value)) && value >= 0);
          })) throw new Error('Invalid completed outcome');
      return outcome;
    case 'cancelled': case 'timed_out': return outcome;
    case 'policy_denied': if (typeof outcome.rule === 'string' || (outcome.rule && typeof outcome.rule.configured?.id === 'string' && /^[A-Za-z0-9_.-]{1,64}$/.test(outcome.rule.configured.id))) return outcome; break;
    case 'limit_exceeded': if (typeof outcome.limit === 'string') return outcome; break;
    case 'failed': if (typeof outcome.code === 'string' && typeof outcome.delivery === 'string') return outcome; break;
  }
  throw new Error('Unknown or invalid Pablo outcome');
}

function describeOutcome(outcome: RunOutcome): string {
  switch (outcome.status) {
    case 'limit_exceeded': return `${outcome.status}: ${outcome.limit}`;
    case 'policy_denied': return `${outcome.status}: ${typeof outcome.rule === 'string' ? outcome.rule : outcome.rule.configured.id}`;
    case 'failed': return `${outcome.status}: ${outcome.code} (${outcome.delivery})`;
    default: return outcome.status;
  }
}

export interface ClientOptions {
  binary?: string;
  args?: string[];
  env?: NodeJS.ProcessEnv;
  onUpdate?: (notification: SessionNotification, context: ClientContext) => void | Promise<void>;
  onCompaction?: (notification: unknown) => void | Promise<void>;
  onSkill?: (notification: unknown) => void | Promise<void>;
  onModelAttempt?: (notification: unknown) => void | Promise<void>;
  onDiagnostic?: (text: string) => void;
  onSpawn?: (child: ChildProcessWithoutNullStreams) => void;
}

/** The callback uses ACP methods directly; this is not a second language SDK. */
export async function withPablo<T>(options: ClientOptions, operation: (context: ClientContext) => Promise<T>): Promise<T> {
  const binary = options.binary ?? process.env.PABLO_BINARY ?? fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
  const child = spawn(binary, ['acp', '--stdio', ...(options.args ?? [])], {
    env: options.env ?? process.env, stdio: ['pipe', 'pipe', 'pipe'],
  });
  const exited = once(child, 'exit');
  // Attach immediately; a missing executable must reject rather than become an
  // unhandled event while the protocol connection is starting.
  exited.catch(() => {});
  child.stdin.on('error', () => {});
  child.stderr.on('data', (chunk: Buffer) => options.onDiagnostic?.(chunk.toString()));
  options.onSpawn?.(child);
  const stream = ndJsonStream(Writable.toWeb(child.stdin), Readable.toWeb(child.stdout) as unknown as ReadableStream<Uint8Array>);
  try {
    return await client({ name: 'pablo-reference' })
      .onNotification('session/update', ({ params, agent }) => options.onUpdate?.(params, agent))
      // Older development servers used unversioned methods; the current server
      // publishes only versioned methods after v2 negotiation.
      .onNotification('_pablo/compaction', (params: unknown) => params, ({ params }) => options.onCompaction?.(params))
      .onNotification('_pablo/skill', (params: unknown) => params, ({ params }) => options.onSkill?.(params))
      .onNotification('_pablo/model_attempt', (params: unknown) => params, ({ params }) => options.onModelAttempt?.(params))
      .onNotification('_pablo/v1/compaction', (params: unknown) => params, ({ params }) => options.onCompaction?.(params))
      .onNotification('_pablo/v1/skill', (params: unknown) => params, ({ params }) => options.onSkill?.(params))
      .onNotification('_pablo/v1/model_attempt', (params: unknown) => params, ({ params }) => options.onModelAttempt?.(params))
      .connectWith(stream, operation);
  } finally {
    child.stdin.end();
    // Pablo awaits cleanup on EOF. A broken/unresponsive executable still gets
    // a bounded client lifetime, with SIGTERM first so Pablo can clean up.
    const timer = setTimeout(() => child.kill('SIGTERM'), 5000);
    const killTimer = setTimeout(() => child.kill('SIGKILL'), 10000);
    try { await exited; } finally { clearTimeout(timer); clearTimeout(killTimer); }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [task, cwd = process.cwd(), ...args] = process.argv.slice(2);
  if (!task) {
    process.stderr.write('Usage: node examples/acp-client.ts "TASK" [WORKSPACE] [PABLO OPTIONS...]\n');
    process.exitCode = 2;
  } else {
    try {
      const { outcome, stopReason } = await withPablo({
        args,
        onDiagnostic: text => process.stderr.write(text),
        onUpdate: ({ update }) => {
          if (update.sessionUpdate === 'agent_message_chunk' && update.content.type === 'text') process.stdout.write(update.content.text);
          if (update.sessionUpdate === 'tool_call') {
            const input = update.rawInput as { command?: unknown; cwd?: unknown } | undefined;
            process.stderr.write(`\n${update.title}: ${JSON.stringify(input?.command)} (cwd ${JSON.stringify(input?.cwd)})\n`);
          }
          if (update.sessionUpdate === 'tool_call_update' && update.rawOutput) {
            const result = update.rawOutput as { status?: string; shell?: { exit_code?: number | null; stdout?: string; stderr?: string } };
            const shell = result.shell;
            process.stderr.write(`shell.run: ${result.status ?? update.status}${shell ? ` (exit ${shell.exit_code ?? 'none'}, stdout ${Buffer.byteLength(shell.stdout ?? '')} bytes, stderr ${Buffer.byteLength(shell.stderr ?? '')} bytes)` : ''}\n`);
          }
        },
      }, async cx => {
        const init = await cx.request('initialize', {
          protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v2': true, 'pablo/task-v2': true } },
          clientInfo: { name: 'pablo-reference', version: 'c1.3' },
        });
        if (init.protocolVersion !== 1 || init.agentCapabilities?._meta?.['pablo/v2'] !== true) throw new Error('Pablo v2 negotiation failed');
        const { sessionId } = await cx.request('session/new', { cwd: resolve(cwd), mcpServers: [] });
        const cancel = () => { void cx.notify('session/cancel', { sessionId }); };
        process.on('SIGINT', cancel);
        try {
          const response = await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: task }] });
          return { outcome: outcomeOf(response), stopReason: response.stopReason };
        }
        finally { process.off('SIGINT', cancel); }
      });
      process.stdout.write('\n');
      process.stderr.write(`pablo outcome: ${describeOutcome(outcome)}\n`);
      if (stopReason === 'cancelled' && outcome.status !== 'cancelled') process.stderr.write('ACP turn cancelled after the runtime settled\n');
      process.exitCode = stopReason === 'cancelled' ? 130 : outcome.status === 'completed' ? 0 : 1;
    } catch (error) {
      let message = 'Pablo ACP request failed';
      if (error instanceof RequestError) {
        try {
          const data = error.data as Record<string, unknown> | undefined;
          message = `pablo outcome: ${describeOutcome(parseOutcome(data?.['pablo/v2'] ?? data?.['pablo/v1']))}`;
        } catch { /* An ordinary protocol error has no negotiated run outcome. */ }
      }
      process.stderr.write(`${message}\n`);
      process.exitCode = 1;
    }
  }
}
