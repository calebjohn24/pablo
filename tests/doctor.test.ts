import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,realpath,rm,writeFile,readdir} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {fileURLToPath} from 'node:url';
import {body,cleanEnv,server} from './fixtures/telemetry.ts';
const exec=promisify(execFile),root=fileURLToPath(new URL('../',import.meta.url)),binary=process.env.PABLO_DOCTOR_BINARY??join(root,'target/debug/pablo');
const secret='DOCTOR_PRIVATE_CREDENTIAL';
const base='schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="DOCTOR_KEY"}]\n';
async function invoke(cwd:string,args:string[]=[],env:NodeJS.ProcessEnv={}) {
 let stdout='',stderr='',code=0;
 try {const r=await exec(binary,['doctor','--json',...args],{cwd,env:{...cleanEnv(),...env},timeout:15000});stdout=r.stdout;stderr=r.stderr;}
 catch(error:any){assert.equal(typeof error.code,'number',String(error));code=error.code;stdout=error.stdout;stderr=error.stderr;}
 assert(!stdout.includes(secret)&&!stderr.includes(secret));
 const report=JSON.parse(stdout);assert.equal(report.exit_code,code);assert.equal(stderr,'');assert(report.duration_ms>=0);
 return report;
}
async function workspace(fn:(cwd:string)=>Promise<void>) {const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-doctor-')));try{await fn(cwd);}finally{await rm(cwd,{recursive:true,force:true});}}
test('D01 legacy doctor reports missing/present credentials without exposing values',async()=>workspace(async cwd=>{
 const missing=await invoke(cwd);assert.equal(missing.exit_code,3);assert.match(missing.findings[0].fix,/declared source/);
 await writeFile(join(cwd,'.env'),`AI_GATEWAY_API_KEY=${secret}\n`);
 const present=await invoke(cwd);assert.equal(present.exit_code,0);assert.equal(present.models[0].model,'zai/glm-5.3-flash');assert.match(present.models[0].credential,/present/);assert.equal(present.probe_result,'not probed');assert.equal(present.protocols.acp.rust_sdk.version,'2.1.0');assert.equal(present.protocols.mcp,'2025-11-25');assert.equal(present.protocols.a2a,'1.0');assert.equal(present.protocols.open_responses.profile,'open-responses-text-tools-v1');const {readFile}=await import('node:fs/promises');assert.equal(present.protocols.open_responses.revision,JSON.parse(await readFile(join(root,'docs/project/fixtures/c3-open-responses/upstream-pin.json'),'utf8')).revision);assert.equal(present.settings.shell.enabled,true);
 const bad=await invoke(cwd,[],{AI_GATEWAY_API_KEY:secret+' invalid'});assert.equal(bad.exit_code,3);
}));
test('D01 configured doctor is offline, starts no MCP process and creates no trace',async()=>workspace(async cwd=>{
 let requests=0;const peer=await server(async(_req,res)=>{requests++;res.writeHead(500);res.end(secret);});
 try {
  const entry=join(cwd,'entry.toml');await writeFile(entry,base+`\n[options.mcp.servers.local]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","touch launched; exit 1"]\n[options.children]\nenabled=true\n[options.trace]\npath={base="workspace",path="trace-{session_id}.jsonl"}\n[options.otel]\nexporter="otlp"\nendpoint="${peer.url}/v1/traces"\n`);
  const report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',peer.url],{DOCTOR_KEY:secret});
  assert.equal(report.exit_code,0);assert.equal(requests,0);assert.deepEqual(await readdir(cwd),['entry.toml']);
  assert.equal(report.settings.mcp[0].connectivity,'not probed');assert.equal(report.settings.children.enabled,true);assert.equal(report.settings.otel.export,'not started');assert(report.configuration.sources.length);assert(Object.keys(report.configuration.provenance).length);
 } finally {await peer.close();}
}));
for(const [status,expected] of [[200,0],[401,4],[403,4],[404,5],[400,5],[500,8],[302,8]] as const) {
 test(`D01 explicit provider probe maps HTTP ${status} to safe diagnostic ${expected}`,async()=>workspace(async cwd=>{
  let requests=0;const peer=await server(async(req,res)=>{requests++;const request=JSON.parse((await body(req)).toString());assert.equal(request.model,'zai/glm-5.3-flash');assert.equal(request.max_tokens,16);assert(!request.tools?.length);res.writeHead(status,{'content-type':'text/event-stream',location:'http://127.0.0.1:1/forbidden'});res.end(secret);});
  try {const entry=join(cwd,'entry.toml');await writeFile(entry,base);const report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',peer.url,'--probe','provider'],{DOCTOR_KEY:secret});assert.equal(report.exit_code,expected);assert.equal(requests,1);if(expected)assert(report.findings[0].cause&&report.findings[0].fix);else assert.match(report.probe_result,/accepted/);}finally{await peer.close();}
 }));
}
test('D01 explicit MCP failure is diagnosed and static denial prevents launch',async()=>workspace(async cwd=>{
 const entry=join(cwd,'entry.toml'),args=['--config',entry,'--bind',`workspace=${cwd}`,'--probe','mcp'];
 const mcp='\n[options.mcp.servers.local]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","touch launched; exit 1"]\n';
 await writeFile(entry,base+mcp);let report=await invoke(cwd,args,{DOCTOR_KEY:secret});assert.equal(report.exit_code,6);assert((await readdir(cwd)).includes('launched'));await rm(join(cwd,'launched'));
 await writeFile(entry,base+mcp+'\n[options.policy.executables]\ndefault="deny"\n');report=await invoke(cwd,args,{DOCTOR_KEY:secret});assert.equal(report.exit_code,7);assert(!(await readdir(cwd)).includes('launched'));
}));

test('D01 MCP probe joins successful SDK startup without calling tools and diagnoses optional omissions',async()=>workspace(async cwd=>{
 const python=join(root,'.pablo/mcp-fixture-venv/bin/python'),fixture=join(root,'tests/fixtures/mcp/server.py'),entry=join(cwd,'entry.toml');
 const mcp=`\n[options.mcp.servers.local]\ntransport="stdio"\ncommand=${JSON.stringify(python)}\nargs=[${JSON.stringify(fixture)}]\n`;
 await writeFile(entry,base+mcp);
 let report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`,'--probe','mcp'],{DOCTOR_KEY:secret});
 assert.equal(report.exit_code,0);assert.match(report.probe_result,/closed/);
 const {readFile}=await import('node:fs/promises');const pid=Number(await readFile(join(cwd,'pid'),'utf8'));
 assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');assert(!(await readdir(cwd)).includes('context.json'));
 await writeFile(entry,base+'\n[options.mcp.servers.optional]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","exit 1"]\nrequired=false\n');
 report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`,'--probe','mcp'],{DOCTOR_KEY:secret});assert.equal(report.exit_code,6);
}));

