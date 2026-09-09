/** Offline local overhead comparison. No live model, tool command, or coding-quality claim. */
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
import { promisify } from 'node:util';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, writeFile, readFile, realpath, stat, rm } from 'node:fs/promises';
import { tmpdir, cpus, release } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
const root=resolve(import.meta.dirname,'..');
const count=Number(process.argv[2]??30);assert(Number.isInteger(count)&&count>=1&&count<=100);
const outputPath=resolve(process.argv[3]??join(root,'.pablo/measurements/codex-comparison.json'));
const binaries={pablo:join(root,'target/release/pablo'),codex:process.env.PABLO_COMPARE_CODEX??'/opt/homebrew/bin/codex'};
const exec=promisify(execFile);const base=await realpath(await mkdtemp(join(tmpdir(),'pablo-codex-compare-')));
const cwd=join(base,'workspace');const codexHome=join(base,'codex-home');await mkdir(cwd);await mkdir(codexHome);
const env={PATH:process.env.PATH,TMPDIR:process.env.TMPDIR,LANG:'en_US.UTF-8',CODEX_HOME:codexHome};
let requests=[];let sequence=0;const failures=[];
const server=createServer(async(req,res)=>{try{
 let raw='';for await(const chunk of req){raw+=chunk;assert(raw.length<1024*1024);}const body=JSON.parse(raw);
 requests.push({at:performance.now(),bytes:Buffer.byteLength(raw),path:req.url});
 assert.equal(req.method,'POST');assert.equal(req.headers.authorization,req.url.startsWith('/codex/')?undefined:'Bearer pablo-local-fixture');
 res.writeHead(200,{'content-type':'text/event-stream'});
 if(req.url==='/codex/responses'){
  const item={id:`msg_${sequence}`,type:'message',status:'completed',role:'assistant',content:[{type:'output_text',text:'benchmark',annotations:[]}]};
  const response={id:`resp_${sequence++}`,object:'response',created_at:1,status:'completed',output:[item],usage:{input_tokens:1,output_tokens:1,total_tokens:2}};
  const events=[{type:'response.created',response:{...response,status:'in_progress',output:[]}},{type:'response.output_item.added',output_index:0,item:{...item,status:'in_progress',content:[]}},{type:'response.content_part.added',item_id:item.id,output_index:0,content_index:0,part:{type:'output_text',text:'',annotations:[]}},{type:'response.output_text.delta',item_id:item.id,output_index:0,content_index:0,delta:'benchmark'},{type:'response.output_item.done',output_index:0,item},{type:'response.completed',response}];
  res.end(events.map((e,i)=>`event: ${e.type}\ndata: ${JSON.stringify({...e,sequence_number:i})}\n\n`).join(''));
 }else{
  assert.equal(req.url,'/pablo/chat/completions');assert.equal(body.stream,true);
  res.end('data: '+JSON.stringify({choices:[{index:0,delta:{content:'benchmark'},finish_reason:null}]})+'\n\ndata: '+JSON.stringify({choices:[{index:0,delta:{},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
 }
}catch(e){failures.push(e.message);res.destroy();}});
server.listen(0,'127.0.0.1');await once(server,'listening');const endpoint=`http://127.0.0.1:${server.address().port}`;
await writeFile(join(codexHome,'config.toml'),`model = "gpt-5.4"\nmodel_provider = "fixture"\napproval_policy = "never"\nsandbox_mode = "read-only"\n[analytics]\nenabled = false\n[model_providers.fixture]\nname = "offline fixture"\nbase_url = "${endpoint}/codex"\nwire_api = "responses"\nrequires_openai_auth = false\nsupports_websockets = false\nrequest_max_retries = 0\nstream_max_retries = 0\n`);
const task='Return benchmark.';
const stats=values=>{const s=values.toSorted((a,b)=>a-b);return {n:s.length,min:s[0],p50:s[Math.ceil(s.length*.5)-1],p95:s[Math.ceil(s.length*.95)-1],max:s.at(-1)};};
const collected=Object.fromEntries(Object.keys(binaries).map(k=>[k,{}]));
function record(name,key,value,sample){if(sample>=5)(collected[name][key]??=[]).push(value);}
function launch(name,args){return spawn(binaries[name],args,{cwd,env:{...env,...(name==='pablo'?{PABLO_FIXTURE_ENDPOINT:endpoint+'/pablo/chat/completions'}:{})},stdio:['pipe','pipe','pipe']});}
async function processRun(name,args){const start=performance.now();const child=launch(name,args);const exited=once(child,'exit');let stdout='',stderr='';child.stdout.on('data',b=>stdout+=b);child.stderr.on('data',b=>stderr+=b);child.stdin.end();const timer=setTimeout(()=>child.kill('SIGKILL'),15000);try{const[code]=await exited;assert.equal(code,0,stderr);return {stdout,elapsed:performance.now()-start,start};}finally{clearTimeout(timer);}}
class Peer{
 constructor(name){this.name=name;this.started=performance.now();this.child=launch(name,name==='pablo'?['acp','--stdio']:['app-server']);this.exited=once(this.child,'exit');this.pending=new Map();this.id=0;this.notifications=[];this.stderr='';let text='';this.child.stderr.on('data',b=>this.stderr+=b);this.child.stdout.on('data',b=>{text+=b;while(text.includes('\n')){const end=text.indexOf('\n');const msg=JSON.parse(text.slice(0,end));text=text.slice(end+1);if(this.pending.has(msg.id)){const p=this.pending.get(msg.id);this.pending.delete(msg.id);msg.error?p.reject(new Error(JSON.stringify(msg.error))):p.resolve(msg.result);}else {const at=performance.now();this.notifications.push({msg,at});if(msg.method==='turn/completed')this.turnDone?.(at);}}});this.timer=setTimeout(()=>this.child.kill('SIGKILL'),30000);}
 send(method,params,id){this.child.stdin.write(JSON.stringify({jsonrpc:'2.0',method,params,...(id===undefined?{}:{id})})+'\n');}
 request(method,params){const id=++this.id;let timer;const p=new Promise((resolve,reject)=>{this.pending.set(id,{resolve,reject});timer=setTimeout(()=>reject(new Error(`timeout ${method}: ${this.stderr.slice(-500)}`)),10000);});this.send(method,params,id);return p.finally(()=>clearTimeout(timer));}
 async init(){if(this.name==='pablo')await this.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});else{await this.request('initialize',{clientInfo:{name:'pablo_comparison',version:'1'},capabilities:{experimentalApi:true}});this.send('initialized',{});}return performance.now()-this.started;}
 async task(){const before=performance.now();let id;if(this.name==='pablo')id=(await this.request('session/new',{cwd,mcpServers:[]})).sessionId;else id=(await this.request('thread/start',{cwd,model:'gpt-5.4',modelProvider:'fixture',sandbox:'read-only',approvalPolicy:'never',ephemeral:true})).thread.id;
  const start=performance.now();this.notifications=[];requests=[];let completedAt;const completion=new Promise(resolve=>{this.turnDone=resolve;});
  if(this.name==='pablo'){const r=await this.request('session/prompt',{sessionId:id,prompt:[{type:'text',text:task}]});assert.equal(r._meta['pablo/v1'].outcome.output,'benchmark');}
  else {let timer;try{await this.request('turn/start',{threadId:id,input:[{type:'text',text:task}]});completedAt=await Promise.race([completion,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('timeout waiting for turn/completed')),10000);})]);}finally{clearTimeout(timer);this.turnDone=undefined;}const done=this.notifications.find(n=>n.msg.method==='turn/completed').msg.params;assert.equal(done.turn.status,'completed',JSON.stringify(done.turn.error));assert(this.notifications.some(n=>n.msg.method==='item/completed'&&n.msg.params.item.text==='benchmark'));}
  const end=completedAt??performance.now();assert.equal(requests.length,1);return {turn_ms:end-start,new_task_ms:end-before,request_bytes:requests[0].bytes};
 }
 async rss(){await delay(50);return Number((await exec('ps',['-o','rss=','-p',String(this.child.pid)])).stdout.trim());}
 async close(){this.child.stdin.end();const timer=setTimeout(()=>this.child.kill('SIGTERM'),2000);try{const[code,signal]=await this.exited;assert.equal(code,0,`${this.name}: ${signal} ${this.stderr}`);}finally{clearTimeout(timer);clearTimeout(this.timer);}}
}
try{
 for(let sample=0;sample<count+5;sample++){
  for(const name of (sample%2?['codex','pablo']:['pablo','codex'])){
   const v=await processRun(name,['--version']);record(name,'version_process_ms',v.elapsed,sample);
   requests=[];const r=await processRun(name,name==='pablo'?['run',task,'--json']:['exec','--ephemeral','--skip-git-repo-check','--json',task]);assert.equal(requests.length,1);
   if(name==='pablo')assert.equal(JSON.parse(r.stdout).outcome.output,'benchmark');else assert(r.stdout.trim().split('\n').map(l=>JSON.parse(l)).some(e=>e.type==='item.completed'&&e.item.text==='benchmark'));
   record(name,'cli_provider_ready_ms',requests[0].at-r.start,sample);record(name,'cli_total_ms',r.elapsed,sample);record(name,'cli_request_bytes',requests[0].bytes,sample);
   const peer=new Peer(name);try{record(name,'server_initialize_ms',await peer.init(),sample);record(name,'server_idle_rss_kib',await peer.rss(),sample);}finally{await peer.close();}
  }
 }
 for(const name of ['pablo','codex']){const peer=new Peer(name);try{await peer.init();for(let sample=0;sample<count+5;sample++){const r=await peer.task();for(const[k,v]of Object.entries(r))record(name,'warm_'+k,v,sample);}}finally{await peer.close();}}
 const report={timestamp:new Date().toISOString(),platform:{os:process.platform,arch:process.arch,kernel:release(),cpu:cpus()[0].model,node:process.version},method:{samples:count,warmup:5,workload:'one loopback model request returning fixed text; no inference, tool call, network model latency or quality evaluation',protocols:{pablo:'Chat Completions SSE; CLI task JSON and ACP server',codex:'Responses SSE; exec JSONL and App Server'},configuration:'fresh temporary workspace, isolated Codex home with no auth/plugins/MCP/user config; Codex read-only, ephemeral threads; Pablo default capabilities; neither asked to execute a tool',timers:'CLI includes process lifetime; server idle RSS after initialize and 50 ms; warm turn excludes thread/session creation, new_task includes it; completion uses protocol notification receipt, without polling',scope:'root process RSS only; installed main executable bytes, not complete distribution or peak memory; warm Codex threads remain loaded until process exit'},stats:Object.fromEntries(Object.entries(collected).map(([name,metrics])=>[name,Object.fromEntries(Object.entries(metrics).map(([k,v])=>[k,stats(v)]))])),builds:{},samples:collected};
 for(const[name,path]of Object.entries(binaries)){const bytes=await readFile(path);report.builds[name]={version:(await processRun(name,['--version'])).stdout.trim(),bytes:(await stat(path)).size,sha256:createHash('sha256').update(bytes).digest('hex')};}
 assert.deepEqual(failures,[]);await mkdir(resolve(outputPath,'..'),{recursive:true});await writeFile(outputPath,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify({outputPath,builds:report.builds,stats:report.stats},null,2));
}finally{server.closeAllConnections();await new Promise(r=>server.close(r));await rm(base,{recursive:true,force:true});}
