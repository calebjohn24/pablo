/** Minimal bounded OTLP test decoder. Tags follow opentelemetry-proto 0.32.0. */
import assert from 'node:assert/strict';
function fields(bytes: Buffer): Map<number, Buffer[]> {
  assert(bytes.length <= 1024 * 1024);
  let offset = 0;
  const values = new Map<number, Buffer[]>();
  const varint = () => {
    let n = 0n;
    for (let shift = 0n; shift < 70n; shift += 7n) {
      assert(offset < bytes.length);
      const b = bytes[offset++]; n |= BigInt(b & 127) << shift;
      if (!(b & 128)) return n;
    }
    throw new Error('invalid protobuf varint');
  };
  while (offset < bytes.length) {
    const tag = Number(varint()), key = tag >>> 3, wire = tag & 7;
    assert(key > 0);
    if (wire === 0) { varint(); continue; }
    const length = wire === 2 ? Number(varint()) : wire === 1 ? 8 : wire === 5 ? 4 : -1;
    assert(length >= 0 && length <= bytes.length - offset);
    const value = bytes.subarray(offset, offset + length); offset += length;
    const list = values.get(key) ?? []; list.push(value); values.set(key, list);
  }
  return values;
}
export function otlpSpans(payload: Buffer) {
  return (fields(payload).get(1) ?? []).flatMap(resource =>
    (fields(resource).get(2) ?? []).flatMap(scope =>
      (fields(scope).get(2) ?? []).map(encoded => {
        const span = fields(encoded);
        const hex = (tag: number) => span.get(tag)?.[0].toString('hex') ?? '';
        const attributes: Record<string, string> = {};
        for (const value of span.get(9) ?? []) {
          const pair = fields(value), key = pair.get(1)?.[0].toString();
          const string = fields(pair.get(2)![0]).get(1)?.[0].toString();
          if (key && string !== undefined) attributes[key] = string;
        }
        return {trace_id:hex(1),span_id:hex(2),parent_span_id:hex(4),name:span.get(5)?.[0].toString(),attributes};
      })));
}
