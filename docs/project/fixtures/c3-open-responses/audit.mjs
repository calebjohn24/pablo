// Offline OR01 contract audit. This does not execute or certify a runtime adapter.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import Ajv2020 from 'ajv/dist/2020.js';

const bytes = path => readFile(new URL(path, import.meta.url));
const json = async path => JSON.parse(await bytes(path));
const pin = await json('upstream-pin.json');
const fixture = await json('roundtrip.json');
const profile = await json('profile.json');
const upstream = await json('upstream/openapi.json');
// OpenAPI annotation keywords (discriminator, example, x-*) do not change JSON
// Schema validation. Keep the upstream schema untouched; no remote retrieval.
const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
ajv.addSchema({ $id: 'urn:pablo:or01', ...upstream }, 'urn:pablo:or01');
const requestSchema = ajv.compile({ $ref: 'urn:pablo:or01#/components/schemas/CreateResponseBody' });
const eventSchema = ajv.compile({ ...upstream.paths['/responses'].post.responses['200'].content['text/event-stream'].schema, components: upstream.components });
function valid(validate, value) {
  assert(validate(value), JSON.stringify(validate.errors));
}

// An independent fixture consistency check, deliberately not production parsing.
// C3.9 must pass these wires through actual core/CLI/ACP before claiming OR02.
function lifecycle(turn) {
  const events = turn.events;
  assert.equal(events[0].type, 'response.created');
  const responseId = events[0].response.id;
  let previous = -1, progress = false, terminal;
  const items = [], ids = new Set(), calls = new Set();
  for (const event of events) {
    assert(!terminal, 'event after terminal');
    assert(Number.isSafeInteger(event.sequence_number) && event.sequence_number > previous);
    previous = event.sequence_number;
    if (event.response) assert.equal(event.response.id, responseId);
    if (event.type === 'response.created') {
      assert.equal(event, events[0]);
      assert.equal(event.response.status, 'in_progress');
    } else if (event.type === 'response.in_progress') {
      assert(!progress); progress = true;
      assert.equal(event.response.status, 'in_progress');
    } else if (event.type === 'response.output_item.added') {
      assert(progress); assert.equal(event.output_index, items.length);
      const item = event.item;
      assert(!ids.has(item.id)); ids.add(item.id);
      assert(['message', 'function_call', 'reasoning'].includes(item.type));
      if (item.type === 'message') {
        assert.equal(item.role, 'assistant'); assert.deepEqual(item.content, []);
      } else if (item.type === 'function_call') {
        assert(!calls.has(item.call_id)); calls.add(item.call_id);
        assert.equal(item.arguments, ''); assert.equal(item.name, 'fs_read');
      } else {
        assert(!item.content?.length); assert.deepEqual(item.summary, []);
      }
      items.push({ added: item, parts: [], arguments: '', done: false });
    } else if (event.type === 'response.completed') {
      assert(progress && items.every(item => item.done));
      assert.equal(event.response.status, 'completed');
      assert.deepEqual(event.response.output, items.map(item => item.final));
      terminal = event;
    } else {
      const item = items[event.output_index];
      assert(item && !item.done);
      if (event.item_id) assert.equal(event.item_id, item.added.id);
      if (event.type === 'response.output_item.done') {
        assert.equal(event.item.id, item.added.id);
        assert.equal(event.item.type, item.added.type);
        if (event.item.type === 'message') {
          assert.equal(event.item.status, 'completed');
          assert.equal(event.item.phase, item.added.phase);
          assert(item.parts.every(p => p.closed));
          assert.deepEqual(event.item.content, item.parts.map(p => p.final));
        } else if (event.item.type === 'function_call') {
          assert.equal(event.item.status, 'completed'); assert(item.argumentsDone);
          assert.equal(event.item.arguments, item.arguments);
          assert.equal(event.item.call_id, item.added.call_id);
          assert.equal(event.item.name, item.added.name);
          assert.equal(typeof JSON.parse(item.arguments), 'object');
        } else {
          assert(!event.item.content?.length); assert(item.parts.every(p => p.closed));
          assert.deepEqual(event.item.summary, item.parts.map(p => p.final));
          assert.equal(typeof event.item.encrypted_content, 'string');
        }
        item.done = true; item.final = event.item;
      } else if (event.type.startsWith('response.function_call_arguments.')) {
        assert.equal(item.added.type, 'function_call'); assert(!item.argumentsDone);
        if (event.type.endsWith('.delta')) item.arguments += event.delta;
        else { assert.equal(event.arguments, item.arguments); item.argumentsDone = true; }
      } else if (['response.content_part.added', 'response.reasoning_summary_part.added'].includes(event.type)) {
        const summary = event.type === 'response.reasoning_summary_part.added';
        assert.equal(item.added.type, summary ? 'reasoning' : 'message');
        assert.equal(summary ? event.summary_index : event.content_index, item.parts.length);
        assert.equal(event.part.type, summary ? 'summary_text' : 'output_text'); assert.equal(event.part.text, '');
        if (!summary) assert.deepEqual(event.part.annotations, []);
        item.parts.push({ text: '', done: false, closed: false });
      } else {
        const part = item.parts[event.content_index ?? event.summary_index]; assert(part && !part.closed);
        if (['response.output_text.delta', 'response.reasoning_summary_text.delta'].includes(event.type)) {
          assert(!part.done); part.text += event.delta;
        } else if (['response.output_text.done', 'response.reasoning_summary_text.done'].includes(event.type)) {
          assert(!part.done); assert.equal(event.text, part.text); part.done = true;
        } else if (['response.content_part.done', 'response.reasoning_summary_part.done'].includes(event.type)) {
          assert(part.done); assert.equal(event.part.text, part.text);
          part.closed = true; part.final = event.part;
        } else assert.fail('unsupported event');
      }
    }
  }
  assert(terminal); assert.equal(turn.done, '[DONE]');
  return terminal.response.output;
}

