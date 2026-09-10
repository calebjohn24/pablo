/** Offline wire builder based on the pinned OR01 response resource. */
import { readFile } from 'node:fs/promises';
const fixture = JSON.parse(await readFile(new URL('../../docs/project/fixtures/c3-open-responses/roundtrip.json', import.meta.url), 'utf8'));
export function responsesEvents(request: any, output: { chunks?: string[], call?: { id: string, name: string, arguments: string } }) {
  const events: any[] = [];
  const emit = (type: string, fields: object) => events.push({ type, sequence_number: events.length, ...structuredClone(fields) });
  const response = (items: any[], status: string) => ({ ...structuredClone(fixture.turns[2].events.at(-1).response),
    id: 'resp_measure', model: request.model, instructions: request.instructions, tools: request.tools ?? [], tool_choice: request.tool_choice ?? 'none',
    max_output_tokens: request.max_output_tokens, output: items, status, completed_at: status === 'completed' ? 1789056001 : null, usage: null });
  emit('response.created', { response: response([], 'in_progress') }); emit('response.in_progress', { response: response([], 'in_progress') });
  let item: any;
  if (output.call) {
    const call = output.call;
    item = { type: 'function_call', id: 'fc_measure', call_id: call.id, name: call.name, arguments: '', status: 'in_progress' };
    emit('response.output_item.added', { output_index: 0, item });
    emit('response.function_call_arguments.delta', { output_index: 0, item_id: item.id, delta: call.arguments });
    emit('response.function_call_arguments.done', { output_index: 0, item_id: item.id, arguments: call.arguments });
    item.arguments = call.arguments; item.status = 'completed';
  } else {
    const chunks = output.chunks ?? [];
    item = { type: 'message', id: 'msg_measure', role: 'assistant', status: 'in_progress', content: [] };
    const part = { type: 'output_text', text: '', annotations: [] };
    const at = { output_index: 0, item_id: item.id, content_index: 0 };
    emit('response.output_item.added', { output_index: 0, item }); emit('response.content_part.added', { ...at, part });
    for (const chunk of chunks) emit('response.output_text.delta', { ...at, delta: chunk });
    part.text = chunks.join(''); emit('response.output_text.done', { ...at, text: part.text }); emit('response.content_part.done', { ...at, part });
    item.content = [part]; item.status = 'completed';
  }
  emit('response.output_item.done', { output_index: 0, item }); emit('response.completed', { response: response([item], 'completed') });
  return events;
}
export const responsesWire = (events: any[], done = true) => events.map(e => `event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`).join('') + (done ? 'data: [DONE]\n\n' : '');
