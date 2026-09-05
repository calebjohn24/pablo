import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';
import { body, cleanEnv, content, exercise, exporterSecret, parentId, server, traceId, traceparent } from './fixtures/telemetry.ts';

const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const enabled = { OTEL_TRACES_EXPORTER: 'otlp' };

test('network export is opt-in; disabled SDK and sampling preserve native IDs', { timeout: 20000 }, async () => {
  let count = 0;
  const receiver = await server((_req, res) => { count++; res.end(); });
  try {
    for (const env of [{}, { ...enabled, OTEL_SDK_DISABLED: 'true' }, { ...enabled, OTEL_TRACES_SAMPLER: 'always_off' }, { ...enabled, OTEL_TRACES_EXPORTER: 'none' }]) {
      const result = await exercise({ ...env, OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url });
      assert.equal(result.events[0].trace_id, traceId);
      assert.equal(result.events[0].parent_span_id, parentId);
      assert(result.events.every(e => e.span_id !== '0'.repeat(16)));
      if ('OTEL_SDK_DISABLED' in env || 'OTEL_TRACES_SAMPLER' in env) assert(result.events.every(e => e.trace_flags === '00'));
    }
    assert.equal(count, 0);
  } finally { await receiver.close(); }
});

test('OTLP uses Protobuf, standard overrides, gzip and private headers', { timeout: 15000 }, async () => {
  let count = 0;
  const receiver = await server(async (req, res) => {
    count++;
    assert.equal(req.url, '/custom-traces');
    assert.equal(req.headers['content-type'], 'application/x-protobuf');
    assert.equal(req.headers['content-encoding'], 'gzip');
    assert.equal(req.headers.authorization, `Bearer ${exporterSecret}`);
    assert.equal(req.headers['x-general'], undefined);
    const payload = gunzipSync(await body(req));
    assert(payload.includes(Buffer.from('invoke_agent pablo')));
    for (const secret of [content, exporterSecret, 'printf', 'synthetic-provider']) assert(!payload.includes(Buffer.from(secret)));
    res.writeHead(200, { 'content-type': 'application/x-protobuf' }); res.end();
  });
  try {
    const result = await exercise({ ...enabled,
      OTEL_EXPORTER_OTLP_ENDPOINT: 'http://127.0.0.1:1/unused',
      OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: `${receiver.url}/custom-traces`,
      OTEL_EXPORTER_OTLP_PROTOCOL: 'grpc', OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: 'http/protobuf',
      OTEL_EXPORTER_OTLP_HEADERS: 'x-general=unused',
      OTEL_EXPORTER_OTLP_TRACES_HEADERS: `authorization=Bearer%20${exporterSecret}`,
      OTEL_EXPORTER_OTLP_COMPRESSION: 'gzip',
    });
    assert.equal(count, 1); assert(!result.stderr.includes('telemetry'));
  } finally { await receiver.close(); }
});

test('Collector rejection, retry, disconnect, stall and oversized replies cannot change outcomes', { timeout: 30000 }, async t => {
  for (const mode of ['reject', 'server-error', 'retry', 'throttle', 'disconnect', 'stall', 'oversized', 'malformed', 'partial', 'redirect', 'cancel'] as const) await t.test(mode, async () => {
    let count = 0;
    const receiver = await server(async (req, res) => {
      await body(req); count++;
      if (mode === 'stall' || mode === 'cancel') { res.writeHead(200); res.flushHeaders(); return; }
      if (mode === 'disconnect') { res.destroy(); return; }
      if ((mode === 'retry' || mode === 'throttle') && count > 1) { res.writeHead(200); res.end(); return; }
      if (mode === 'partial') { res.end(Buffer.from([0x0a, 0x02, 0x08, 0x01])); return; }
      if (mode === 'malformed') { res.end('invalid-protobuf'); return; }
      if (mode === 'oversized') { res.end(Buffer.alloc(128 * 1024)); return; }
      res.writeHead(mode === 'retry' ? 503 : mode === 'throttle' ? 429 : mode === 'server-error' ? 500 : mode === 'redirect' ? 307 : 401,
        { location: '/redirected', 'retry-after': '0' }); res.end(exporterSecret);
    });
    try {
      const result = await exercise({ ...enabled, OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url,
        OTEL_EXPORTER_OTLP_TIMEOUT: '60000', OTEL_EXPORTER_OTLP_HEADERS: `authorization=${exporterSecret}`,
      }, { cancel: mode === 'cancel' });
      assert(result.elapsedMs < 4000, `cleanup took ${result.elapsedMs} ms`);
      assert(count > 0);
      if (mode === 'retry' || mode === 'throttle') { assert.equal(count, 2); assert(!result.stderr.includes('telemetry dropped')); }
      else { assert(result.stderr.includes('telemetry dropped')); }
      if (['reject', 'server-error', 'redirect', 'partial', 'malformed'].includes(mode)) assert.equal(count, 1);
      if (mode === 'partial') assert(result.stderr.includes('dropped 1 spans'));
    } finally { await receiver.close(); }
  });
});

