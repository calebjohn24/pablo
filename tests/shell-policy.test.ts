import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';
const exec=promisify(execFile);
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const rule=(id:string,args:string[],match='exact')=>`{id=${JSON.stringify(id)},executable="/usr/bin/printf",args=${JSON.stringify(args)},match="${match}"}`;
const base=`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="UNREAD_SYNTHETIC_KEY"}]
[options.interfaces]
cli_output="json"
[options.filesystem]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
`;

test('Q01 configured CLI and ACP enforce command modes, quoting, immutable ceilings and private decisions', {timeout:30000}, async()=>{
 const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-q01-hosts-')));
 let command='';let taskEnv:Record<string,string>|undefined;let requests=0;
 const gateway=await server(async(req,res)=>{
  assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');
  const request=JSON.parse((await body(req)).toString());requests++;
  res.writeHead(200,{'content-type':'text/event-stream'});
  const result=request.messages.at(-1);
  const chunk=result.role==='tool'?{content:JSON.parse(result.content).shell.stdout}:{tool_calls:[{index:0,id:'call',type:'function',function:{name:'shell_run',arguments:JSON.stringify({command,cwd:'.',...(taskEnv?{env:taskEnv}:{})})}}]};
  res.end('data: '+JSON.stringify({choices:[{index:0,delta:chunk,finish_reason:result.role==='tool'?'stop':'tool_calls'}]})+'\n\ndata: [DONE]\n\n');
 });
 try {
  const entry=join(cwd,'entry.toml');
  const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
  const allow=`[options.shell.commands]\ndefault="deny"\nallow=[${rule('allow.exact',['%s','private literal;$HOME'])}]\n`;
  const deny=`[options.shell.commands]\ndefault="allow"\ndeny=[${rule('deny.prefix',['private'],'prefix')}]\n`;
  const cases=[
   {config:allow,command:"printf %s 'private literal;$HOME'",output:'private literal;$HOME',id:'allow.exact'},
   {config:allow,command:"printf '%s private literal;$HOME'",id:'builtin.shell.commands.allowlist_miss'},
   {config:deny,command:'printf safe',output:'safe',id:'builtin.shell.commands.default_allow'},
   {config:deny,command:'printf private more',id:'deny.prefix'},
   {config:allow+`deny=[${rule('deny.overlap',['%s'],'prefix')}]\n`,command:"printf %s 'private literal;$HOME'",id:'deny.overlap'},
   {config:deny,command:'printf safe; touch marker',id:'builtin.shell.commands.syntax'},
   {config:deny,command:'printf $(touch marker)',id:'builtin.shell.commands.syntax'},
   {config:deny,command:'printf safe > marker',id:'builtin.shell.commands.syntax'},
   {config:deny+'[[authority]]\nid="root"\n[authority.shell.commands]\ndefault="deny"\n',command:'printf safe',id:'builtin.shell.commands.default_deny'},
   {config:deny+'[options.shell.environment]\nallowed_names=["PABLO_TASK_ALLOWED"]\n[[authority]]\nid="root"\n[authority.shell.environment]\nallowed_names=[]\n',command:'printf safe',env:{PABLO_TASK_ALLOWED:'private-env'},id:'builtin.shell.environment.names'},
   {config:deny+'[options.shell.cwd_roots]\ndefault="deny"\n',command:'printf safe',id:'builtin.shell.cwd_roots.default_deny'},
  ];
  for(const c of cases) {
   command=c.command;taskEnv=c.env;await writeFile(entry,base+c.config);const before=requests;
   let stdout:string;
   try {stdout=(await exec(binary,['run','private-task',...args],{env:cleanEnv(),timeout:5000})).stdout;assert(c.output!==undefined);}
   catch(e) {const error=e as {stdout:string,code:number};assert.equal(error.code,1);stdout=error.stdout;assert.equal(c.output,undefined);}
   const cli=JSON.parse(stdout).outcome;
   let acp;
   await withPablo({binary,args,env:cleanEnv()},async cx=>{
    await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true}}});
    const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
    acp=outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'private-task'}]}));
   });
   assert.deepEqual(acp,cli);
   if(c.output!==undefined) {assert.equal(cli.status,'completed');assert.equal(cli.output,c.output);assert.equal(requests-before,4);}
   else {assert.equal(cli.status,'policy_denied');assert.equal(cli.rule.configured.id,c.id);assert.equal(requests-before,2);}
   const traces=(await readdir(cwd)).filter(n=>n.endsWith('.jsonl'));assert.equal(traces.length,2);
   for(const trace of traces) {
    const raw=await readFile(join(cwd,trace),'utf8');const events=raw.trim().split('\n').map(s=>JSON.parse(s));
    assert.equal(events.filter(e=>e.type==='run.finished').length,1);
    assert.equal(events.filter(e=>e.type==='shell.started').length,c.output===undefined?0:1);
    assert(raw.includes(c.id));for(const text of ['private literal','private-task','private-env',cwd])assert(!raw.includes(text));
    await rm(join(cwd,trace));
   }
   assert(!(await readdir(cwd)).includes('marker'));
  }
 } finally {await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