test('OR01 immutable upstream bytes, version and local-only schema references', async () => {
  assert.match(pin.revision, /^[a-f0-9]{40}$/);
  assert.equal(pin.revision, fixture.upstream_revision);
  assert.equal(upstream.info.version, pin.specification_version);
  for (const file of pin.files) {
    const data = await bytes(file.local_path);
    assert.equal(data.length, file.bytes);
    assert.equal(createHash('sha256').update(data).digest('hex'), file.sha256);
  }
  function walk(value) {
    if (value && typeof value === 'object') {
      if (value.$ref) assert(value.$ref.startsWith('#/components/schemas/'));
      Object.values(value).forEach(walk);
    }
  }
  walk(upstream);
});

test('OR01 three requests and every event validate against untouched upstream OpenAPI 3.1 schemas', () => {
  for (const turn of fixture.turns) {
    valid(requestSchema, turn.request);
    for (const event of turn.events) valid(eventSchema, event);
  }
});

test('OR01 requests preserve the selected profile, stable prefix and explicit stateless tool mode', () => {
  assert.equal(profile.id, fixture.profile); assert.equal(profile.upstream_revision, pin.revision);
  assert.equal(profile.specification_version, upstream.info.version);
  for (const turn of fixture.turns) {
    const request = turn.request;
    for (const key of ['model', 'instructions', 'tools', 'include']) {
      assert.equal(JSON.stringify(request[key]), JSON.stringify(fixture.turns[0].request[key]));
    }
    assert.equal(request.stream, true); assert.equal(request.store, false);
    assert.equal(request.background, false); assert.equal(request.parallel_tool_calls, false);
    assert.equal(request.truncation, 'disabled'); assert.equal(request.previous_response_id, undefined);
    assert(request.max_output_tokens >= profile.limits.minimum_output_tokens);
    assert(Buffer.byteLength(JSON.stringify(request)) <= profile.limits.request_bytes);
  }
});

test('OR01 actual CRLF SSE wires match golden JSON, event names and one terminal sentinel', async () => {
  for (let i = 0; i < fixture.turns.length; i++) {
    const raw = new TextDecoder('utf-8', { fatal: true }).decode(await bytes(`turn-${i + 1}.sse`));
    const frames = raw.split('\r\n\r\n').filter(Boolean).filter(f => !f.startsWith(':'));
    assert.equal(frames.pop(), 'data: [DONE]');
    const events = frames.map(frame => {
      const [name, data] = frame.split('\r\n');
      const event = JSON.parse(data.slice(6)); assert.equal(name, `event: ${event.type}`);
      return event;
    });
    assert.deepEqual(events, fixture.turns[i].events);
  }
});

test('OR01 lifecycle, phases, exact call/result correlation and opaque continuation across two tool rounds', () => {
  let history = fixture.turns[0].request.input;
  const calls = new Set();
  for (const turn of fixture.turns) {
    assert.deepEqual(turn.request.input, history);
    const output = lifecycle(turn);
    const emittedCalls = output.filter(i => i.type === 'function_call');
    assert.equal(turn.tool_results.length, emittedCalls.length);
    for (const [i, call] of emittedCalls.entries()) {
      assert(!calls.has(call.call_id)); calls.add(call.call_id);
      assert.equal(turn.tool_results[i].call_id, call.call_id);
    }
    history = [...history, ...output, ...turn.tool_results];
    const text = output.filter(i => i.type === 'message').flatMap(i => i.content).map(p => p.text).join('');
    assert.equal(text, turn.expected.text);
    for (const secret of fixture.private_sentinels) assert(!text.includes(secret));
  }
  assert.equal(calls.size, 2);
  for (const secret of fixture.private_sentinels) assert(JSON.stringify(fixture.turns[2].request.input).includes(secret));
});

