/** E01 combined fixture shared by ACP acceptance, Collector proof and measurements. */
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdtemp,mkdir,readFile,realpath,rm,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './telemetry.ts';
import {withPablo,taskOf} from '../../examples/acp-client.ts';
const root=fileURLToPath(new URL('../../',import.meta.url));
export const python=join(root,'.pablo/mcp-fixture-venv/bin/python');
export type E01Mode='complete'|'cancel'|'denied';
const wire=(delta:object,finish='stop')=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\ndata: [DONE]\n\n`;
export async function extensibilityFixture(mode:E01Mode='complete',otel=''){
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-e01-')));
  const skillBody='PRIVATE_E01_SKILL: retain exact input values and workspace scope.';
  const evidence=JSON.stringify({base:40,padding:'PRIVATE_E01_EVIDENCE'+'x'.repeat(6000)});
  const skillData=JSON.stringify({delta:1,padding:'PRIVATE_E01_RESOURCE'+'x'.repeat(6000)});
  const artifact='{"delta":1}',reference={path:'artifact.json',revision:createHash('sha256').update(artifact).digest('hex')};
  await mkdir(join(cwd,'skills/combine'),{recursive:true});
  await writeFile(join(cwd,'skills/combine/SKILL.md'),`---\nname: combine\ndescription: Combine selected evidence.\n---\n${skillBody}\n`);
  await writeFile(join(cwd,'skills/combine/delta.json'),skillData);
  await writeFile(join(cwd,'mcp.token'),'synthetic-mcp-token',{mode:0o600});
  await writeFile(join(cwd,'evidence.txt'),evidence);await writeFile(join(cwd,reference.path),artifact);
  let step=0,summaries=0,rootCalls=0,childCalls=0,source:any,summary:any,initialTools:any,initialSystem:any;
  let onCancel:()=>Promise<void>=async()=>{throw new Error('cancellation callback not installed');};
  let finishA:()=>void=()=>{},finishB:()=>void=()=>{};
  const entered=new Set<string>();
  const gateway=await server(async(req,res)=>{
    const r=JSON.parse((await body(req)).toString());
    const messages=r.messages;
    const results=messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
    const finish=(value:unknown)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end(wire({content:JSON.stringify(value)}));};
    const call=(name:string,args:object)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end(wire({tool_calls:[{index:0,id:`e01-${rootCalls}-${childCalls}`,type:'function',function:{name,arguments:JSON.stringify(args)}}]},'tool_calls'));};
    if(r.model==='zai/glm-5.3-flash'){
      childCalls++;
      const input=JSON.parse(messages.find((m:any)=>m.role==='user').content);
      assert(!JSON.stringify(messages).includes('PRIVATE_E01_ROOT'));
      if(input.task==='source-A'){
        assert.deepEqual(JSON.parse(input.context[0]),{base:40,delta:1});
        assert(!entered.has('A'));entered.add('A');finishA=()=>finish({answer:41});
        if(entered.has('B'))finishA();return;
      }
      if(input.task==='source-B'){
        assert(!entered.has('B'));entered.add('B');finishB=()=>finish(reference);
        assert.deepEqual(JSON.parse(input.context[0]),reference);
        if(entered.has('A'))finishA();return;
      }
      assert.equal(input.task,'combine handoffs');
      const selected=input.context.map((s:string)=>JSON.parse(s).handoff);
      assert.equal(selected[0].selection.result_id,source.result_id);
      assert.deepEqual(selected[0].accounting,source.accounting);
      assert.deepEqual(selected[0].validation,source.validation);
      assert.deepEqual(selected[0].trace,source.trace);
      assert.deepEqual(selected[1].value,reference);
      assert(!JSON.stringify(messages).includes('source-A'));
      if(results.length===0){call('fs_read',{path:selected[1].value.path});return;}
      assert.equal(results[0].filesystem.revision,reference.revision);
      finish({answer:selected[0].value.answer+JSON.parse(results[0].filesystem.text).delta});return;
    }
    rootCalls++;assert.equal(r.model,'z-ai/glm-5.3-flash');
    const system=messages.find((m:any)=>m.role==='system').content;
    if(initialTools===undefined){initialTools=r.tools;initialSystem=system;}
    assert.deepEqual(r.tools,initialTools);assert.equal(system,initialSystem);
    assert(!system.includes(skillBody));assert(messages.some((m:any)=>m.content?.includes(skillBody)));
    const last=messages.at(-1).content;
    const mcp=r.tools.find((t:any)=>t.function.name.startsWith('mcp_')).function.name;
    if(typeof last==='string'&&last.startsWith('Create a concise handoff')){
      summaries++;assert.equal(summaries,1);assert.equal(r.tool_choice,'none');
      assert.equal(step,6);assert(JSON.stringify(messages).includes('PRIVATE_E01_EVIDENCE'));
      source=results[4].subagent.settled[0];assert.equal(source.validation.status,'valid');
      summary={goal:'Combine evidence; keep workspace scope and exact values.',completed:{agent_id:source.agent.agent_id,result_id:source.result_id},active:results[3].subagent.agent.agent_id,next:'Inspect completed result, use MCP again, wait active child, then combine selected handoffs.'};
      const pid=Number(await readFile(join(cwd,'pid'),'utf8'));process.kill(-pid,0);
      finish(summary);return;
    }
    if(summaries){
      const compacted=messages.find((m:any)=>m.role==='user'&&m.content.startsWith('Derived history summary'));
      assert(compacted);assert.deepEqual(JSON.parse(compacted.content.slice(compacted.content.indexOf('\n')+1)),summary);
      assert(!JSON.stringify(messages).includes('PRIVATE_E01_EVIDENCE')||step>=8);
      if(step===6){assert.equal(results.length,0);assert(!JSON.stringify(messages).includes('PRIVATE_E01_RESOURCE'));}
    }
    const current=step++;
    const capabilities={tools:[],skills:[],mcp_servers:[],model_route:['secondary']};
    switch(current){
      case 0:call('skill_read',{skill:'host/combine',path:'delta.json'});return;
      case 1:assert.equal(results[0].skill.resource.text,skillData);call(mcp,{});return;
      case 2:{
        assert.equal(results[1].mcp.content.structured.text,evidence);
        const base=JSON.parse(results[1].mcp.content.structured.text).base,delta=JSON.parse(results[0].skill.resource.text).delta;
        call('subagent',{action:'spawn',request:{input:'source-A',context:[JSON.stringify({base,delta})],capabilities,output_schema:{type:'object'}}});return;
      }
      case 3:call('subagent',{action:'spawn',request:{input:'source-B',context:[JSON.stringify(reference)],capabilities,output_schema:{type:'object'}}});return;
      case 4:call('subagent',{action:'wait',agent_ids:[results[2].subagent.agent.agent_id],mode:'all',timeout_ms:5000});return;
      case 5:
        assert.equal(results[4].subagent.settled[0].outcome.status,'completed');assert(entered.has('B'));
        res.writeHead(400,{'content-type':'application/json'});res.end(JSON.stringify({error:{code:'context_length_exceeded',message:'synthetic forced compaction'}}));return;
      case 6:
        if(mode==='cancel'){await onCancel();return;}
        if(mode==='denied'){call('subagent',{action:'spawn',request:{input:'forbidden write',capabilities:{...capabilities,tools:['fs.write']}}});return;}
        call('subagent',{action:'inspect',agent_id:summary.completed.agent_id});return;
      case 7:
        if(mode==='denied'){assert.equal(results[0].status,'recoverable_error');assert.equal(results[0].subagent.action,'error');call('subagent',{action:'stop',agent_id:summary.active});return;}
        assert.equal(results[0].subagent.snapshot.result_id,summary.completed.result_id);assert.deepEqual(results[0].subagent.snapshot,source);call(mcp,{});return;
      case 8:
        if(mode==='denied'){assert.equal(results[1].subagent.snapshot.outcome.status,'cancelled');finish({denied:true});return;}
        assert.equal(results[1].mcp.content.structured.text,evidence);finishB();call('subagent',{action:'wait',agent_ids:[summary.active],mode:'all',timeout_ms:5000});return;
      case 9:{
        const second=results[2].subagent.settled[0];assert.equal(second.validation.status,'valid');
        call('subagent',{action:'spawn',request:{input:'combine handoffs',capabilities:{...capabilities,tools:['fs.read']},output_schema:{type:'object',properties:{answer:{const:42}},required:['answer']},handoffs:[source,second].map((s,i)=>({source_agent_id:s.agent.agent_id,result_id:s.result_id,kind:i===0?'inline':'artifact'}))}});return;
      }
      case 10:call('subagent',{action:'wait',agent_ids:[results[3].subagent.agent.agent_id],mode:'all',timeout_ms:5000});return;
      case 11:
        assert.deepEqual(JSON.parse(results[4].subagent.settled[0].outcome.output),{answer:42});
        finish({answer:42});return;
      default:throw new Error(`unexpected root step ${current}`);
    }
  });
  const entry=join(cwd,'entry.toml');
  const base=(await readFile(join(root,'docs/project/fixtures/c3-model-routes/three-providers.toml'),'utf8')).replace('max_model_calls=2','max_model_calls=30').replace('max_tool_calls=1','max_tool_calls=20');
  await writeFile(entry,base+`
