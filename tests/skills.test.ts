import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,mkdir,writeFile,readFile,realpath,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {cleanEnv,body,server} from './fixtures/telemetry.ts';
import {withPablo,outcomeOf} from '../examples/acp-client.ts';
const exec=promisify(execFile);const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));

test('S01 configured inspection lists portable metadata without credentials, activation or ambient roots',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skills-host-')));
  try{
    await mkdir(join(cwd,'.agents/skills/read'),{recursive:true});
    const file=join(cwd,'.agents/skills/read/SKILL.md');await writeFile(file,'---\nname: read\ndescription: Read synthetic evidence.\nmetadata:\n  version: "1"\n---\nPRIVATE_INSTRUCTIONS\n');
    const entry=join(cwd,'entry.toml');const config='schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="MISSING_KEY"}]\n';
    await writeFile(entry,config);const flags=['--config',entry,'--bind',`workspace=${cwd}`];const env=cleanEnv();
    const empty=JSON.parse((await exec(binary,['skills','list',...flags],{cwd,env})).stdout);assert.deepEqual(empty.entries,[]);assert.deepEqual(empty.roots,[]);
    await writeFile(entry,config+'[options.skills.roots]\nworkspace={base="workspace",path=".agents/skills"}\n');
    const first=(await exec(binary,['skills','list',...flags],{cwd,env})).stdout;const catalog=JSON.parse(first);
    assert.equal(catalog.entries.length,1);assert.equal(catalog.entries[0].qualified_name,'workspace/read');assert.equal(catalog.roots[0].path,join(cwd,'.agents/skills'));assert(!first.includes('PRIVATE_INSTRUCTIONS'));
    await writeFile(file,(await readFile(file,'utf8')).replace('PRIVATE_INSTRUCTIONS','OTHER_BODY'));
    assert.equal((await exec(binary,['skills','list',...flags],{cwd,env})).stdout,first,'body changes cannot affect a metadata-only catalog');
    const shown=JSON.parse((await exec(binary,['skills','show','read',...flags],{cwd,env})).stdout);assert.deepEqual(shown,catalog.entries[0]);
    await assert.rejects(exec(binary,['skills','show','missing',...flags],{cwd,env}));
    await writeFile(entry,config+'[options.skills.roots]\nworkspace={base="source",path=".agents/skills"}\n[[authority]]\nid="host"\nskill_roots=[{base="workspace",path=".agents/skills"}]\n');
    const rendered=(await exec(binary,['config','render',...flags],{cwd,env})).stdout;assert(rendered.includes('skill_roots'));assert(!rendered.includes('PRIVATE_INSTRUCTIONS'));
    assert.equal(JSON.parse((await exec(binary,['skills','list',...flags],{cwd,env})).stdout).entries.length,1);
    await writeFile(entry,(await readFile(entry,'utf8')).replace('skill_roots=[{base="workspace",path=".agents/skills"}]','skill_roots=[]'));
    await assert.rejects(exec(binary,['skills','list',...flags],{cwd,env}),(e:any)=>e.stderr.includes('config_authority_violation')&&e.stdout==='');
    await writeFile(entry,config+'[options.skills.roots]\nworkspace={base="workspace",path="../escape"}\n');
    await assert.rejects(exec(binary,['config','validate',...flags],{cwd,env}));
  }finally{await rm(cwd,{recursive:true,force:true});}
});

