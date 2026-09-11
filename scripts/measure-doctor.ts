/** Offline process and self-reported doctor duration; no configured command is launched. */
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,realpath,readFile,writeFile,rm,readdir} from 'node:fs/promises';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {cleanEnv} from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free source fingerprint helper.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url)),binary=join(root,'target/release/pablo'),exec=promisify(execFile);
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-doctor-measure-'))),entry=join(cwd,'entry.toml');
const checkpoint=process.env.PABLO_MEASURE_CHECKPOINT??'C3.31';assert.match(checkpoint,/^C[0-9]+\.[0-9]+[a-z]?$/);
const modes:Record<string,unknown>={};
const stats=(samples:number[])=>{const sorted=samples.toSorted((a,b)=>a-b);return {n:sorted.length,min:sorted[0],p50:sorted[14],p95:sorted[28],max:sorted[29]};};
try{
 await writeFile(entry,'schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="DOCTOR_KEY"}]\n[options.mcp.servers.local]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","touch must-not-launch; exit 1"]\n[options.children]\nenabled=true\n');
 for(const [mode,args] of [['legacy',[]],['configured',['--config',entry,'--bind',`workspace=${cwd}`]]] as const){
  const processMs:number[]=[],diagnosisMs:number[]=[];
  for(let i=0;i<35;i++){const start=performance.now();const {stdout}=await exec(binary,['doctor','--json',...args],{cwd,env:{...cleanEnv(),AI_GATEWAY_API_KEY:'synthetic-doctor-measure',DOCTOR_KEY:'synthetic-doctor-measure'},timeout:5000});const elapsed=performance.now()-start,report=JSON.parse(stdout);assert.equal(report.exit_code,0);assert.equal(report.probe_result,'not probed');assert(report.duration_ms<1000&&elapsed<1000,'offline doctor exceeds one second');if(i>=5){processMs.push(elapsed);diagnosisMs.push(report.duration_ms);}}
  modes[mode]={process_ms:stats(processMs),diagnosis_ms:stats(diagnosisMs),samples:{process_ms:processMs,diagnosis_ms:diagnosisMs}};
 }
 assert.deepEqual(await readdir(cwd),['entry.toml']);
 const report={checkpoint,timestamp:new Date().toISOString(),source_sha256:await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),platform:`${process.platform}/${process.arch}`,method:'30 samples per mode after 5 warmups; each is a fresh release doctor process. Synthetic local credentials; configured MCP launcher must never run. Process duration includes startup; diagnosis duration is reported by doctor before serialization.',modes};
 await writeFile(join(root,`.pablo/measurements/${checkpoint.toLowerCase()}-doctor.json`),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}finally{await rm(cwd,{recursive:true,force:true});}
