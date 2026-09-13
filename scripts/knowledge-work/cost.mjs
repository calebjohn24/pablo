// Standard text pricing verified from the official model page on 2026-09-12.
export const astraPricing={model:'gpt-6-astra',verified:'2026-09-12',source:'https://developers.openai.com/api/docs/models/gpt-6-astra',usd_per_million:{input:10,cached_input:1,cache_write:12.5,output:50},long_context_threshold:272000};
export function astraCost(usage) {
  const unknown=reason=>({estimated_cost_usd:null,cost_estimate_unavailable_reason:reason});
  if(!usage)return unknown('No usage reported');
  const {input_tokens:input,cached_input_tokens:cached,cache_write_input_tokens:writes,output_tokens:output}=usage;
  if(![input,cached,writes,output].every(v=>Number.isSafeInteger(v)&&v>=0)||cached+writes>input)return unknown('Incomplete or inconsistent token breakdown');
  if(input>astraPricing.long_context_threshold)return unknown('Aggregate usage cannot determine per-request long-context pricing');
  return {estimated_cost_usd:((input-cached-writes)*10+cached+writes*12.5+output*50)/1e6,cost_estimate_basis:{...astraPricing,assumptions:'Standard tier, text-only, cache tokens included in total input, output includes reasoning; no hosted tool charges. Estimate, not an account billing receipt.'}};
}
