import { resolve } from 'node:path';
export const models = {pablo:'z-ai/glm-5.3-flash', 'pablo-astra':'openai/gpt-6-astra', pi:'z-ai/glm-5.3-flash', ori:'z-ai/glm-5.3-flash', codex:'gpt-6-astra', claude:'claude-fable-5-1'};
export function command(name, {root, workspace, prompt, timeout, paths={}}) {
  const baseName = name==='pablo-astra'?'pablo':name;
  const bin = paths[name] ?? process.env[`PABLO_KNOWLEDGE_${baseName.toUpperCase()}_BINARY`] ?? ({pablo:resolve(root,'target/release/pablo'),pi:resolve(root,'.pablo/knowledge-tools/node_modules/.bin/pi'),ori:resolve(root,'.pablo/knowledge-tools/bin/ori'),codex:'codex',claude:'claude'})[baseName];
  const pi = ['-p','--mode','json','--no-session','--no-extensions','--no-skills','--no-prompt-templates','--no-themes','--no-context-files','--no-approve','--tools','read,bash,edit,write,grep,find,ls','--thinking','off'];
  switch(baseName) {
    case 'pablo': return [bin,'run',prompt,'--json','--workspace',workspace,'--allow-write','--provider','openrouter','--model',models[name],'--env-file',resolve(root,'.env'),'--timeout',String(timeout),'--trace',resolve(workspace,'../pablo-trace.jsonl')];
    case 'codex': return [bin,'exec','--ignore-user-config','--ephemeral','--skip-git-repo-check','--sandbox','workspace-write','-c','approval_policy="never"','-c','model_reasoning_effort="medium"','-c','web_search="disabled"','-c','features.multi_agent=false','-c','features.memories=false','-c','project_doc_max_bytes=0','--model',models.codex,'--json',prompt];
    case 'claude': return [bin,'-p','--safe-mode','--no-session-persistence','--strict-mcp-config','--disable-slash-commands','--tools','Read,Write,Edit,Bash,Glob,Grep','--allowedTools','Read,Write,Edit,Bash,Glob,Grep','--permission-mode','dontAsk','--model',models.claude,'--effort','high','--output-format','stream-json','--include-partial-messages','--verbose',prompt];
    case 'pi': return [bin,...pi,'--provider','openrouter','--model',models.pi,prompt];
    case 'ori': return [bin,'pi','--model',models.ori,'--reasoning-effort','none',...pi,prompt];
    default: throw Error(`Unknown harness: ${name}`);
  }
}
/** Normalize only reported events. Unknown usage, costs and first-token latency stay null. */
export function normalize(name, raw, receipts=[]) {
  if(name==='pablo-astra')name='pablo';
  let offset=0, firstAssistant=null, tools=0, usage=null, cost=null, resolvedModel=null, completed=false;
  const errors=[],events=[];
  if(name==='ori') {try {const error=JSON.parse(raw);if(error.ok===false)errors.push(error.error?.code??'ori_error');}catch{}}
  for(const line of raw.split('\n')) {
    offset += Buffer.byteLength(line)+1;
    if(!line.trim()) continue;
    let e; try {e=JSON.parse(line);} catch {continue;}
    events.push(e);
    const ms=receipts.find(r=>r.stream==='stdout'&&r.end_byte>=offset)?.ms ?? null;
    const assistant = (name==='codex'&&e.type==='item.completed'&&e.item?.type==='agent_message') || (name==='claude'&&(e.type==='assistant'&&(e.message?.content??[]).some(c=>c.type==='text'&&c.text)||e.type==='stream_event'&&e.event?.delta?.type==='text_delta')) || ((name==='pi'||name==='ori')&&e.type==='message_update'&&e.assistantMessageEvent?.type==='text_delta');
    if(assistant && firstAssistant===null) firstAssistant=ms;
    if(name==='codex') {
      if(e.type==='item.completed'&&['command_execution','file_change','mcp_tool_call','web_search'].includes(e.item?.type)) tools++;
      if(e.type==='turn.completed') {usage=e.usage??null;completed=true;}
      if(e.type==='turn.failed'||e.type==='error') errors.push(e.type);
    } else if(name==='claude') {
      if(e.type==='system'&&e.subtype==='init') resolvedModel=e.model??null;
      if(e.type==='assistant') tools+=(e.message?.content??[]).filter(c=>c.type==='tool_use').length;
      if(e.type==='result') {completed=!e.is_error;usage=e.usage??null;cost=e.total_cost_usd??null;if(e.is_error)errors.push(e.subtype??'result_error');}
    } else if(name==='pi'||name==='ori') {
      if(e.type==='agent_end')completed=true;
      if(e.type==='tool_execution_start')tools++;
      if(e.type==='message_end'&&e.message?.role==='assistant') {
        resolvedModel=e.message.model??resolvedModel;
        if(e.message.usage) {usage??={};for(const[k,v]of Object.entries(e.message.usage))if(typeof v==='number')usage[k]=(usage[k]??0)+v;
          if(typeof e.message.usage.cost?.total==='number')cost=(cost??0)+e.message.usage.cost.total;}
        if(['error','aborted'].includes(e.message.stopReason))errors.push(e.message.stopReason);
      }
    } else if(name==='pablo') {
      usage=e.accounting??e.outcome?.accounting??null;
      completed=e.outcome?.status==='completed';
      tools=usage?.tool_calls==null?null:Number(usage.tool_calls);
      cost=usage?.cost_microusd==null?null:Number(usage.cost_microusd)/1000000;
      if(e.error)errors.push(e.error.code??'task_error');
      if(e.outcome?.status && e.outcome.status!=='completed')errors.push(e.outcome.status);
    }
  }
  return {completed,first_assistant_observed_ms:firstAssistant,first_assistant_boundary:name==='codex'?'completed message event, not first token':name==='pablo'?'unavailable from buffered task envelope':'first text event received',tool_calls:tools,reported_usage:usage,reported_cost_usd:cost,reported_model:resolvedModel,errors,event_count:events.length};
}
