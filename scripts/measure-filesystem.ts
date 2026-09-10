/** C2 filesystem timings: fixed offline evidence, warm independent ACP sessions. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { withPablo, taskOf } from '../examples/acp-client.ts';
import { body, cleanEnv, server } from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import { sourceFingerprint } from './lib/source-fingerprint.mjs';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const binary=resolve(process.env.PABLO_MEASURE_BINARY ?? join(root,'target/release/pablo'));
const configured=process.env.PABLO_MEASURE_CONFIGURED==='1';
const destination=resolve(process.argv[2] ?? join(root,`.pablo/measurements/c2.5-filesystem-${process.platform}-${process.arch}.json`));
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-fs-measure-')));
const content='a'.repeat(128*1024-1)+'\n';const hash=(s:string)=>createHash('sha256').update(s).digest('hex');
const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
const finish=frame({},'stop')+'data: [DONE]\n\n';
const workloads=[
  {name:'read_128k',tool:'fs_read',args:{path:'read.txt',max_bytes:content.length},check:(r:any)=>assert.equal(r.text,content)},
  {name:'list_1000',tool:'fs_list',args:{path:'listing',max_entries:1000},check:(r:any)=>assert.equal(r.entries.length,1000)},
  {name:'search_1m',tool:'fs_search',args:{path:'search',query:'needle',max_matches:100},check:(r:any)=>assert.equal(r.matches.length,100)},
  {name:'replace_128k',tool:'fs_write',args:{path:'write.txt',text:content,expected_revision:hash(content)},check:(r:any)=>{assert.equal(r.revision,hash(content));assert.equal(r.committed,true);}},
  {name:'edit_128k',tool:'fs_edit',args:{path:'edit.txt',old_text:'needle',new_text:'needle',expected_revision:hash('needle'+content)},check:(r:any)=>{assert.equal(r.revision,hash('needle'+content));assert.equal(r.committed,true);}},
];
let current=workloads[0];let requests=0;
const gateway=await server(async(req,res)=>{
  const request=JSON.parse((await body(req)).toString());requests++;
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(request.messages.at(-1).role==='tool') {
    const result=JSON.parse(request.messages.at(-1).content);assert.equal(result.status,'completed');current.check(result.filesystem);
    res.end(frame({content:'verified'})+finish);
  } else res.end(frame({tool_calls:[{index:0,id:'fs_measure',type:'function',function:{name:current.tool,arguments:JSON.stringify(current.args)}}]},'tool_calls')+'data: [DONE]\n\n');
});
const stats=(values:number[])=>{const s=values.toSorted((a,b)=>a-b);return {n:s.length,p50:s[Math.ceil(s.length*.5)-1],p95:s[Math.ceil(s.length*.95)-1],p99:s.at(-1),min:s[0],max:s.at(-1)};};
try {
  const entry=join(cwd,'deployment.toml');
  if(configured)await writeFile(entry,`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="UNREAD_FIXTURE_KEY"}]
[options.shell]
enabled=false
[options.filesystem]
write=true
[options.model]
id="zai/glm-5.3-flash"
[options.limits]
max_model_calls=2
max_tool_calls=1
`);
  const options=configured?['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions']:['--no-shell','--allow-write','--model','zai/glm-5.3-flash','--max-model-calls','2','--max-tool-calls','1'];
  await writeFile(join(cwd,'read.txt'),content);await writeFile(join(cwd,'write.txt'),content);await writeFile(join(cwd,'edit.txt'),'needle'+content);
  await mkdir(join(cwd,'listing'));await mkdir(join(cwd,'search'));
  for(let i=0;i<1000;i++) await writeFile(join(cwd,'listing',String(i).padStart(4,'0')),'');
  for(let i=0;i<100;i++) await writeFile(join(cwd,'search',String(i).padStart(3,'0')), 'needle\n'+'x'.repeat(10479)+'\n');
  const measurements:Record<string,unknown>={};
  for(const workload of workloads) {
    current=workload;const promptMs:number[]=[];const toolMs:number[]=[];let start=0;let toolElapsed=0;
    await withPablo({binary,args:options,env:{...cleanEnv(),PABLO_FIXTURE_ENDPOINT:gateway.url+'/v1/chat/completions'},onUpdate:n=>{
      const meta=n._meta?.['pablo/v1'] as any;
      if(n.update.sessionUpdate==='tool_call') start=meta.timestamp_unix_micros;
      if(n.update.sessionUpdate==='tool_call_update' && n.update.status==='completed') toolElapsed=(meta.timestamp_unix_micros-start)/1000;
    }},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true,'pablo/task-v1':true}}});
      for(let sample=0;sample<35;sample++) {
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});requests=0;toolElapsed=0;
        const began=performance.now();const task=taskOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Measure fixed filesystem evidence.'}]}));const elapsed=performance.now()-began;
        assert.equal(task.outcome.status,'completed');assert.equal(task.accounting.model_calls,'2');assert.equal(task.accounting.tool_calls,'1');assert.equal(requests,2);assert(toolElapsed>=0);
        if(sample>=5){promptMs.push(elapsed);toolMs.push(toolElapsed);}
      }
    });
    measurements[workload.name]={prompt_ms:stats(promptMs),tool_ms:stats(toolMs),prompt_samples_ms:promptMs,tool_samples_ms:toolMs};
  }
  const report={checkpoint:process.env.PABLO_MEASURE_CHECKPOINT ?? 'C2.5',timestamp:new Date().toISOString(),platform:`${process.platform}/${process.arch}`,source_sha256:process.env.PABLO_MEASURE_SOURCE_SHA256 ?? await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),method:{configuration:configured?'explicit deployment file; re-resolved per task':'legacy invocation',samples:30,warmup:5,cache:'warm filesystem; no forced eviction',prompt:'two loopback HTTP/SSE calls plus one real filesystem tool and ACP envelope delivery; fresh sessions in reused process',tool:'native tool.started to tool.finished timestamps; includes joined worker and filesystem work',search_bytes:1048700,setup:'fixture construction and process/catalog/SDK setup excluded from measured warm samples',baseline:'new capability workload; no equivalent C1 native filesystem tool'},measurements};
  await mkdir(dirname(destination),{recursive:true});await writeFile(destination,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify({destination,measurements:Object.fromEntries(Object.entries(measurements).map(([k,v])=>[k,{prompt_ms:(v as any).prompt_ms,tool_ms:(v as any).tool_ms}]))}));
} finally {await gateway.close();await rm(cwd,{recursive:true,force:true});}