test('D01 credential files and MCP command arguments stay private, while the plain report is readable',async()=>workspace(async cwd=>{
 await writeFile(join(cwd,'provider.key'),secret,{mode:0o600});const entry=join(cwd,'entry.toml');
 await writeFile(entry,`schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="file",path={base="config",path="provider.key"},encoding="utf8"}]\n[options.mcp.servers.local]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c",${JSON.stringify('echo '+secret)}]\n`);
 const report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`]);assert.equal(report.exit_code,0);assert.match(report.models[0].credential,/present/);
 const {stdout}=await exec(binary,['doctor','--config',entry,'--bind',`workspace=${cwd}`],{cwd,env:cleanEnv()});assert.match(stdout,/Mode: offline/);assert.match(stdout,/Model: vercel/);assert(!stdout.includes(secret));assert(!stdout.includes('\x1b'));assert(!(await readdir(cwd)).includes('trace.jsonl'));
}));

test('D01 provider probe uses the selected route entry without fallback',async()=>workspace(async cwd=>{
 const {readFile}=await import('node:fs/promises');const config=await readFile(join(root,'docs/project/fixtures/c3-model-routes/three-providers.toml'),'utf8');const entry=join(cwd,'entry.toml');await writeFile(entry,config);
 let requests=0;const peer=await server(async(req,res)=>{requests++;assert.equal(JSON.parse((await body(req)).toString()).model,'z-ai/glm-5.3-flash');res.writeHead(401);res.end(secret);});
 try{const report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',peer.url,'--probe','provider'],{ROUTE_ROUTER_KEY:secret,ROUTE_VERCEL_KEY:secret,ROUTE_RESPONSES_KEY:secret});assert.equal(report.exit_code,4);assert.equal(report.models.length,3);assert.equal(requests,1);}finally{await peer.close();}
}));

test('D01 explicit Open Responses probe uses its configured profile and no tools',async()=>workspace(async cwd=>{
 const entry=join(cwd,'entry.toml');await writeFile(entry,'schema_version=1\n[credentials.gateway]\nconsumer="provider.open_responses"\nsources=[{kind="environment",name="DOCTOR_KEY"}]\n[options.model]\nprovider="open_responses"\nid="fixture-text-tools-v1"\nendpoint="https://responses.example.test/v1/responses"\ncapability_profile="open-responses-text-tools-v1"\n');
 let requests=0;const peer=await server(async(req,res)=>{requests++;const request=JSON.parse((await body(req)).toString());assert.equal(request.model,'fixture-text-tools-v1');assert.equal(request.max_output_tokens,16);assert.equal(request.stream,true);assert(!request.tools?.length);res.writeHead(200,{'content-type':'text/event-stream'});res.end('');});
 try{const report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',peer.url,'--probe','provider'],{DOCTOR_KEY:secret});assert.equal(report.exit_code,0);assert.equal(requests,1);}finally{await peer.close();}
}));

test('D01 ordinary CLI failure hints point to doctor while preserving existing exit codes',async()=>workspace(async cwd=>{
 await assert.rejects(exec(binary,['run','synthetic task'],{cwd,env:cleanEnv()}),(error:any)=>error.code===2&&error.stderr.includes('Fix:')&&error.stderr.includes('pablo doctor')&&!error.stderr.includes(secret));
 const peer=await server(async(_req,res)=>{res.writeHead(401);res.end(secret);});
 try{await assert.rejects(exec(binary,['run','synthetic task','--no-shell','--no-filesystem'],{cwd,env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:peer.url}}),(error:any)=>error.code===1&&error.stderr.includes('Fix:')&&error.stderr.includes('doctor --probe provider')&&!error.stderr.includes(secret));}finally{await peer.close();}
}));

test('D01 scoped MCP, A2A and exporter credentials are diagnosed offline without values',async()=>workspace(async cwd=>{
 const entry=join(cwd,'entry.toml');await writeFile(entry,base+`\n[credentials.mcp_env]\nconsumer="mcp.env"\nsources=[{kind="environment",name="MCP_ENV_KEY"}]\n[credentials.mcp_header]\nconsumer="mcp.headers"\nsources=[{kind="environment",name="MCP_HEADER_KEY"}]\n[credentials.remote]\nconsumer="a2a.bearer"\nsources=[{kind="environment",name="REMOTE_KEY"}]\n[credentials.exporter]\nconsumer="otel.headers"\nsources=[{kind="environment",name="EXPORTER_KEY"}]\n[options.mcp.servers.local]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","touch must-not-launch"]\nenv={FIXTURE_TOKEN="mcp_env"}\n[options.mcp.servers.remote]\ntransport="http"\nurl="https://mcp.example.test/mcp"\nheaders={x-fixture="mcp_header"}\n[options.a2a.remotes.peer]\ncard_url="https://agent.example.test/.well-known/agent-card.json"\nendpoint="https://agent.example.test/rpc"\nbearer={scheme="bearer",credential="remote"}\n[options.otel]\nexporter="otlp"\nheaders="exporter"\n`);
 const report=await invoke(cwd,['--config',entry,'--bind',`workspace=${cwd}`],{DOCTOR_KEY:secret,MCP_ENV_KEY:secret,MCP_HEADER_KEY:secret,REMOTE_KEY:secret,EXPORTER_KEY:'x-fixture='+secret});
 assert.equal(report.exit_code,0);assert.equal(report.settings.mcp.length,2);assert(report.settings.mcp.every((s:any)=>s.credentials.startsWith('present')));assert.match(report.settings.a2a[0].credential,/present/);assert.match(report.settings.otel.credential,/present/);assert.deepEqual(await readdir(cwd),['entry.toml']);
}));

