import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { Ajv2020 } from 'ajv/dist/2020.js';
const ajv = new Ajv2020({ strict: false, validateFormats: false });
const validate = ajv.compile(JSON.parse(readFileSync(new URL('../../docs/pablo-acp-notifications-v1.schema.json', import.meta.url), 'utf8')));
/** Validate actual peer notifications, including captured and redacted content. */
export function assertExtension(value: unknown): void {
  assert(validate(value), ajv.errorsText(validate.errors));
  const notification = value as Record<string, any>;
  for (const [field, bound] of [['summary', 1048576], ['instructions', 262144]] as const) {
    if (typeof notification[field] === 'string') {
      const bytes = Buffer.byteLength(notification[field]);
      assert(bytes <= bound);
      assert.equal(bytes, field === 'summary' ? notification.summary_bytes : notification.skill.instruction_bytes);
    }
  }
}
