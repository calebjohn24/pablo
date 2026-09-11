/** Warm terminal process; fresh independent native task on every submission. */
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {cleanEnv} from '../tests/fixtures/telemetry.ts';
// @ts-expect-error Dependency-free source fingerprint helper.
import {sourceFingerprint} from './lib/source-fingerprint.mjs';
const root=fileURLToPath(new URL('../',import.meta.url)),binary=join(root,'target/release/pablo');
const {stdout}=await promisify(execFile)(join(root,'.pablo/a2a-fixture-venv/bin/python'),[join(root,'tests/fixtures/tui/basic.py'),binary,'35'],{env:cleanEnv(),timeout:30000});
const proof=JSON.parse(stdout);assert.equal(proof.tasks,36);assert.equal(proof.submit_to_completed_screen_ms.length,35);assert(proof.cancelledThroughRunToken&&proof.freshTaskContext&&proof.oscNeutralized&&proof.terminalRestored);
const samples=proof.submit_to_completed_screen_ms.slice(5),sorted=samples.toSorted((a:number,b:number)=>a-b);
const checkpoint=process.env.PABLO_MEASURE_CHECKPOINT ?? 'C3.29';
assert.match(checkpoint, /^C[0-9]+\.[0-9]+[a-z]?$/);
const report={checkpoint,timestamp:new Date().toISOString(),source_sha256:await sourceFingerprint(root),binary_sha256:createHash('sha256').update(await readFile(binary)).digest('hex'),platform:`${process.platform}/${process.arch}`,method:{samples:30,warmup:5,workload:'PTY input to completed screen for one immediate local HTTP/SSE model response per fresh native task; same terminal process. Renderer capped at 20 frames/s; nonblocking input polls at 5 ms. Process/fixture startup excluded. Includes screen-delivery observation, not isolated runtime or live-model latency.',cleanup:'Final real run-token cancellation and terminal-mode restoration verified; terminal owner and HTTP fixture joined.'},stats:{submit_to_completed_screen_ms:{min:sorted[0],p50:sorted[14],p95:sorted[28],max:sorted[29]}},samples_ms:samples};
await writeFile(join(root,`.pablo/measurements/${checkpoint.toLowerCase()}-tui.json`),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
