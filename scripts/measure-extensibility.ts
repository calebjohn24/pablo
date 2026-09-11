/** Complete E01 ACP workload with MCP, Skill, handoffs, compaction and native trace. */
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {extensibilityFixture} from '../tests/fixtures/extensibility.ts';
// @ts-expect-error Dependency-free source fingerprint helper.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url)),binary=join(root,'target/release/pablo');
const samples:number[]=[],ratios:number[]=[];
for(let i=0;i<35;i++){
  const fixture=await extensibilityFixture();
  try{
    const started=performance.now();const task=await fixture.run(binary);const elapsed=performance.now()-started;
    const proof=await fixture.verify(task);
    assert.equal(proof.counts.rootCalls+proof.counts.childCalls,17);
    if(i>=5){samples.push(elapsed);ratios.push(Number(proof.compaction.after_bytes)/Number(proof.compaction.before_bytes));}
  }finally{await fixture.close();}
}
const stats=(values:number[])=>{const s=values.toSorted((a,b)=>a-b);return {min:s[0],p50:s[14],p95:s[28],max:s[29]};};
const report={checkpoint:'C3.25',source_sha256:await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),platform:`${process.platform}/${process.arch}`,node:process.version,method:{samples:30,warmup:5,timing:'Complete fresh ACP process including MCP startup, Skill activation/read, two-child fan-out, forced overflow and compaction with active/completed children, post-compaction MCP reuse, verified handoff/fan-in, native trace and joined shutdown. Fixture setup/verification excluded; each iteration has an independent Python MCP process. Sequential after builds/tests.',models:17,tools:11,child_runs:3,compactions:1,comparison:'Combined workload baseline only; includes Python startup and synthetic HTTP delivery, not a causal comparison or live-provider latency.'},stats:{fresh_acp_ms:stats(samples),compaction_after_before_bytes:stats(ratios)}};
await writeFile(join(root,'.pablo/measurements/c3.25-extensibility.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
