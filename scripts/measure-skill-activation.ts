/** Offline S02 activation/resource timing; no credentials or live providers. */
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,mkdir,writeFile,readFile,realpath,rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {cleanEnv,body,server} from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));const binary=join(root,'target/release/pablo');const exec=promisify(execFile);
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skill-activation-measure-')));
const names=Array.from({length:8},(_,n)=>`skill-${n}`);const resource='SYNTHETIC_RESOURCE_'.repeat(1024);
const sha=(value:string|Buffer)=>createHash('sha256').update(value).digest('hex');
let calls=0;let initialRequestBytes=0;let continuationRequestBytes=0;
const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
const gateway=await server(async(req,res)=>{
  const bytes=await body(req);const request=JSON.parse(bytes.toString());calls++;
  const active=request.messages.filter((m:any)=>m.role==='user'&&m.content.startsWith('Explicitly activated Skill:'));
  assert.equal(active.length,8);assert(!request.messages.find((m:any)=>m.role==='system')?.content.includes('SYNTHETIC_INSTRUCTION'));
  res.writeHead(200,{'content-type':'text/event-stream'});
  if(request.messages.at(-1).role==='tool'){
    continuationRequestBytes=bytes.length;const result=JSON.parse(request.messages.at(-1).content).skill.resource;assert.equal(result.text,resource);assert.equal(result.sha256,sha(resource));
    res.end(frame({content:'Consumed selected resource.'})+frame({},'stop')+'data: [DONE]\n\n');
  }else{
    initialRequestBytes=bytes.length;
    res.end(frame({tool_calls:[{index:0,id:'resource',type:'function',function:{name:'skill_read',arguments:JSON.stringify({skill:'host/skill-0',path:'evidence.txt'})}}]},'tool_calls')+'data: [DONE]\n\n');
  }
});
try{
  let instructionBytes=0;
  for(const name of names){
    const dir=join(cwd,'skills',name);await mkdir(dir,{recursive:true});const instructions='SYNTHETIC_INSTRUCTION '.repeat(384);instructionBytes+=Buffer.byteLength(instructions);
    await writeFile(join(dir,'SKILL.md'),`---\nname: ${name}\ndescription: Read synthetic evidence.\n---\n${instructions}`);
  }
  await writeFile(join(cwd,'skills/skill-0/evidence.txt'),resource);
  const entry=join(cwd,'entry.toml');await writeFile(entry,`schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.shell]\nenabled=false\n[options.filesystem]\nenabled=false\n[options.skills]\nactivate=${JSON.stringify(names)}\n[options.skills.roots]\nhost={base="workspace",path="skills"}\n`);
  const samples:number[]=[];
  for(let i=0;i<35;i++){
    const start=performance.now();const {stdout}=await exec(binary,['run','Read selected evidence.','--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url],{cwd,env:cleanEnv(),timeout:10000,maxBuffer:1024*1024});const elapsed=performance.now()-start;
    assert.equal(stdout.trim(),'Consumed selected resource.');if(i>=5)samples.push(elapsed);
  }
  assert.equal(calls,70);const values=samples.toSorted((a,b)=>a-b);const bytes=await readFile(binary);
  const report={checkpoint:'C3.20',source_sha256:await sourceFingerprint(root),binary_sha256:sha(bytes),binary_bytes:bytes.length,platform:`${process.platform}/${process.arch}`,method:{samples:30,warmup:5,skills:8,instruction_body_bytes:instructionBytes,resource_bytes:Buffer.byteLength(resource),model_calls_per_run:2,initial_request_bytes:initialRequestBytes,continuation_request_bytes:continuationRequestBytes,timing:'CLI startup, configuration, fresh activation of eight Skills, two loopback model calls and one actual selected resource read; fixture construction excluded; warm filesystem; no live provider'},stats:{min:values[0],p50:values[14],p95:values[28],max:values[29]}};
  await writeFile(join(root,'.pablo/measurements/c3.20-skill-activation.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