test('OR01 upstream schema rejects malformed request and event shapes', () => {
  for (const mutate of [r => r.stream = 'yes', r => r.max_output_tokens = 1,
    r => r.tools[0].name = 'fs.read', r => r.input[0].content[0].type = 'unknown',
    r => r.include = ['invented']]) {
    const value = structuredClone(fixture.turns[0].request); mutate(value);
    assert.equal(requestSchema(value), false);
  }
  for (const mutate of [e => delete e.response.id, e => e.sequence_number = '0',
    e => e.type = 'unknown', e => delete e.response.output,
    e => e.response.usage = { input_tokens: 1 }]) {
    const value = structuredClone(fixture.turns[0].events[0]); mutate(value);
    assert.equal(eventSchema(value), false);
  }
});

test('OR01 rejects schema-valid but inconsistent lifecycle mutations', () => {
  const select = (t, type) => t.events.find(e => e.type === type);
  const mutations = [
    t => t.events[1].sequence_number = 0,
    t => t.events[1].response.id = 'changed',
    t => select(t, 'response.output_item.added').output_index = 7,
    t => select(t, 'response.output_text.delta').item_id = 'unknown',
    t => select(t, 'response.output_text.done').text = 'mismatch',
    t => select(t, 'response.content_part.done').part.text = 'mismatch',
    t => select(t, 'response.function_call_arguments.done').arguments = '{}',
    t => t.events.find(e => e.type === 'response.output_item.done' && e.item.type === 'function_call').item.call_id = 'changed',
    t => t.events.find(e => e.type === 'response.output_item.done' && e.item.type === 'message').item.phase = 'final_answer',
    t => t.events.at(-1).response.output = [],
    t => t.events.push({ ...t.events.at(-1), sequence_number: 1000 }),
    t => t.events.splice(t.events.findIndex(e => e.type === 'response.content_part.added'), 1),
    t => t.events.splice(t.events.findIndex(e => e.type === 'response.function_call_arguments.done'), 1),
  ];
  for (const mutate of mutations) {
    const turn = structuredClone(fixture.turns[0]); mutate(turn);
    for (const event of turn.events) valid(eventSchema, event);
    assert.throws(() => lifecycle(turn));
  }
  const noDone = structuredClone(fixture.turns[0]); noDone.done = ''; assert.throws(() => lifecycle(noDone));
});

test('OR01 token accounting keeps unknown cache-write/cost and does not double count reasoning', () => {
  const sums = { input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_write_input_tokens: null };
  for (const turn of fixture.turns) {
    const usage = turn.events.at(-1).response.usage;
    assert.equal(usage.total_tokens, usage.input_tokens + usage.output_tokens);
    assert(usage.output_tokens_details.reasoning_tokens <= usage.output_tokens);
    assert.deepEqual(turn.expected.usage, { input_tokens: usage.input_tokens, output_tokens: usage.output_tokens,
      cache_read_input_tokens: usage.input_tokens_details.cached_tokens, cache_write_input_tokens: null });
    assert.equal(turn.expected.cost_microusd, null);
    for (const key of ['input_tokens', 'output_tokens', 'cache_read_input_tokens']) sums[key] += turn.expected.usage[key];
  }
  assert.deepEqual(sums, fixture.aggregate_usage);
});

test('OR01 terminal examples are upstream-valid for completed, incomplete, failed, null and zero usage', async () => {
  const cases = await json('terminals.json');
  assert.equal(cases.length, 6);
  for (const { event, expected } of cases) {
    valid(eventSchema, event);
    assert.equal(event.type, `response.${event.response.status}`);
    const reason = event.response.incomplete_details?.reason;
    const finish = event.type === 'response.completed' ? 'stop' : reason === 'max_output_tokens' ? 'length' : null;
    const error = event.type === 'response.failed' ? 'provider_rejected' : reason === 'content_filter' ? 'unsupported_provider_content' : null;
    assert.deepEqual(expected, { finish, error });
  }
});

test('OR01 nonempty raw reasoning output has no lossless input mapping in pinned schema', () => {
  const output = structuredClone(fixture.turns[0].events[2]);
  output.item.content = [{ type: 'reasoning_text', text: 'synthetic-private-reasoning' }];
  valid(eventSchema, output);
  const request = structuredClone(fixture.turns[0].request); request.input.push(output.item);
  assert.equal(requestSchema(request), false);
  // Therefore the selected profile must reject this shape, not flatten/discard it.
  const turn = structuredClone(fixture.turns[0]); turn.events[2] = output;
  assert.throws(() => lifecycle(turn));
});