test('invalid or unnegotiated W3C context starts a local trace; unsampled parents stay unsampled', { timeout: 15000 }, async () => {
  for (const options of [{ parent: 'invalid-secret-parent' }, { parent: 'a'.repeat(513) }, { extended: false }]) {
    const result = await exercise({}, options);
    assert.notEqual(result.events[0].trace_id, traceId);
    assert.equal(result.events[0].parent_span_id, null);
    assert(!result.stderr.includes('invalid-secret-parent'));
  }
  const disabled = await exercise({ OTEL_PROPAGATORS: 'none' });
  assert.notEqual(disabled.events[0].trace_id, traceId);
  const unsampled = await exercise({}, { parent: `00-${traceId}-${parentId}-00` });
  assert(unsampled.events.every(e => e.trace_flags === '00'));
});

test('CLI shares exporter configuration and never echoes invalid configuration', { timeout: 10000 }, async () => {
  let count = 0;
  const receiver = await server(async (req, res) => {
    count++; assert.equal(req.url, '/v1/traces'); const payload = await body(req);
    assert(payload.includes(Buffer.from(traceId, 'hex'))); assert(payload.includes(Buffer.from(parentId, 'hex'))); res.end();
  });
  try {
    for (const config of [enabled, { ...enabled, OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: exporterSecret }]) {
      const child = spawn(binary, ['demo', '--traceparent', traceparent], { env: { ...cleanEnv(), ...config, OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url } });
      let stdout = ''; let stderr = '';
      child.stdout.on('data', b => { stdout += b; }); child.stderr.on('data', b => { stderr += b; });
      const [code] = await once(child, 'exit'); assert.equal(code, 0); assert.equal(stdout, 'Hello from pablo.\n');
      assert(!stderr.includes(exporterSecret));
    }
    assert.equal(count, 1);
  } finally { await receiver.close(); }
});

test('batch queue saturation stays bounded and reports lost spans; signal timeout overrides general timeout', { timeout: 10000 }, async () => {
  const receiver = await server(async (req, res) => { await body(req); res.writeHead(200); res.flushHeaders(); });
  try {
    const saturated = await exercise({ ...enabled, OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url,
      OTEL_BSP_MAX_QUEUE_SIZE: '1', OTEL_BSP_MAX_EXPORT_BATCH_SIZE: '1', OTEL_EXPORTER_OTLP_TIMEOUT: '60000',
    });
    assert(saturated.elapsedMs < 4000); assert(saturated.stderr.includes('telemetry dropped 4 spans'));
    const timeout = await exercise({ ...enabled, OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url,
      OTEL_EXPORTER_OTLP_TIMEOUT: '60000', OTEL_EXPORTER_OTLP_TRACES_TIMEOUT: '20',
    });
    assert(timeout.elapsedMs < 1500); assert(timeout.stderr.includes('telemetry dropped'));
  } finally { await receiver.close(); }
});