[options.children]
enabled=true
[options.context]
keep_recent_turns=0
max_summary_tokens=512
[options.skills]
activate=["host/combine"]
[options.skills.roots]
host={base="workspace",path="skills"}
[options.output]
schema='{"type":"object"}'
[options.trace]
path={base="workspace",path="trace.jsonl"}
[credentials.mcp]
consumer="mcp.env"
sources=[{kind="file",path={base="workspace",path="mcp.token"},encoding="utf8"}]
[options.mcp.servers.local]
transport="stdio"
command=${JSON.stringify(python)}
args=[${JSON.stringify(join(root,'tests/fixtures/mcp/server.py'))}]
env={FIXTURE_TOKEN="mcp"}
${otel}
`);
  const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
  const env=cleanEnv();
  return {cwd,args,env,setCancel:(callback:()=>Promise<void>)=>{onCancel=callback;},source:()=>source,counts:()=>({rootCalls,childCalls,summaries}),
    async run(binary=join(root,'target/debug/pablo')){
      return withPablo({binary,args,env},async cx=>{
        await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true,'pablo/skills-v1':true,'pablo/compaction-v1':true}}});
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
        onCancel=()=>cx.notify('session/cancel',{sessionId});
        return taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'PRIVATE_E01_ROOT: combine evidence, preserve workspace scope.'}]}));
      });
    },
    async verify(task:any){
      assert.equal(rootCalls,mode==='complete'?13:mode==='cancel'?8:10);
      assert.equal(childCalls,mode==='complete'?4:2);
      assert.equal(summaries,1);assert.equal(task.outcome.status,mode==='cancel'?'cancelled':'completed');
      if(mode!=='cancel')assert.deepEqual(JSON.parse(task.outcome.output),mode==='denied'?{denied:true}:{answer:42});
      assert.equal(task.accounting.model_calls,String(rootCalls+childCalls));
      assert.equal(task.accounting.tool_calls,mode==='complete'?'11':mode==='cancel'?'5':'7');
      const pid=Number(await readFile(join(cwd,'pid'),'utf8'));assert.throws(()=>process.kill(-pid,0),(e:any)=>e.code==='ESRCH');
      const text=await readFile(join(cwd,'trace.jsonl'),'utf8');
      for(const secret of [skillBody,'PRIVATE_E01_ROOT','PRIVATE_E01_RESOURCE','PRIVATE_E01_EVIDENCE','synthetic-mcp-token'])assert(!text.includes(secret));
      const events=text.trim().split('\n').map(s=>JSON.parse(s));
      assert.deepEqual(events.map(e=>e.root_seq),events.map((_,i)=>i+1));
      const compacted=events.filter(e=>e.type==='context.compaction.finished');assert.equal(compacted.length,1);assert.equal(compacted[0].compaction.status,'completed');
      assert.equal(compacted[0].summary,null);assert(Number(compacted[0].compaction.after_bytes)<Number(compacted[0].compaction.before_bytes));
      const children=events.filter(e=>e.type==='run.finished'&&e.agent.depth===1);
      assert.equal(children.length,mode==='complete'?3:2);assert.equal(events.at(-1).agent.depth,0);
      const completed=children.find(e=>e.agent.agent_id===summary.completed.agent_id),active=children.find(e=>e.agent.agent_id===summary.active);
      assert(completed&&active);assert.equal(completed.outcome.status,'completed');
      assert(completed.root_seq<compacted[0].root_seq&&active.root_seq>compacted[0].root_seq);
      assert.equal(active.outcome.status,mode==='complete'?'completed':'cancelled');
      const mcp=events.filter(e=>e.type==='tool.started'&&e.call.name==='mcp/local/read_evidence');
      assert.equal(mcp.length,mode==='complete'?2:1);assert(mcp[0].root_seq<compacted[0].root_seq);
      if(mode==='complete')assert(mcp[1].root_seq>compacted[0].root_seq);
      assert.equal(events.filter(e=>e.type==='skill.activated').length,1);
      return {events,compaction:compacted[0].compaction,context:JSON.parse(await readFile(join(cwd,'context.json'),'utf8')),counts:{rootCalls,childCalls,summaries}};
    },
    async close(){try{await gateway.close();}finally{await rm(cwd,{recursive:true,force:true});}}
  };
}
