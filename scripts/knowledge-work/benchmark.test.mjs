import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,writeFile,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {tasks,score,hash} from './tasks.mjs';
import {command,normalize} from './adapters.mjs';
import {aggregate} from './run.mjs';
const exec=promisify(execFile);
const answer=t=>({findings:t.questions.map(({id,value,sources})=>({id,value,sources}))});
const report='Professional handoff with facts, rationale, citations, open questions and next actions. '.repeat(2);
test('seed reproducibility, hidden answers, known independent arithmetic',()=>{
  assert.deepEqual(tasks(42),tasks(42));assert.notEqual(tasks(42)[0].fingerprint,tasks(43)[0].fingerprint);
  assert.equal(tasks(42)[0].questions[2].value,2066);
  assert.equal(tasks(42)[2].questions[1].value,11760);
  assert.equal(tasks(42)[5].questions[0].value,13);
  for(const t of tasks()) {assert(!Object.keys(t.files).some(n=>/answer|key/.test(n)));assert(score(t,answer(t),report).pass);}
  assert.equal(hash(Buffer.from('abc')),'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
});
test('wrong facts, fabricated sources, duplicate findings, missing reports fail',()=>{
  const t=tasks()[0];let a=answer(t);a.findings[0].value=-1;assert(!score(t,a,report).pass);
  a=answer(t);a.findings[0].sources=['invented.md'];assert(!score(t,a,report).pass);
  a=answer(t);a.findings.push(a.findings[0]);assert(!score(t,a,report).pass);
  assert(!score(t,answer(t),'').pass);assert(!score(t,null,report).pass);
  a=answer(t);a.findings[0].value=String(a.findings[0].value);assert(!score(t,a,report).pass);
});
test('array ordering does not affect score; unknown/extraneous fields rejected',()=>{
  const t=tasks()[2],a=answer(t);a.findings.reverse();a.findings[0].value=[...a.findings[0].value].reverse();assert(score(t,a,report).pass);
  a.extra='invented';assert(!score(t,a,report).pass);
});
test('adapters preserve model identities, use argv and disable reuse',()=>{
  const opts={root:'/repo',workspace:'/workspace',prompt:'literal `pwd` $(whoami)',timeout:30};
  for(const h of ['pablo','pablo-astra','codex','claude','pi','ori']) {const c=command(h,opts);assert(c.includes(opts.prompt));assert(!c.includes('--continue'));}
  assert(command('pablo-astra',opts).includes('openai/gpt-6-astra'));assert(command('pablo-astra',opts)[0].endsWith('/pablo'));assert(command('ori',opts).includes('pi')); assert(command('pablo',opts).includes('z-ai/glm-5.3-flash'));
});
test('event metrics distinguish completed message from streaming; errors retained',()=>{
  const c=normalize('codex','{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}\n{"type":"turn.failed"}\n',[{stream:'stdout',end_byte:1000,ms:17}]);
  assert.equal(c.first_assistant_observed_ms,17);assert.match(c.first_assistant_boundary,/not first token/);assert.deepEqual(c.errors,['turn.failed']);
  const p=normalize('pi',[{type:'message_end',message:{role:'assistant',model:'glm',usage:{input:4,output:2,cost:{total:.01}}}},{type:'message_end',message:{role:'assistant',model:'glm',usage:{input:8,output:3,cost:{total:.02}}}}].map(JSON.stringify).join('\n'));
  assert.equal(p.reported_usage.input,12);assert.equal(p.reported_cost_usd,.03);assert.equal(p.first_assistant_observed_ms,null);
});
test('failures remain in correctness denominator; missing metrics stay null',()=>{
  const s=aggregate([{harness:'pi',status:'unavailable'},{harness:'pi',status:'completed',correctness:{pass:true,factualScore:1,citationScore:1},resources:{wall_ms:10}}]).pi;
  assert.equal(s.pass_rate,.5);assert.equal(s.mean_factual_score,.5);assert.equal(s.cpu_p50_s,null);assert.equal(s.peak_tree_rss_kib,null);
});
async function monitor(code,timeout=4) {
  const dir=await mkdtemp(join(tmpdir(),'knowledge-monitor-test-'));
  const spec=join(dir,'spec.json');await writeFile(spec,JSON.stringify({argv:['python3','-c',code],cwd:dir,output:dir,timeout_s:timeout,sample_ms:50}));
  try {await exec('python3',[resolve(import.meta.dirname,'monitor.py'),spec],{timeout:10000});return {result:JSON.parse(await readFile(join(dir,'resources.json'),'utf8')),stdout:await readFile(join(dir,'stdout.jsonl'),'utf8')};} finally {await rm(dir,{recursive:true,force:true});}
}
test('resource monitor observes child memory and CPU, captures output and reaps',async()=>{
  const {result:r,stdout}=await monitor('import subprocess,sys; p=subprocess.Popen([sys.executable,"-c","import time; b=bytearray(32*1024*1024); t=time.monotonic();\\nwhile time.monotonic()-t<0.6: pass"]); print("started",flush=True); p.wait()');
  assert.equal(r.exit_code,0);assert.equal(r.failure,null);assert(r.cpu_total_s>.2);assert(r.sampled_tree_peak_rss_kib>32000);assert(r.samples.some(s=>s.processes>=2));assert.match(stdout,/started/);assert(r.first_stdout_ms<r.wall_ms);
});
test('timeout kills process group and returns bounded failure',async()=>{
  const {result:r}=await monitor('import subprocess,sys,time; subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"]); time.sleep(60)',.3);
  assert.equal(r.failure,'timeout');assert(r.exit_code<0);assert(r.wall_ms<4000);
});
test('full runner retains artifacts and report without reading provider credentials',async()=>{
  const dir=await mkdtemp(join(tmpdir(),'knowledge-runner-test-'));
  const bin=join(dir,'fake-pablo'),output=join(dir,'results');
  const fixture=answer(tasks()[0]);
  const source=`#!/usr/bin/env node\nimport fs from 'node:fs';\nif(process.argv.includes('--version')){console.log('fixture 1');process.exit(0);}\nfs.writeFileSync('answer.json',${JSON.stringify(JSON.stringify(fixture))});\nfs.writeFileSync('report.md',${JSON.stringify(report)});\nconsole.log(JSON.stringify({outcome:{status:'completed'},accounting:{tool_calls:'1',cost_microusd:'2'}}));\n`;
  await writeFile(bin,source,{mode:0o755});
  try {
    await exec('node',[resolve(import.meta.dirname,'run.mjs'),'--live','--harnesses','pablo','--tasks','reconciliation','--output',output],{env:{...process.env,PABLO_KNOWLEDGE_PABLO_BINARY:bin},timeout:10000});
    const r=JSON.parse(await readFile(join(output,'report.json'),'utf8'));
    assert.equal(r.summary.pablo.passed,1);assert.equal(r.rows[0].events.tool_calls,1);assert.equal(r.rows[0].events.reported_cost_usd,.000002);
    assert(r.sourceHashes['monitor.py']);assert.match(await readFile(join(output,'report.md'),'utf8'),/1\/1/);
    await assert.rejects(exec('node',[resolve(import.meta.dirname,'run.mjs'),'--live','--harnesses','pablo','--tasks','reconciliation','--output',output],{env:{...process.env,PABLO_KNOWLEDGE_PABLO_BINARY:bin},timeout:10000}));
    await rm(r.workspace_root,{recursive:true,force:true});
  } finally {await rm(dir,{recursive:true,force:true});}
});

test('handoff provides the year required by its date answer key and uses current evidence',()=>{
  const t=tasks().find(t=>t.id==='meeting-handoff');
  assert.match(t.prompt,/All dates in these documents refer to 2026/);
  assert.deepEqual(t.questions.find(q=>q.id==='open_actions').sources,['meeting-0911.md','email-0912.md']);
  assert(t.questions.filter(q=>q.id==='launch_date'||q.id==='legal_due').every(q=>q.value.startsWith('2026-')));
});

test('timeout also kills an observed tool in a separate process group',async()=>{
  const {result:r,stdout}=await monitor('import subprocess,sys,time; p=subprocess.Popen([sys.executable,"-c","import time; time.sleep(60)"],start_new_session=True); print(p.pid,flush=True); time.sleep(60)',.4);
  const pid=Number(stdout.trim());assert(Number.isInteger(pid)&&pid>0);assert.equal(r.failure,'timeout');
  try {const {stdout:state}=await exec('ps',['-o','stat=','-p',String(pid)]);assert(state.trim()===''||state.trim().startsWith('Z'),'separate-group tool still running');}
  catch(e) {if(e.code!==1)throw e;}
  finally {try {process.kill(pid,'SIGKILL');}catch{}}
});
