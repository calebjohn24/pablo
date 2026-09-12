import {readFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {aggregate} from './run.mjs';
const quantile=(xs,p)=>xs.length?xs.toSorted((a,b)=>a-b)[Math.ceil(xs.length*p)-1]:null;
export function compare(rows, base, candidate) {
 const a=rows.filter(r=>r.harness===base),b=rows.filter(r=>r.harness===candidate),s=aggregate([...a,...b]);
 if(!a.length||!b.length)return {promote:false,reason:'missing comparison'};
 const x=s[base],y=s[candidate],families={};
 for(const task of new Set(a.map(r=>r.task))) {
  const t=aggregate([...a,...b].filter(r=>r.task===task));
  families[task]={default_facts:t[base]?.factual_accuracy??null,low_facts:t[candidate]?.factual_accuracy??null};
 }
 const key=r=>`${r.task}/${r.seed}/${r.repeat}`,keys=new Set(a.map(key));
 const gates={matched_attempts:a.length===54&&b.length===54&&keys.size===a.length&&new Set(b.map(key)).size===b.length&&b.every(r=>keys.has(key(r))),
  p50:y.wall_p50_ms!=null&&x.wall_p50_ms!=null&&y.wall_p50_ms<=x.wall_p50_ms*.8,
  p95:y.wall_p95_ms!=null&&x.wall_p95_ms!=null&&y.wall_p95_ms<=x.wall_p95_ms,
  facts:y.factual_accuracy>=x.factual_accuracy,
  task_families:Object.values(families).every(f=>f.low_facts!=null&&f.default_facts!=null&&f.low_facts>=f.default_facts),
  completion:b.filter(r=>r.status==='completed').length>=a.filter(r=>r.status==='completed').length,
  cost:y.reported_cost_usd!=null&&x.reported_cost_usd!=null&&Math.round(y.reported_cost_usd*1e6)<=Math.round(x.reported_cost_usd*1e6)};
 const paired=a.flatMap(r=>{const other=b.find(t=>key(t)===key(r));return r.resources?.wall_ms&&other?.resources?.wall_ms?[{task:r.task,seed:r.seed,repeat:r.repeat,ratio:other.resources.wall_ms/r.resources.wall_ms}]:[];});
 return {promote:false,numeric_gates_pass:Object.values(gates).every(Boolean),gates,detail_review:'required before promotion',families,p50_improvement:1-y.wall_p50_ms/x.wall_p50_ms,paired_ratio_p50:quantile(paired.map(r=>r.ratio),.5),paired};
}
export function summarize(report) {
 const summary=aggregate(report.rows),calls={};
 for(const h of new Set(report.rows.map(r=>r.harness))) {
  const rows=report.rows.filter(r=>r.harness===h),all=rows.flatMap(r=>r.model_calls??[]),metrics={};
  for(const field of ['preparation_us','dispatch_us','headers_us','first_data_us','first_text_us','first_tool_delta_us','terminal_us','complete_us','reasoning_tokens']) {
   const values=all.map(c=>c.diagnostics?.[field]).filter(Number.isFinite);metrics[field]={known:values.length,p50:quantile(values,.5),p95:quantile(values,.95)};
  }
  const fractions=rows.filter(r=>r.timings?.runtime_us>0&&r.timings?.model_us!=null).map(r=>r.timings.model_us/r.timings.runtime_us);
  calls[h]={count:all.length,metrics,model_fraction_p50:quantile(fractions,.5),http_versions:[...new Set(all.map(c=>c.diagnostics?.http_version).filter(Boolean))]};
  // Strict task passes are internal legacy scorer data, never headline metrics.
  delete summary[h].passed;delete summary[h].pass_rate;delete summary[h].successful_wall_p50_ms;
 }
 return {suiteVersion:report.suiteVersion,timestamp:report.timestamp,profiles:report.profiles,planned_runs:report.planned_runs,attempts:report.rows.length,builds:Object.fromEntries(Object.entries(report.builds).map(([k,{path,...v}])=>[k,v])),sourceHashes:report.sourceHashes,platform:report.platform,method:report.method,summary,calls,
  comparisons:{glm:compare(report.rows,'pablo-provider_default','pablo-low'),astra:compare(report.rows,'pablo-astra-provider_default','pablo-astra-low')},
  rows:report.rows.map(r=>({id:r.id,harness:r.harness,model:r.model,task:r.task,seed:r.seed,repeat:r.repeat,fingerprint:r.fingerprint,factual_total:r.factual_total,status:r.status,resources:r.resources?Object.fromEntries(Object.entries(r.resources).filter(([k])=>k!=="samples")):null,correctness:r.correctness?{checks:r.correctness.checks,reportPresent:r.correctness.reportPresent,factualScore:r.correctness.factualScore,citationScore:r.correctness.citationScore}:null,events:r.events,model_calls:r.model_calls,timings:r.timings}))};
}
if(process.argv[1]&&resolve(process.argv[1])===resolve(import.meta.filename)) {
 const report=JSON.parse(await readFile(process.argv[2],'utf8'));
 await writeFile(process.argv[3],JSON.stringify(summarize(report),null,2)+'\n');
}
