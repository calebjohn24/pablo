// Small serial transport experiment. Build both binaries before invoking --live.
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdir,writeFile,readFile,stat,rm} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {traceDiagnostics} from './latency.mjs';
import {hash} from './tasks.mjs';
const exec=promisify(execFile),root=resolve(import.meta.dirname,'../..');
if(!process.argv.includes('--live')) { console.log('Dry run: 24 serial text-only calls; GLM and Astra, low effort, alternating HTTP/1.1 and HTTP/2-capable binaries. Add --live after building .pablo/latency/pablo-h1 and pablo-h2.'); }
else {
 const env={OTEL_TRACES_EXPORTER:"none"};for(const k of ["PATH","HOME","TMPDIR","LANG","USER","SHELL","OPENROUTER_API_KEY"])if(process.env[k])env[k]=process.env[k];
 const output=resolve(root,'.pablo/measurements/latency-transport'),lock=resolve(root,'.pablo/knowledge-work.lock');
 await mkdir(lock);
 try {
  await mkdir(output);const rows=[],builds={};
  for(const variant of ['h1','h2']){const binary=resolve(root,`.pablo/latency/pablo-${variant}`);builds[variant]={binary,sha256:hash(await readFile(binary)),bytes:(await stat(binary)).size};}
  for(let rep=0;rep<6;rep++)for(const model of ['z-ai/glm-5.3-flash','openai/gpt-6-astra'])for(const variant of rep%2?['h2','h1']:['h1','h2']) {
   const id=`${model.startsWith('openai')?'astra':'glm'}-${rep}-${variant}`,workspace=join(output,id);await mkdir(workspace);
   const trace=join(workspace,'trace.jsonl'),argv=[builds[variant].binary,'run','Reply exactly OK. Do not call tools.','--provider','openrouter','--model',model,'--reasoning-effort','low','--no-shell','--no-filesystem','--workspace',workspace,'--env-file',resolve(root,'.env'),'--timeout','60','--json','--trace',trace];
   const spec=join(workspace,'monitor.json');await writeFile(spec,JSON.stringify({argv,cwd:workspace,output:workspace,timeout_s:60,sample_ms:100}));
   await exec('python3',[resolve(import.meta.dirname,'monitor.py'),spec],{timeout:70000,env});
   const resources=JSON.parse(await readFile(join(workspace,'resources.json'),'utf8'));
   let task=null,calls=[];try{task=JSON.parse(await readFile(join(workspace,'stdout.jsonl'),'utf8'));calls=traceDiagnostics((await readFile(trace,'utf8')).trim().split('\n').map(JSON.parse));}catch{}
   rows.push({id,model,variant,repeat:rep,resources,status:task?.outcome?.status??'failed',correct:task?.outcome?.output?.trim()==='OK',cost_usd:task?.accounting?.cost_microusd==null?null:Number(task.accounting.cost_microusd)/1e6,calls});
   await writeFile(join(output,'report.json'),JSON.stringify({builds,method:'Fresh process per call, six alternating pairs per model; low effort, same prompt, serial. Header wait includes network and provider processing. Pool reuse is checked by separate controlled fixtures.',rows},null,2)+'\n');
   console.log(`${id}: ${rows.at(-1).status}; ${calls[0]?.diagnostics?.http_version??'unknown'}; ${Math.round(resources.wall_ms)} ms`);
  }
 }finally{await rm(lock,{recursive:true,force:true});}
}
