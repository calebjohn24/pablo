/** Explicit real-Collector acceptance. Ordinary tests use local HTTP fixtures. */
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { body, cleanEnv, content, exercise, exporterSecret, parentId, server, traceId, traceState } from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import { collectorBinary, lock } from './lib/collector.mjs';

const filesystem = process.argv.includes("--filesystem");
const binary = await collectorBinary();
const received: any[] = [];
const proof = await server(async (req, res) => {
  assert.equal(req.url, '/v1/traces');
  assert(req.headers['content-type']?.includes('application/json'));
  received.push(JSON.parse((await body(req)).toString()));
  res.writeHead(200, { 'content-type': 'application/json' }); res.end('{}');
});
// Reserve an ephemeral port, then release it immediately before starting Collector.
const reservation = await server((_req, res) => { res.end(); });
const listen = `127.0.0.1:${reservation.port}`;
await reservation.close();
const child = spawn(binary, ['--config', fileURLToPath(new URL('../tests/fixtures/collector/config.yaml', import.meta.url))], {
  env: { ...cleanEnv(), PABLO_COLLECTOR_LISTEN: listen, PABLO_COLLECTOR_PROOF: proof.url },
  stdio: ['ignore', 'pipe', 'pipe'],
});
const exited = once(child, 'exit'); exited.catch(() => {});
let diagnostics = '';
child.stderr.on('data', b => { diagnostics += b; }); child.stdout.on('data', b => { diagnostics += b; });
try {
  let ready = false;
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) throw new Error(`Collector exited during startup: ${diagnostics}`);
    try { await fetch(`http://${listen}/v1/traces`, { signal: AbortSignal.timeout(100) }); ready = true; break; } catch { await delay(50); }
  }
  assert(ready, 'Collector did not listen');
  const result = await exercise({ OTEL_TRACES_EXPORTER: 'otlp', OTEL_EXPORTER_OTLP_ENDPOINT: `http://${listen}`,
    OTEL_EXPORTER_OTLP_HEADERS: `authorization=Bearer%20${exporterSecret}`,
    OTEL_RESOURCE_ATTRIBUTES: 'service.name=lower-precedence,deployment.environment.name=fixture', OTEL_SERVICE_NAME: 'pablo-collector-proof',
  }, {filesystem});
  for (let i = 0; received.length === 0 && i < 100; i++) await delay(20);
  const resources = received.flatMap(r => r.resourceSpans);
  assert(resources.length > 0, 'real Collector must forward accepted spans');
  const spans = resources.flatMap(r => r.scopeSpans.flatMap((s: any) => s.spans));
  assert.equal(spans.length, 4);
  const root = spans.find(s => s.name === 'invoke_agent pablo'); assert(root);
  assert.equal(root.parentSpanId, parentId); assert.equal(root.traceId, traceId); assert.equal(root.traceState, traceState);
  assert.equal(root.flags & 0x300, 0x300, 'parent is known and remote');
  assert.equal(spans.filter(s => s.name.startsWith('chat ')).length, 2);
  assert.equal(spans.filter(s => s.name === (filesystem ? 'execute_tool fs.read' : 'execute_tool shell.run')).length, 1);
  const events = result.events;
  for (const span of spans) {
    assert.equal(span.traceId, traceId); assert.equal(span.traceState, traceState);
    if (span !== root) assert.equal(span.parentSpanId, root.spanId);
    const native = events.filter(e => e.span_id === span.spanId); assert(native.length >= 2);
    assert.equal(BigInt(span.startTimeUnixNano), BigInt(native[0].timestamp_unix_micros) * 1000n);
    assert.equal(BigInt(span.endTimeUnixNano), BigInt(native.at(-1).timestamp_unix_micros) * 1000n);
    assert(native.every(e => e.parent_span_id === span.parentSpanId && e.trace_id === span.traceId));
    assert(!span.events?.length);
  }
  for (const resource of resources) {
    const attrs = Object.fromEntries(resource.resource.attributes.map((a: any) => [a.key, a.value.stringValue]));
    assert.equal(attrs['service.name'], 'pablo-collector-proof');
    assert.equal(attrs['service.version'], '0.1.0-dev.1');
    assert.equal(attrs['deployment.environment.name'], 'fixture');
    for (const scope of resource.scopeSpans) {
      assert.equal(scope.scope.name, 'pablo');
      assert.equal(scope.schemaUrl, 'https://opentelemetry.io/schemas/gen-ai-dev/1.42.0-dev');
    }
  }
  const serialized = JSON.stringify(received) + diagnostics;
  for (const secret of [content, exporterSecret, 'synthetic-provider', 'printf', 'synthetic-baggage']) assert(!serialized.includes(secret));
  assert(!result.stderr.includes('telemetry'));
  console.log(JSON.stringify({ collector: lock.version, filesystem, platform: `${process.platform}/${process.arch}`, spans: spans.length,
    nativeEvents: events.length, remoteParent: true, exactIdsAndTimestamps: true, metadataOnly: true, elapsedMs: Math.round(result.elapsedMs) }));
} finally {
  child.kill('SIGTERM');
  const timer = setTimeout(() => child.kill('SIGKILL'), 3000);
  try { await exited; } finally { clearTimeout(timer); await proof.close(); }
}
assert.equal(child.exitCode, 0, 'Collector must shut down cleanly');
