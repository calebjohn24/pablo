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
  | { status: 'policy_denied'; rule: string }
  | { status: 'limit_exceeded'; limit: string }
  | { status: 'failed'; code: string; delivery: string };

export function outcomeOf(response: PromptResponse): RunOutcome {
  return parseOutcome(response._meta?.['pablo/v1']);
}

function parseOutcome(value: unknown): RunOutcome {
  const meta = value as Record<string, unknown> | undefined;
  const outcome = meta?.outcome as RunOutcome | undefined;
  if (!outcome || typeof outcome !== 'object') throw new Error('Missing negotiated Pablo outcome');
  switch (outcome.status) {
    case 'completed':
      if (typeof outcome.output !== 'string' || outcome.finish_reason !== 'stop' || !outcome.usage ||
          !['input_tokens', 'output_tokens', 'cache_read_input_tokens', 'cache_write_input_tokens'].every(key => {
            const value = (outcome.usage as Record<string, unknown>)[key];
            return value === null || (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0);
          })) throw new Error('Invalid completed outcome');
      return outcome;
    case 'cancelled': case 'timed_out': return outcome;
    case 'policy_denied': if (typeof outcome.rule === 'string') return outcome; break;
    case 'limit_exceeded': if (typeof outcome.limit === 'string') return outcome; break;
    case 'failed': if (typeof outcome.code === 'string' && typeof outcome.delivery === 'string') return outcome; break;
  }
  throw new Error('Unknown or invalid Pablo outcome');
}

function describeOutcome(outcome: RunOutcome): string {
  switch (outcome.status) {
    case 'limit_exceeded': return `${outcome.status}: ${outcome.limit}`;
    case 'policy_denied': return `${outcome.status}: ${outcome.rule}`;
    case 'failed': return `${outcome.status}: ${outcome.code} (${outcome.delivery})`;
    default: return outcome.status;
  }
}

export interface ClientOptions {
  binary?: string;
  args?: string[];
  env?: NodeJS.ProcessEnv;
  onUpdate?: (notification: SessionNotification, context: ClientContext) => void | Promise<void>;
  onDiagnostic?: (text: string) => void;
  onSpawn?: (child: ChildProcessWithoutNullStreams) => void;
}

/** The callback uses ACP methods directly; this is not a second language SDK. */
export async function withPablo<T>(options: ClientOptions, operation: (context: ClientContext) => Promise<T>): Promise<T> {
  const binary = options.binary ?? fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
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
          protocolVersion: 1, clientCapabilities: { _meta: { 'pablo/v1': true } },
          clientInfo: { name: 'pablo-reference', version: 'c1.3' },
        });
        if (init.protocolVersion !== 1 || init.agentCapabilities?._meta?.['pablo/v1'] !== true) throw new Error('Pablo v1 negotiation failed');
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
          message = `pablo outcome: ${describeOutcome(parseOutcome(data?.['pablo/v1']))}`;
        } catch { /* An ordinary protocol error has no negotiated run outcome. */ }
      }
      process.stderr.write(`${message}\n`);
      process.exitCode = 1;
    }
  }
}
