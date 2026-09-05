import assert from 'node:assert/strict';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { once } from 'node:events';
import { mkdtemp, readFile, realpath, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { withPablo, outcomeOf } from '../../examples/acp-client.ts';

export const traceId = 'a123456789abcdef0123456789abcdef';
export const parentId = 'b123456789abcdef';
export const traceparent = `00-${traceId}-${parentId}-01`;
export const exporterSecret = 'synthetic-exporter-credential-c15';
export const providerSecret = 'synthetic-provider-credential-c15';
export const content = 'synthetic-task-content-c15';
export const traceState = 'pablofixture=parent';
export function cleanEnv(): NodeJS.ProcessEnv {
  return Object.fromEntries(Object.entries(process.env).filter(([key]) =>
    !key.startsWith('OTEL_') && !key.startsWith('PABLO_') && !['AI_GATEWAY_API_KEY', 'VERCEL_AI_GATEWAY'].includes(key)));
}
export async function server(handler: (req: IncomingMessage, res: ServerResponse) => Promise<void> | void) {
  const errors: unknown[] = [];
  const instance = createServer((req, res) => { Promise.resolve().then(() => handler(req, res)).catch(error => { errors.push(error); res.destroy(); }); });
  instance.listen(0, '127.0.0.1'); await once(instance, 'listening');
  const address = instance.address(); assert(address && typeof address !== 'string');
  return { url: `http://127.0.0.1:${address.port}`, port: address.port,
    async close() { instance.closeAllConnections(); await new Promise<void>(resolve => instance.close(() => resolve())); assert.deepEqual(errors, []); } };
}
export async function body(req: IncomingMessage) {
  const chunks: Buffer[] = []; let size = 0;
  for await (const chunk of req) { size += chunk.length; assert(size <= 1024 * 1024); chunks.push(chunk); }
  return Buffer.concat(chunks);
}
const frame = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;

export async function exercise(env: NodeJS.ProcessEnv, options: { parent?: string; state?: string; extended?: boolean; cancel?: boolean } = {}) {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-collector-')));
  const path = join(cwd, 'trace.jsonl');
  const requests: any[] = []; let stdout = ''; let stderr = ''; let calls = 0;
  let childExit: { code: number | null; signal: NodeJS.Signals | null } | undefined;
  const gateway = await server(async (req, res) => {
    assert.equal(req.headers.authorization, 'Bearer pablo-local-fixture');
    assert(!req.headers.traceparent && !req.headers.baggage);
    const request = JSON.parse((await body(req)).toString()); requests.push(request); calls++;
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (calls === 1) {
      const command = options.cancel ? 'sleep 30' : `printf '${content}'; env`;
      res.end(frame({ tool_calls: [{ index: 0, id: 'call_c15', type: 'function', function: {
        name: 'shell_run', arguments: JSON.stringify({ command, cwd: '.' }),
      } }] }, 'tool_calls') + 'data: [DONE]\n\n');
    } else {
      const tool = JSON.parse(request.messages.at(-1).content);
      assert.equal(tool.shell.exit_code, 0);
      assert(tool.shell.stdout.includes(content));
      for (const forbidden of ['OTEL_', providerSecret, exporterSecret, traceId, traceState, 'synthetic-baggage']) assert(!tool.shell.stdout.includes(forbidden));
      res.end(frame({ content }) + frame({}, 'stop') + 'data: [DONE]\n\n');
    }
  });
  const start = performance.now();
  try {
    let terminal: any;
    await withPablo({ env: { ...cleanEnv(), ...env, AI_GATEWAY_API_KEY: providerSecret, PABLO_FIXTURE_ENDPOINT: `${gateway.url}/v1/chat/completions` },
      args: ['--model', 'fixture/collector', '--trace', path, '--capture-content', '--max-model-calls', '2', '--max-tool-calls', '1'],
      onSpawn: child => {
        child.stdout.on('data', chunk => { stdout += chunk; });
        child.on('exit', (code, signal) => { childExit = { code, signal }; });
      },
      onDiagnostic: text => { stderr += text; },
      onUpdate: async ({ sessionId, update }, cx) => {
        if (options.cancel && update.sessionUpdate === 'tool_call') await cx.notify('session/cancel', { sessionId });
      },
    }, async cx => {
      await cx.request('initialize', { protocolVersion: 1, clientCapabilities: options.extended === false ? {} : { _meta: { 'pablo/v1': true } } });
      const { sessionId } = await cx.request('session/new', { cwd, mcpServers: [] });
      const response = await cx.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: content }],
        _meta: { 'pablo/v1': { traceparent: options.parent ?? traceparent, tracestate: options.state ?? traceState, baggage: 'secret=synthetic-baggage' } },
      });
      if (options.extended !== false) {
        terminal = outcomeOf(response); assert.equal(terminal.status, options.cancel ? 'cancelled' : 'completed');
        if (!options.cancel) assert.equal(terminal.output, content);
      }
    });
    assert.deepEqual(childExit, { code: 0, signal: null }, 'Pablo must exit without client-enforced termination');
    const native = await readFile(path, 'utf8');
    const events = native.trim().split('\n').map(line => JSON.parse(line));
    assert.equal(events.filter(e => e.type === 'run.finished').length, 1);
    assert.equal(events.at(-1).outcome.status, options.cancel ? 'cancelled' : 'completed');
    assert.equal(calls, options.cancel ? 1 : 2);
    for (const secret of [exporterSecret, providerSecret, 'pablo-local-fixture', 'synthetic-baggage']) {
      assert(!(stdout + stderr + native + JSON.stringify(requests)).includes(secret), `serialized credential: ${secret}`);
    }
    assert(!JSON.stringify(requests).includes(traceId));
    assert(!JSON.stringify(requests).includes(traceState));
    return { events, stdout, stderr, terminal, elapsedMs: performance.now() - start };
  } finally { await gateway.close(); await rm(cwd, { recursive: true, force: true }); }
}
