/** Explicit paid C3.7 CLI/ACP acceptance. Never imported by ordinary tests. */
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { execFile, spawn } from 'node:child_process';
import { once } from 'node:events';
import { promisify } from 'node:util';
import { mkdir, readFile, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { withPablo, taskOf } from '../examples/acp-client.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import { sourceFingerprint } from './lib/source-fingerprint.mjs';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const args=process.argv.slice(2);
if(args.length>2){console.error('Usage: node scripts/smoke-live-openrouter.ts [BINARY] [CREDENTIAL_FILE]');process.exit(2);}
const binary=resolve(root,args[0]??'target/release/pablo');
const credentialFile=resolve(root,args[1]??'.env');
const explicitFile=args[1]!==undefined;
const model='z-ai/glm-5.3-flash';
const runRoot=join(root,'.pablo','live',`c3.7-openrouter-${randomUUID()}`);
const env={...process.env};
// The harness never opens a credential file, reads its values or prints child errors.
// Only the executable resolves the selected reference; no fixture can replace live delivery.
for(const key of Object.keys(env))if(key.startsWith('PABLO_')||key.startsWith('OTEL_')||['AI_GATEWAY_API_KEY','VERCEL_AI_GATEWAY'].includes(key))delete env[key];
const results:any[]=[];let stage='setup';let surface='cli';let workspace:string|undefined;
const safeCodes=new Set(['config_credential_missing','config_credential_invalid','config_credential_scope','config_invalid_value','provider_rejected','provider_transport','malformed_stream']);
function closedCode(raw:string):string|undefined{return [...safeCodes].find(code=>new RegExp(`\\b${code}\\b`).test(raw));}
function watchdog(child:import('node:child_process').ChildProcess){
 const term=setTimeout(()=>child.kill('SIGTERM'),100000);const kill=setTimeout(()=>child.kill('SIGKILL'),110000);
 const clear=()=>{clearTimeout(term);clearTimeout(kill);};child.once('exit',clear);child.once('error',clear);
}
try{
 await mkdir(runRoot,{recursive:true,mode:0o700});
 const binaryHash=createHash('sha256').update(await readFile(binary)).digest('hex');
 const sourceHash=await sourceFingerprint(root);
 const rust=(await promisify(execFile)('rustc',['--version'])).stdout.trim();
 for(surface of ['cli','acp']){
  workspace=join(runRoot,surface);await mkdir(workspace,{mode:0o700});workspace=await realpath(workspace);
  const marker=`pablo-live-evidence-${randomUUID()}`;await writeFile(join(workspace,'evidence.txt'),marker,{mode:0o600});
  const trace=join(workspace,'trace.jsonl');const entry=join(workspace,'deployment.toml');
  // Credential paths are host bindings outside the model-visible workspace. No secrets are interpolated.
  const config=`schema_version=1
[credentials.gateway]
consumer="provider.openrouter"
sources=[${explicitFile?'':'{kind="environment",name="OPENROUTER_API_KEY"},'}{kind="file",path={base="binding",name="credentials",path=${JSON.stringify(basename(credentialFile))}},encoding="dotenv",key="OPENROUTER_API_KEY"}]
[options.model]
provider="openrouter"
id="${model}"
[options.shell]
enabled=false
[options.policy.tools]
default="deny"
allow=[{id="live.read",value="fs.read"}]
[options.limits]
max_run_duration_ms=90000
max_model_calls=2
max_tool_calls=1
max_output_tokens=4096
[options.trace]
path={base="workspace",path="trace.jsonl"}
`;
  await writeFile(entry,config,{mode:0o600});
  const options=['--config',entry,'--bind',`workspace=${workspace}`,'--bind',`credentials=${dirname(credentialFile)}`];
  const taskText='Use exactly one fs.read call to read evidence.txt, then return its exact contents and nothing else. Do not read other files.';
  let raw='';let diagnostics='';let task:any;let streamed='';const toolResults:any[]=[];let exitCode:number|null=null;let exitSignal:NodeJS.Signals|null=null;
  const started=performance.now();stage='live '+surface+' request';
  if(surface==='cli'){
   const child=spawn(binary,['run',taskText,...options,'--json'],{env,stdio:['ignore','pipe','pipe']});watchdog(child);
   child.stdout.on('data',b=>{if(raw.length+b.length>8*1024*1024)child.kill('SIGTERM');else raw+=b.toString();});
   child.stderr.on('data',b=>{diagnostics=(diagnostics+b.toString()).slice(-65536);});
   [exitCode,exitSignal]=await once(child,'exit') as [number|null,NodeJS.Signals|null];
   if(exitCode!==0)throw {safe_code:closedCode(diagnostics+raw)};
   task=JSON.parse(raw);
  }else{
   await withPablo({binary,args:options,env,onSpawn:child=>{
    watchdog(child);child.once('exit',(code,signal)=>{exitCode=code;exitSignal=signal;});
    child.stdout.on('data',b=>{if(raw.length+b.length>8*1024*1024)child.kill('SIGTERM');else raw+=b.toString();});
   },onDiagnostic:text=>{diagnostics=(diagnostics+text).slice(-65536);},onUpdate:({update})=>{
    if(update.sessionUpdate==='agent_message_chunk'&&update.content.type==='text')streamed+=update.content.text;
    if(update.sessionUpdate==='tool_call_update'&&update.status==='completed')toolResults.push(update.rawOutput);
   }},async cx=>{
    const init=await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v2':true,'pablo/task-v2':true}},clientInfo:{name:'pablo-live-acceptance',version:'c3.7'}});
    assert.equal(init.agentCapabilities?._meta?.['pablo/task-v2'],true);
    const {sessionId}=await cx.request('session/new',{cwd:workspace!,mcpServers:[]});
    const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:taskText}]});
    assert.equal(response.stopReason,'end_turn');task=taskOf(response);
   });
  }
  stage=surface+' evidence validation';assert.equal(exitCode,0);assert.equal(exitSignal,null);
  stage=surface+' exact output';assert.equal(task.outcome.status,'completed');assert.equal(task.outcome.output,marker);
  if(surface==='acp'){assert.equal(streamed,marker);assert.equal(toolResults.length,1);assert.equal(toolResults[0].filesystem.text,marker);}
  stage=surface+' native lifecycle';const nativeRaw=await readFile(trace,'utf8');const records=nativeRaw.trim().split('\n').map(s=>JSON.parse(s));
  const starts=records.filter(e=>e.type==='model.started');const finishes=records.filter(e=>e.type==='model.finished');
  const tools=records.filter(e=>e.type==='tool.started');const ended=records.filter(e=>e.type==='tool.finished');const terminals=records.filter(e=>e.type==='run.finished');
  assert.equal(starts.length,2);assert.equal(finishes.length,2);assert.equal(tools.length,1);assert.equal(ended.length,1);assert.equal(terminals.length,1);
  assert(starts.every(e=>e.provider==='openrouter'&&e.model===model));assert.equal(tools[0].call.name,'fs.read');
  assert(starts[0].seq<tools[0].seq&&tools[0].seq<ended[0].seq&&ended[0].seq<starts[1].seq);
  assert.equal(ended[0].result.status,'completed');assert(!records.some(e=>e.type==='shell.started'));
  stage=surface+' native accounting';assert.equal(task.accounting.model_calls,'2');assert.equal(task.accounting.tool_calls,'1');
  assert.deepEqual(terminals[0].accounting,task.accounting);
  assert.deepEqual(terminals[0].outcome,{...task.outcome,output:null,output_bytes:Buffer.byteLength(marker)});
  assert.equal(terminals[0].content_redacted,true);
  for(const field of ['input_tokens','output_tokens','cache_read_input_tokens','cache_write_input_tokens']){
   const counts=finishes.map(e=>e.usage[field]);assert(counts.every(v=>v===null||(Number.isSafeInteger(v)&&v>=0)));
   assert.equal(task.accounting.usage[field],counts.some(v=>v===null)?null:counts.reduce((n,v)=>n+BigInt(v),0n).toString());
  }
  assert(BigInt(task.accounting.usage.input_tokens)>0n);assert(BigInt(task.accounting.usage.output_tokens)>0n);
  stage=surface+' privacy and deployment identity';assert(!nativeRaw.includes(marker));assert(!diagnostics.includes(marker));assert.equal((await stat(trace)).mode&0o777,0o600);
  // Metadata-only native trace intentionally omits file contents. The exact unseen
  // marker in the answer plus the observed allowed read supplies the live proof.
  const fingerprint=records.find(e=>e.deployment)?.deployment?.fingerprint;assert.match(fingerprint,/^sha256:[a-f0-9]{64}$/);
  assert(!/(?:Bearer |sk-or-)/.test(raw+diagnostics+nativeRaw));
  const summary={surface,result:'passed',elapsed_ms:performance.now()-started,provider:'openrouter',model,binary:relative(root,binary),binary_sha256:binaryHash,source_sha256:sourceHash,config_fingerprint:fingerprint,platform:`${process.platform}/${process.arch}`,node:process.version,rust,accounting:task.accounting,per_call_usage:finishes.map(e=>e.usage),exact_evidence:true,model_tool_model:true,one_terminal:true,no_shell:true,credential_source:explicitFile?'explicit dotenv reference; executable-only resolution':'OPENROUTER_API_KEY environment then root dotenv reference; executable-only resolution',trace_redacted:true,workspace_removed:false};
  await writeFile(join(runRoot,surface+'-trace.jsonl'),nativeRaw,{flag:'wx',mode:0o600});
  await rm(workspace,{recursive:true,force:true});workspace=undefined;summary.workspace_removed=true;
  results.push(summary);await writeFile(join(runRoot,'summary.json'),JSON.stringify({checkpoint:'C3.7',timestamp:new Date().toISOString(),results},null,2)+'\n',{mode:0o600});
  console.log(JSON.stringify(summary));
 }
}catch(error){
 const e=error as any;const native=e?.data?.['pablo/v2']?.task?.outcome;const code=safeCodes.has(e?.safe_code)?e.safe_code:safeCodes.has(native?.code)?native.code:undefined;
 const summary={checkpoint:'C3.7',result:'failed',surface,stage,code,completed_surfaces:results.map(r=>r.surface)};
 await mkdir(runRoot,{recursive:true,mode:0o700});await writeFile(join(runRoot,'failure.json'),JSON.stringify(summary,null,2)+'\n',{mode:0o600});
 console.error(JSON.stringify(summary));process.exitCode=1;
}finally{if(workspace)await rm(workspace,{recursive:true,force:true});}
