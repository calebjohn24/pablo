/** Explicit paid smoke: one Jev evaluation and one selected Vercel model call. */
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const exec = promisify(execFile);
const root = fileURLToPath(new URL('../',import.meta.url));
const binary = join(root,'target/debug/pablo');
const workspace = await realpath(await mkdtemp(join(tmpdir(),'pablo-live-jev-')));
try {
  // The executable privately parses .env. This script never reads credential bytes.
  let config = await readFile(new URL('../docs/guide/examples/jev-router.toml',import.meta.url),'utf8');
  config = config.replace('sources = [{ kind = "environment", name = "AI_GATEWAY_API_KEY" }]',
    'sources = [{ kind = "file", path = { base = "binding", name = "secrets", path = ".env" }, encoding = "dotenv", key = "AI_GATEWAY_API_KEY" }]');
  config += '\n[options.filesystem]\nenabled=false\n[options.limits]\nmax_model_calls=2\nmax_tool_calls=0\nmax_output_tokens=128\nmax_run_duration_ms=60000\n';
  const entry = join(workspace,'entry.toml'), trace = join(workspace,'trace.jsonl');
  await writeFile(entry,config);
  const env = Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('PABLO_')&&!key.startsWith('OTEL_')&&!['AI_GATEWAY_API_KEY','VERCEL_AI_GATEWAY','OPENROUTER_API_KEY'].includes(key)));
  const start = performance.now();
  const result = await exec(binary,['run','Reply with exactly OK.', '--config',entry,'--bind',`workspace=${workspace}`,'--bind',`secrets=${root}`,'--json','--trace',trace],{env,timeout:70000}).catch((error:any)=>({stdout:error.stdout,stderr:String(error.stderr).match(/config_[a-z_]+(?: at [\w/.-]+)?/)?.[0] ?? 'setup_or_process_failure',exitCode:error.code}));
  if (!result.stdout.trim()) {
    throw new Error(`Live smoke did not admit a task: ${result.stderr}`);
  }
  const task = JSON.parse(result.stdout);
  const events = (await readFile(trace,'utf8')).trim().split('\n').map(line=>JSON.parse(line));
  const models = events.filter(e=>e.type==='model.started').map(e=>e.model);
  console.log(JSON.stringify({status:task.outcome.status,failure_code:task.outcome.code??null,limit:task.outcome.limit??null,models,model_calls:task.accounting.model_calls,elapsed_ms:Math.round(performance.now()-start),credential_source:'root .env parsed privately by executable'},null,2));
  assert.equal(task.outcome.status,'completed');
  assert.equal(task.outcome.output.trim(),'OK');
  assert.equal(task.accounting.model_calls,'2');
  assert.equal(models[0],'typesafe-ai/jev');
  assert(['google/gemini-3.8-flash','openai/gpt-5.6-sol'].includes(models[1]));
} finally {
  await rm(workspace,{recursive:true,force:true});
}
