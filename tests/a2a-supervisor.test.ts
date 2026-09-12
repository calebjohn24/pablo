import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, taskOf } from '../examples/acp-client.ts';
const exec=promisify(execFile);
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(root,'target/debug/pablo');
const python=join(root,'.pablo/a2a-fixture-venv/bin/python');
const base=(await readFile(new URL('../docs/project/fixtures/c3-model-routes/three-providers.toml',import.meta.url),'utf8')).replace('max_model_calls=2','max_model_calls=20').replace('max_tool_calls=1','max_tool_calls=10');
const wire=(delta:object,finish='stop')=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\ndata: [DONE]\n\n`;
test('R02 configured CLI and ACP model delegation shares native ownership and keeps remote usage separate',{timeout:20000,skip:!existsSync(python)},async()=>{
    const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-a2a-supervised-')));
    const peer=spawn(python,[join(root,'tests/fixtures/a2a/wire_server.py')],{cwd,env:{},stdio:['ignore','pipe','pipe']});
    const exited=new Promise<void>((resolve,reject)=>{peer.once('error',reject);peer.once('close',()=>resolve());});
    let stderr='';peer.stderr.on('data',chunk=>{stderr+=chunk.toString();});
    const ready=new Promise<string>((resolve,reject)=>{
        let text='';const timer=setTimeout(()=>reject(new Error('SDK peer readiness timeout')),5000);
        peer.stdout.on('data',chunk=>{text+=chunk.toString();if(text.length>4096){clearTimeout(timer);reject(new Error('SDK peer readiness bound'));}else if(text.includes('\n')){clearTimeout(timer);resolve(JSON.parse(text.split('\n')[0]).url);}});
        exited.then(()=>{clearTimeout(timer);reject(new Error(`SDK peer exited: ${stderr}`));},reject);
    });
    let gateway:Awaited<ReturnType<typeof server>>|undefined;
    try {
        const rpc=await ready;
        gateway=await server(async(req,res)=>{
            const request=JSON.parse((await body(req)).toString());
            assert.equal(request.model,'z-ai/glm-5.3-flash');
            const results=request.messages.filter((m:any)=>m.role==='tool').map((m:any)=>JSON.parse(m.content));
            const call=(args:object)=>wire({tool_calls:[{index:0,id:`call-${results.length}`,type:'function',function:{name:'subagent',arguments:JSON.stringify(args)}}]},'tool_calls');
            res.writeHead(200,{'content-type':'text/event-stream'});
            if(results.length===0){assert.deepEqual(request.tools.find((tool:any)=>tool.function.name==='subagent').function.parameters.properties.remote_request.properties.remote.enum,['peer']);res.end(call({action:'spawn_remote',remote_request:{remote:'peer',parts:[{text:'assembly'}],accepted_output_modes:['text/plain','application/octet-stream'],stream:true}}));}
            else if(results.length===1){assert.equal(results[0].subagent.agent.kind,'remote_a2a');assert.equal(results[0].subagent.agent.state,'queued');res.end(call({action:'wait',agent_ids:[results[0].subagent.agent.agent_id],mode:'all',timeout_ms:5000}));}
            else {
                assert.equal(results.length,2);assert.deepEqual(results[1].subagent.remaining,[]);
                const child=results[1].subagent.settled[0];assert.equal(child.outcome.status,'completed');assert.equal(child.accounting.model_calls,'0');assert.equal(child.accounting.usage.input_tokens,'0');assert.equal(child.accounting.usage.output_tokens,'0');
                assert.equal(child.remote.remote.task_id,'remote-task');assert.equal(child.remote.result.artifacts[0].parts[0].text,'first');assert.equal(child.remote.result.artifacts[0].parts[1].filename,'../../inert.bin');
                assert.equal(child.remote.result.remote_reported_usage.totalTokens,'20');assert.equal(child.remote.remote_reported_usage.costMicrousd,'42');
                res.end(wire({content:'remote result verified'}));
            }
        });
        const entry=join(cwd,'entry.toml');
        await writeFile(entry,base+'\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=false\npath={base="workspace",path="trace-{session_id}.jsonl"}\n[options.a2a.remotes.peer]\ncard_url="https://agent.example.test/.well-known/agent-card.json"\nendpoint="https://agent.example.test/rpc"\n');
        const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--fixture-a2a-endpoint',`peer=${rpc}`];
        const verify=async(session:string)=>{
            const text=await readFile(join(cwd,`trace-${session}.jsonl`),'utf8');assert(!text.includes('inert.bin'));assert(!text.includes('root private task'));
            const events=text.trim().split('\n').map(line=>JSON.parse(line));assert.deepEqual(events.map(e=>e.root_seq),events.map((_,i)=>i+1));
            const children=events.filter(e=>e.agent.depth===1);assert(children.length>=6);assert(children.every(e=>e.agent.kind==='remote_a2a'));assert.deepEqual(children.map(e=>e.seq),children.map((_,i)=>i+1));
            assert.equal(children[0].type,'run.started');assert.equal(children.at(-1).type,'run.finished');assert.equal(children.at(-1).accounting.model_calls,'0');
            assert.equal(children[0].trace_id,events[0].trace_id);assert(events.some(e=>e.type==='tool.started'&&e.span_id===children[0].parent_span_id));
            const updates=children.filter(e=>e.type==='a2a.update');assert(updates.some(e=>e.remote.remote_reported_usage?.totalTokens==='20'));assert(updates.every(e=>e.remote.context_id==='remote-context'&&e.remote.task_id==='remote-task'));
            assert.equal(events.at(-1).agent.depth,0);return children[0].agent.agent_id;
        };
        const cli=JSON.parse((await exec(binary,['run','root private task',...args,'--json'],{env:cleanEnv(),timeout:8000})).stdout);assert.equal(cli.outcome.output,'remote result verified');assert.equal(cli.accounting.model_calls,'3');assert.equal(cli.accounting.tool_calls,'2');await verify(cli.session_id);
        const updates:any[]=[];
        await withPablo({binary,args,env:cleanEnv(),onUpdate:update=>{updates.push(update);}},async cx=>{
            await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}}});const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
            const result=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'root private task'}]}));assert.equal(result.outcome.status,'completed');assert.equal(result.accounting.model_calls,'3');const agent=await verify(sessionId);
            const remote=updates.filter(u=>u._meta?.['pablo/v2']?.agent?.agent_id===agent);assert(remote.some(u=>u.update.sessionUpdate==='tool_call'));assert(remote.some(u=>u.update.rawOutput?.remote_a2a?.remote_reported_usage?.totalTokens==='20'));assert(remote.some(u=>u.update.status==='completed'));assert(remote.every(u=>u.sessionId===sessionId));
        });
    } finally {
        try {if(gateway)await gateway.close();} finally {if(peer.exitCode===null)peer.kill('SIGTERM');const timer=setTimeout(()=>peer.kill('SIGKILL'),3000);try {await exited;} finally {clearTimeout(timer);await rm(cwd,{recursive:true,force:true});}}
    }
});
