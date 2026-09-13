/** Explicit benchmark profiles, never runtime model-specific defaults. */
export function latencyProfiles(harnesses) {
  if(harnesses.some(h=>!['pablo','pablo-astra','pi'].includes(h)))throw Error('Latency matrix supports Pablo, Pablo Astra and Pi');
  return harnesses.flatMap(h=>h==='pi'?[{id:'pi-low',harness:h,piThinking:'low'}]:['provider_default','low'].map(reasoning=>({id:`${h}-${reasoning}`,harness:h,reasoning})));
}
/** Whitelist metadata: never copy text, arguments, messages or private continuation. */
export function traceDiagnostics(trace) {
  const calls=new Map();
  for(const e of trace) {
    if(e.type==='model.started')calls.set(e.span_id,{started_us:e.timestamp_unix_micros,model:e.model});
    if(e.type==='model.finished') {
      const c=calls.get(e.span_id)??{};
      c.status=e.status;c.usage=e.usage;c.output_bytes=e.output_bytes;
      c.elapsed_us=Number(e.timestamp_unix_micros)-Number(c.started_us);
      const keys=['requested_reasoning','reported_reasoning_effort','reasoning_tokens','http_version','preparation_us','dispatch_us','headers_us','first_data_us','first_text_us','first_tool_delta_us','terminal_us','complete_us'];
      c.diagnostics=Object.fromEntries(keys.filter(k=>e.diagnostics?.[k]!==undefined).map(k=>[k,e.diagnostics[k]]));
      calls.set(e.span_id,c);
    }
  }
  return [...calls.values()];
}

export function traceTimings(trace, wallMs) {
  const calls=traceDiagnostics(trace), starts=new Map(), tools=[];
  for(const e of trace) {
    if(e.type==='tool.started')starts.set(e.span_id,{at:e.timestamp_unix_micros,name:e.call?.name});
    if(e.type==='tool.finished') {
      const start=starts.get(e.span_id);
      tools.push({name:start?.name??e.name,elapsed_us:start?Number(e.timestamp_unix_micros)-Number(start.at):null});
    }
  }
  const start=trace.find(e=>e.type==='run.started'),end=trace.findLast(e=>e.type==='run.finished');
  const runtime_us=start&&end?Number(end.timestamp_unix_micros)-Number(start.timestamp_unix_micros):null;
  const model_us=calls.every(c=>Number.isFinite(c.diagnostics?.complete_us))?calls.reduce((s,c)=>s+c.diagnostics.complete_us,0):null;
  const tool_us=tools.every(t=>Number.isFinite(t.elapsed_us))?tools.reduce((s,t)=>s+t.elapsed_us,0):null;
  return {runtime_us,model_us,tool_us,tools,runtime_other_us:runtime_us!=null&&model_us!=null&&tool_us!=null?runtime_us-model_us-tool_us:null,process_other_us:runtime_us!=null&&Number.isFinite(wallMs)?wallMs*1000-runtime_us:null};
}
