/** Offline S01 discovery timing; fixture construction is outside measured work. */
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,mkdir,writeFile,readFile,realpath,rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {cleanEnv} from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free Node helper is JavaScript.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));const binary=join(root,'target/release/pablo');const exec=promisify(execFile);
const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skills-measure-')));
try{
  for(let n=0;n<128;n++){
    const name=`skill-${String(n).padStart(3,'0')}`;const dir=join(cwd,'.agents/skills',name);await mkdir(dir,{recursive:true});
    await writeFile(join(dir,'SKILL.md'),`---\nname: ${name}\ndescription: Read synthetic evidence ${n}.\nmetadata:\n  version: "1"\n---\n`+'PRIVATE_BODY'.repeat(8192));
  }
  const entry=join(cwd,'entry.toml');await writeFile(entry,'schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="UNREAD_KEY"}]\n[options.skills.roots]\nworkspace={base="workspace",path=".agents/skills"}\n');
  const samples:number[]=[];let metadataBytes=0;
  for(let i=0;i<35;i++){
    const start=performance.now();const {stdout}=await exec(binary,['skills','list','--config',entry,'--bind',`workspace=${cwd}`],{cwd,env:cleanEnv(),timeout:10000,maxBuffer:1024*1024});const elapsed=performance.now()-start;
    const catalog=JSON.parse(stdout);assert.equal(catalog.entries.length,128);assert.equal(catalog.diagnostics.length,0);assert(!stdout.includes('PRIVATE_BODY'));metadataBytes=catalog.metadata_bytes;
    if(i>=5)samples.push(elapsed);
  }
  const values=samples.toSorted((a,b)=>a-b);
  const report={checkpoint:'C3.19',source_sha256:await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),platform:`${process.platform}/${process.arch}`,method:{samples:30,warmup:5,skills:128,metadata_bytes:metadataBytes,body_bytes_per_skill:12*8192,timing:'CLI process startup, offline configuration, fresh no-follow discovery and complete JSON output; fixture construction excluded; warm filesystem'},stats:{min:values[0],p50:values[14],p95:values[28],max:values[29]}};
  const destination=join(root,'.pablo/measurements/c3.19-skills.json');await writeFile(destination,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}finally{await rm(cwd,{recursive:true,force:true});}
