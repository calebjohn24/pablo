import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdir,mkdtemp,readFile,writeFile,stat,realpath,rm} from 'node:fs/promises';
import {tmpdir,cpus,release,totalmem} from 'node:os';
import {resolve,join,delimiter} from 'node:path';
import {tasks,score,hash,suiteVersion} from './tasks.mjs';
import {openRouterKey,privateKey} from './credentials.mjs';
import {astraCost} from './cost.mjs';
import {command,models,normalize} from './adapters.mjs';
import {latencyProfiles,traceDiagnostics,traceTimings} from './latency.mjs';
let interrupted=false;
const root=resolve(import.meta.dirname,'../..'), exec=promisify(execFile);
export function aggregate(rows) {
  const quantile=(xs,p)=>xs.length?xs.toSorted((a,b)=>a-b)[Math.ceil(xs.length*p)-1]:null;
  return Object.fromEntries([...new Set(rows.map(r=>r.harness))].map(h=>{
    const all=rows.filter(r=>r.harness===h), run=all.filter(r=>r.resources);
    const estimates=all.map(r=>r.events?.estimated_cost_usd).filter(Number.isFinite);
    const factualChecks=all.flatMap(r=>r.correctness?.checks??Array.from({length:r.factual_total??tasks(r.seed??42).find(t=>t.id===r.task)?.questions.length??0},()=>({value:false})));
    const reportedCosts=all.map(r=>r.events?.reported_cost_usd).filter(Number.isFinite);
    const values=k=>run.map(r=>r.resources[k]).filter(Number.isFinite);
    return [h,{attempts:all.length,measured:run.length,passed:all.filter(r=>r.correctness?.pass&&r.status==='completed').length,pass_rate:all.filter(r=>r.correctness?.pass&&r.status==='completed').length/all.length,
      factual_correct:factualChecks.filter(c=>c.value).length,factual_total:factualChecks.length,factual_accuracy:factualChecks.length?factualChecks.filter(c=>c.value).length/factualChecks.length:null,
      mean_factual_score:all.reduce((s,r)=>s+(r.correctness?.factualScore??0),0)/all.length,
      mean_citation_score:all.reduce((s,r)=>s+(r.correctness?.citationScore??0),0)/all.length,
      completed_wall_p50_ms:quantile(all.filter(r=>r.status==='completed').map(r=>r.resources?.wall_ms).filter(Number.isFinite),.5),
      successful_wall_p50_ms:quantile(all.filter(r=>r.status==='completed'&&r.correctness?.pass).map(r=>r.resources?.wall_ms).filter(Number.isFinite),.5),
      wall_p50_ms:quantile(values('wall_ms'),.5),wall_p95_ms:quantile(values('wall_ms'),.95),
      peak_tree_rss_kib:values('sampled_tree_peak_rss_kib').length?Math.max(...values('sampled_tree_peak_rss_kib')):null,
      cpu_p50_s:quantile(values('cpu_total_s'),.5),cpu_one_core_p50_percent:quantile(values('cpu_percent_one_core'),.5),
      estimated_cost_usd:estimates.length===all.length?estimates.reduce((a,b)=>a+b,0):null,
      reported_cost_usd:reportedCosts.length===all.length?reportedCosts.reduce((a,b)=>a+b,0):null,reported_cost_known_subtotal_usd:reportedCosts.length?reportedCosts.reduce((a,b)=>a+b,0):null,cost_reported_attempts:reportedCosts.length,
      failures:all.filter(r=>r.status!=='completed').map(r=>({task:r.task,status:r.status}))}];
  }));
}
function options(argv) {
  const o={harnesses:'pablo,pablo-astra,codex,claude,ori,pi',seeds:'42',tasks:'all',repeats:'1',timeout:'180',output:'.pablo/measurements/knowledge-work',live:false,'reuse-pablo-openrouter':false,'codex-api-key-file':null,'latency-matrix':false};
  for(let i=0;i<argv.length;i++) {const k=argv[i].replace(/^--/,'');if(k==='live'||k==='reuse-pablo-openrouter'||k==='latency-matrix')o[k]=true;else if(Object.hasOwn(o,k))o[k]=argv[++i];else throw Error(`Unknown option ${argv[i]}`);}
  for(const k of ['repeats','timeout']){o[k]=Number(o[k]);if(!Number.isInteger(o[k])||o[k]<1||o[k]>(k==='repeats'?20:3600))throw Error(`Invalid ${k}`);}
  o.harnesses=o.harnesses.split(',');if(o.harnesses.some(h=>!Object.hasOwn(models,h)))throw Error('Unknown harness');
  o.seeds=o.seeds.split(',').map(Number);o.seeds.forEach(s=>tasks(s));if(new Set(o.harnesses).size!==o.harnesses.length||new Set(o.seeds).size!==o.seeds.length)throw Error('Duplicate harnesses or seeds');return o;
}
async function launch(specPath,env) {
  const child=spawn('python3',[resolve(import.meta.dirname,'monitor.py'),specPath],{env,stdio:['ignore','ignore','pipe']});
  let error='';child.stderr.on('data',b=>{error+=b;});
  const forward=()=>{interrupted=true;child.kill('SIGTERM');};process.once('SIGINT',forward);process.once('SIGTERM',forward);
  try {await new Promise((res,rej)=>{child.on('error',rej);child.on('close',c=>c===0?res():rej(Error(`monitor exited ${c}: ${error.slice(-500)}`)));});}
  finally {process.removeListener('SIGINT',forward);process.removeListener('SIGTERM',forward);}
}
export async function main(argv=process.argv.slice(2)) {
  const o=options(argv), output=resolve(root,o.output), selected=o.seeds.flatMap(seed=>tasks(seed).map(t=>({...t,seed}))).filter(t=>o.tasks==='all'||o.tasks.split(',').includes(t.id));
  if(!selected.length)throw Error('No selected tasks');
  const profiles=o['latency-matrix']?latencyProfiles(o.harnesses):o.harnesses.map(h=>({id:h,harness:h}));
  const matrix={suiteVersion,profiles,models,harnesses:o.harnesses,tasks:selected.map(t=>({id:t.id,seed:t.seed,fingerprint:t.fingerprint})),repeats:o.repeats,planned_runs:selected.length*profiles.length*o.repeats};
  if(!o.live){console.log(JSON.stringify({...matrix,message:'Dry run only. Add --live to invoke providers. No credentials are read by this runner.'},null,2));return;}
  await mkdir(resolve(root,'.pablo'),{recursive:true});const lock=resolve(root,'.pablo/knowledge-work.lock');await mkdir(lock);
  let base;
  try {
    await mkdir(output); // Refuse to overwrite retained results.
    base=await realpath(await mkdtemp(join(tmpdir(),'pablo-knowledge-')));
    const env={};for(const k of ['PATH','HOME','TMPDIR','LANG','USER','SHELL','CODEX_HOME','OPENAI_API_KEY','ANTHROPIC_API_KEY','CLAUDE_CODE_OAUTH_TOKEN','OPENROUTER_API_KEY'])if(process.env[k])env[k]=process.env[k];
    env.PATH=resolve(root,'.pablo/knowledge-tools/node_modules/.bin')+delimiter+(env.PATH??'');env.ORI_TELEMETRY='0';env.OTEL_TRACES_EXPORTER='none';
    const paths={},builds={};
    for(const h of o.harnesses) {
      const cmd=command(h,{root,workspace:base,prompt:'version',timeout:o.timeout})[0];
      try {
        const path=cmd.includes('/')?await realpath(cmd):(await exec('which',[cmd],{env})).stdout.trim();
        const version=(await exec(path,['--version'],{env,cwd:base,timeout:15000})).stdout.trim();
        paths[h]=path;builds[h]={path,version,entry_sha256:hash(await readFile(path)),entry_bytes:(await stat(path)).size};
      } catch {builds[h]={unavailable:true,reason:'executable/version preflight failed'};}
    }
    const sourceFiles=['tasks.mjs','adapters.mjs','run.mjs','monitor.py','credentials.mjs','cost.mjs','latency.mjs'];
    const sourceHashes=Object.fromEntries(await Promise.all(sourceFiles.map(async f=>[f,hash(await readFile(join(import.meta.dirname,f)))])));
    const sharedKey=o['reuse-pablo-openrouter']&&o.harnesses.some(h=>h==='ori'||h==='pi')?await openRouterKey(resolve(root,'.env')):null;
    const codexKey=o['codex-api-key-file']?await privateKey(resolve(o['codex-api-key-file']),'OPENAI_API_KEY'):null;
    const rows=[];
    const report={...matrix,sourceHashes,timestamp:new Date().toISOString(),platform:{os:process.platform,arch:process.arch,kernel:release(),cpu:cpus()[0].model,logical_cpus:cpus().length,memory_bytes:totalmem(),node:process.version,python:(await exec('python3',['--version'])).stdout.trim()},builds,workspace_root:base,
      method:{execution:'serial fresh process and workspace; rotated harness order per task/repetition; no warmup or retries',timing:'process spawn through reaping and pipe cleanup, includes network inference/tools/startup; no server-side inference CPU/RAM visibility',permissions:'equivalent local read/write/shell task capability, different native enforcement; no browsing/delegation requested',models:'GLM via OpenRouter; lab clients native provider/subscription; model and harness effects confounded across model families',effort_profiles:profiles,effort:o['latency-matrix']?Object.fromEntries(profiles.map(p=>[p.id,p.reasoning??p.piThinking])):{pablo:'provider default','pablo-astra':'provider default',pi:'off',ori:'none / Pi off',codex:'medium',claude:'high'},scoring:'exact factual values and required filename sets; no semantic judge; prose existence is not prose quality',credentials:sharedKey?'Explicit private Pablo OpenRouter key reuse for Ori/Pi via child environment; no credential argv or saved key files':'Native clients resolve credentials; Pablo privately loads root .env',limitations:'sampled RSS includes shared pages, misses short processes; wait4 CPU can miss unjoined descendants; native tools/prompts differ; no cache flush; p95 with a small sample is descriptive only'},rows,summary:{}};
    let n=0;
    trials: for(let rep=0;rep<o.repeats;rep++)for(const task of selected) {
      const order=profiles.map((_,i)=>profiles[(i+n)%profiles.length]);n++;
      for(const profile of order) {
        const h=profile.harness;
        const id=`${task.id}-s${task.seed}-r${rep}-${profile.id}`, trial=join(base,id), workspace=join(trial,'workspace'), artifacts=join(output,id);
        await mkdir(workspace,{recursive:true});await mkdir(artifacts);
        for(const[name,content]of Object.entries(task.files))await writeFile(join(workspace,name),content);
        await writeFile(join(artifacts,'prompt.txt'),task.prompt);
        const row={id,harness:profile.id,base_harness:h,reasoning:profile.reasoning??null,pi_thinking:profile.piThinking??null,model:models[h],task:task.id,seed:task.seed,repeat:rep,fingerprint:task.fingerprint,factual_total:task.questions.length,status:'unavailable',resources:null,correctness:null};
        if(!builds[h].unavailable) {
          const cmd=command(h,{root,workspace,prompt:task.prompt,timeout:o.timeout,paths,reasoning:profile.reasoning,piThinking:profile.piThinking});
          row.command=cmd.map(a=>a===task.prompt?'<prompt.txt>':a);row.workspace=workspace;
          const spec=join(trial,'monitor-spec.json');await writeFile(spec,JSON.stringify({argv:cmd,cwd:workspace,output:artifacts,timeout_s:o.timeout,sample_ms:100}));
          console.log(`Running ${id}`);
          try {
            const trialEnv={...env};if(sharedKey&&(h==='ori'||h==='pi'))trialEnv.OPENROUTER_API_KEY=sharedKey;if(codexKey&&h==='codex')trialEnv.CODEX_API_KEY=codexKey;
            await launch(spec,trialEnv);row.resources=JSON.parse(await readFile(join(artifacts,'resources.json'),'utf8'));
            const raw=await readFile(join(artifacts,'stdout.jsonl'),'utf8'),receipts=(await readFile(join(artifacts,'receipts.jsonl'),'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse);
            row.events=normalize(h,raw,receipts);
            if(codexKey&&h==='codex'){row.authentication='openai_api_key';Object.assign(row.events,astraCost(row.events.reported_usage));}
            const diagnostic=raw+'\n'+await readFile(join(artifacts,'stderr.jsonl'),'utf8');
            if(/No API key found for openrouter|not signed in to OpenRouter/i.test(diagnostic))row.failure_category='authentication_unavailable';
            if(h.startsWith('pablo')) {try {const trace=(await readFile(join(trial,'pablo-trace.jsonl'),'utf8')).trim().split('\n').map(JSON.parse);row.events.reported_model=trace.find(e=>e.type==='model.started')?.model??null;row.model_calls=traceDiagnostics(trace);row.timings=traceTimings(trace,row.resources.wall_ms);await writeFile(join(artifacts,'model-diagnostics.json'),JSON.stringify(row.model_calls,null,2)+'\n');}catch{}}
            let answer=null,prose='';
            // Reject symlinks and oversized artifacts before scoring/copying.
            for(const name of ['answer.json','report.md']) {
              try {const p=join(workspace,name);if(await realpath(p)!==p||(await stat(p)).size>1024*1024)continue;const text=await readFile(p,'utf8');await writeFile(join(artifacts,name),text);if(name==='answer.json')answer=JSON.parse(text);else prose=text;} catch {}
            }
            row.correctness=score(task,answer,prose);
            row.sources_unchanged=true;
            for(const[name,content]of Object.entries(task.files))try {if(await readFile(join(workspace,name),'utf8')!==content)row.sources_unchanged=false;}catch{row.sources_unchanged=false;}
            row.status=row.resources.failure??(row.resources.exit_code!==0||row.events.errors.length?'client_error':!row.events.completed?'incomplete':row.events.reported_model&&row.events.reported_model!==models[h]?'model_mismatch':row.sources_unchanged?'completed':'source_modified');
          } catch(e) {row.status='runner_error';row.error=e.message;}
        }
        rows.push(row);report.summary=aggregate(rows);await writeFile(join(output,'report.json'),JSON.stringify(report,null,2)+'\n');
        console.log(`${id}: ${row.status}; factual ${row.correctness?.factualScore??0}, citations ${row.correctness?.citationScore??0}`);
        if(interrupted)break trials;
      }
    }
    report.interrupted=interrupted;await writeFile(join(output,'report.json'),JSON.stringify(report,null,2)+'\n');
    if(interrupted)process.exitCode=130;
    const table=['| Harness / model | Facts | Completed-task wall p50 s | Peak tree MiB | CPU p50 s | Total cost USD |','|---|---:|---:|---:|---:|---:|'];
    const fmt=(n,d=1)=>n==null?'—':n.toFixed(d);
    for(const[h,s]of Object.entries(report.summary))table.push(`| ${h} / ${models[profiles.find(p=>p.id===h)?.harness??h]} | ${s.factual_accuracy==null?'—':fmt(s.factual_accuracy*100)+'%'} | ${fmt(s.completed_wall_p50_ms==null?null:s.completed_wall_p50_ms/1000)} | ${fmt(s.peak_tree_rss_kib==null?null:s.peak_tree_rss_kib/1024)} | ${fmt(s.cpu_p50_s,3)} | ${fmt(s.reported_cost_usd??s.estimated_cost_usd,4)}${s.reported_cost_usd==null&&s.estimated_cost_usd!=null?' (estimate)':''} |`);
    await writeFile(join(output,'report.md'),`# Knowledge-work benchmark\n\n${table.join('\n')}\n\nFailures remain in the denominator. Table latency includes completed client runs, including wrong answers; JSON also retains passed-only and all-attempt latency. Authentication failures are not workload measurements. Missing metrics are not zero. This is a model + harness comparison, with subjective prose quality ungraded.\n`);
    console.log(JSON.stringify({output,summary:report.summary},null,2));
  } finally {await rm(lock,{recursive:true,force:true});}
}
if(process.argv[1]&&resolve(process.argv[1])===resolve(import.meta.filename))main().catch(e=>{console.error(e.message);process.exitCode=1;});