test('S02 CLI and ACP activate explicitly and consume an ordinary selected resource with private telemetry',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skills-activate-host-')));let calls=0;let firstInstructions:string|undefined;
  const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
  const gateway=await server(async(req,res)=>{
    const value=JSON.parse((await body(req)).toString());calls++;
    const system=value.messages.find((m:any)=>m.role==='system')?.content??'';
    if(firstInstructions===undefined)firstInstructions=system;else assert.equal(system,firstInstructions);
    assert(!system.includes('PRIVATE_SKILL_BODY'));assert(value.messages.some((m:any)=>m.content?.includes('PRIVATE_SKILL_BODY')));
    assert.equal(value.tools.length,1);assert.equal(value.tools[0].function.name,'skill_read');
    res.writeHead(200,{'content-type':'text/event-stream'});
    if(value.messages.at(-1).role==='tool'){
      const result=JSON.parse(value.messages.at(-1).content);assert.equal(result.skill.resource.text,'ACTUAL_SELECTED_EVIDENCE');assert.equal(result.skill.resource.sha256.length,64);
      res.end(frame({content:'Consumed selected evidence.'})+frame({},'stop')+'data: [DONE]\n\n');
    }else{
      // The resource does not exist until the model explicitly requests it.
      await writeFile(join(cwd,'skills/read/evidence.txt'),'ACTUAL_SELECTED_EVIDENCE');
      res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:'skill_read',arguments:JSON.stringify({skill:'host/read',path:'evidence.txt'})}}]},'tool_calls')+'data: [DONE]\n\n');
    }
  });
  try{
    await mkdir(join(cwd,'skills/read'),{recursive:true});await writeFile(join(cwd,'skills/read/SKILL.md'),'---\nname: read\ndescription: Read evidence when requested.\n---\nPRIVATE_SKILL_BODY: use evidence.txt only when requested.\n');
    const entry=join(cwd,'entry.toml');const config='schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.shell]\nenabled=false\n[options.filesystem]\nenabled=false\n[options.trace]\npath={base="workspace",path="{session_id}.jsonl"}\n[options.skills.roots]\nhost={base="workspace",path="skills"}\n';await writeFile(entry,config);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--skill','read'];const env=cleanEnv();
    const cli=await exec(binary,['run','Read selected evidence',...args],{cwd,env,timeout:10000});assert.equal(cli.stdout.trim(),'Consumed selected evidence.');assert(cli.stderr.includes('activated Skill host/read'));assert(!cli.stderr.includes('PRIVATE_SKILL_BODY'));
    await rm(join(cwd,'skills/read/evidence.txt'));
    await writeFile(entry,config.replace('[options.skills.roots]','[options.skills]\nactivate=["host/read"]\n[options.skills.roots]'));
    const activations:any[]=[];
    await withPablo({binary,args:args.slice(0,-2),env,onSkill:n=>{activations.push(n);}},async cx=>{
      const init=await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/skills-v1':true}}});assert.equal(init.agentCapabilities?._meta?.['pablo/skills-v1'],true);
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});assert.equal(outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read selected evidence'}]})).status,'completed');
      assert.equal(activations.length,1);assert.equal(activations[0].skill.qualified_name,'host/read');assert.equal(activations[0].instructions,null);
      const trace=await readFile(join(cwd,`${sessionId}.jsonl`),'utf8');assert(trace.includes('skill.activated'));assert(!trace.includes('PRIVATE_SKILL_BODY'));assert(!trace.includes('ACTUAL_SELECTED_EVIDENCE'));
    });assert.equal(calls,4);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('S02 bundled scripts execute only through admitted shell lifecycle',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skill-script-')));let calls=0;
  const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
  const gateway=await server(async(req,res)=>{
    const value=JSON.parse((await body(req)).toString());calls++;res.writeHead(200,{'content-type':'text/event-stream'});
    if(value.messages.at(-1).role==='tool') {assert.equal(JSON.parse(value.messages.at(-1).content).shell.stdout,'BUNDLED_SCRIPT_OUTPUT');res.end(frame({content:'Used script output.'})+frame({},'stop')+'data: [DONE]\n\n');}
    else res.end(frame({tool_calls:[{index:0,id:'script',type:'function',function:{name:'shell_run',arguments:JSON.stringify({command:'/bin/sh skills/script/run.sh',cwd:'.'})}}]},'tool_calls')+'data: [DONE]\n\n');
  });
  try{
    await mkdir(join(cwd,'skills/script'),{recursive:true});await writeFile(join(cwd,'skills/script/SKILL.md'),'---\nname: script\ndescription: Use a bundled script.\nallowed-tools: shell.run\n---\nRun run.sh through the permitted shell capability.\n');await writeFile(join(cwd,'skills/script/run.sh'),"printf BUNDLED_SCRIPT_OUTPUT\n/usr/bin/touch script-ran\n");
    const entry=join(cwd,'entry.toml');const config='schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.filesystem]\nenabled=false\n[options.skills]\nactivate=["script"]\n[options.skills.roots]\nhost={base="workspace",path="skills"}\n';
    const args=['run','Use the selected script','--json','--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];const env=cleanEnv();
    await writeFile(entry,config);const result=JSON.parse((await exec(binary,args,{cwd,env,timeout:10000})).stdout);assert.equal(result.outcome.output,'Used script output.');assert.equal(await readFile(join(cwd,'script-ran'),'utf8'),'');await rm(join(cwd,'script-ran'));
    await writeFile(entry,config+'[options.policy.tools]\ndefault="allow"\ndeny=[{id="host.no_script",value="shell.run"}]\n');
    let denied;try{denied=await exec(binary,args,{cwd,env,timeout:10000});}catch(e:any){denied=e;}
    assert.equal(JSON.parse(denied.stdout).outcome.status,'policy_denied');await assert.rejects(readFile(join(cwd,'script-ran')));assert.equal(calls,3);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});


test('S02 ACP reloads instructions per session and suppresses unnegotiated activation',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skills-fresh-')));let expected='FIRST_PRIVATE_BODY';let calls=0;const activations:unknown[]=[];
  const gateway=await server(async(req,res)=>{
    const value=JSON.parse((await body(req)).toString());calls++;
    assert(value.messages.some((m:any)=>m.content?.includes(expected)));
    assert(!value.messages.some((m:any)=>m.content?.includes(expected==='FIRST_PRIVATE_BODY'?'SECOND_PRIVATE_BODY':'FIRST_PRIVATE_BODY')));
    res.writeHead(200,{'content-type':'text/event-stream'});
    res.end('data: '+JSON.stringify({choices:[{index:0,delta:{content:'fresh instructions'},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
  });
  try{
    await mkdir(join(cwd,'skills/read'),{recursive:true});const skill=join(cwd,'skills/read/SKILL.md');const bodyPrefix='---\nname: read\ndescription: Fresh instructions.\n---\n';await writeFile(skill,bodyPrefix+expected);
    const entry=join(cwd,'entry.toml');const config='schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.shell]\nenabled=false\n[options.filesystem]\nenabled=false\n[options.skills]\nactivate=["host/read"]\n[options.skills.roots]\nhost={base="workspace",path="skills"}\n';await writeFile(entry,config);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];const env=cleanEnv();
    await withPablo({binary,args,env,onSkill:n=>{activations.push(n);}},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      for(let i=0;i<2;i++){
        if(i===1){expected='SECOND_PRIVATE_BODY';await writeFile(skill,bodyPrefix+expected);}
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});assert.equal(outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Use current instructions'}]})).status,'completed');
      }
    });assert.equal(calls,2);assert.deepEqual(activations,[]);
    await writeFile(entry,config.replace('[credentials.gateway]','[deployment]\nlocked=true\nallowed_run_overrides=["input"]\n[credentials.gateway]'));
    await assert.rejects(exec(binary,['run','Denied override',...args,'--skill','read'],{cwd,env}),(e:any)=>e.stderr.includes('config_override_forbidden'));
    assert.equal(calls,2,'locked activation override rejects before provider delivery');
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