test('D01 Ctrl-C during MCP startup cancels and joins the owned process before exit',{timeout:15000},async()=>workspace(async cwd=>{
 const {spawn}=await import('node:child_process');const {readFile}=await import('node:fs/promises');const {setTimeout:delay}=await import('node:timers/promises');
 const entry=join(cwd,'entry.toml');await writeFile(entry,base+'\n[options.mcp.servers.local]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","echo $$ > owned.pid; exec sleep 30"]\n');
 const child=spawn(binary,['doctor','--json','--config',entry,'--bind',`workspace=${cwd}`,'--probe','mcp'],{cwd,env:{...cleanEnv(),DOCTOR_KEY:secret},stdio:['ignore','pipe','pipe']});
 let stdout='',stderr='',owned:number|undefined;child.stdout.on('data',v=>{stdout+=v;});child.stderr.on('data',v=>{stderr+=v;});
 const exited=new Promise<{code:number|null,signal:NodeJS.Signals|null}>((resolve,reject)=>{child.once('error',reject);child.once('close',(code,signal)=>resolve({code,signal}));});
 try{
  const deadline=performance.now()+5000;
  while(owned===undefined){assert(performance.now()<deadline,'MCP startup readiness deadline');assert.equal(child.exitCode,null);try{const candidate=Number((await readFile(join(cwd,'owned.pid'),'utf8')).trim());if(Number.isSafeInteger(candidate)&&candidate>1)owned=candidate;else await delay(10);}catch{await delay(10);}}
  process.kill(owned,0);child.kill('SIGINT');const result=await exited;assert.deepEqual(result,{code:130,signal:null});assert.equal(stderr,'');assert(!stdout.includes(secret));assert.equal(JSON.parse(stdout).exit_code,130);assert.throws(()=>process.kill(-owned!,0),(error:any)=>error.code==='ESRCH');owned=undefined;
 }finally{if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await exited;}if(owned!==undefined){try{process.kill(-owned,'SIGKILL');}catch(error:any){if(error.code!=='ESRCH')throw error;}}}
}));
