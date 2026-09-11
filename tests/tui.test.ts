import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {join} from 'node:path';
import {cleanEnv} from './fixtures/telemetry.ts';
const exec=promisify(execFile),root=fileURLToPath(new URL('../',import.meta.url)),binary=join(root,'target/debug/pablo');
test('T01 redirected no-argument help and explicit TUI rejection preserve ordinary output',async()=>{
 const help=await exec(binary,[],{env:cleanEnv()});assert(help.stdout.includes('Usage:'));assert(!help.stdout.includes('\x1b'));
 await assert.rejects(exec(binary,['tui'],{env:cleanEnv()}),(error:any)=>error.code===2&&!error.stdout.includes('\x1b')&&error.stderr.includes('terminal'));
 const demo=await exec(binary,['demo'],{env:cleanEnv()});assert.equal(demo.stdout,'Hello from pablo.\n');assert(!demo.stdout.includes('\x1b'));
});
const python=join(root,'.pablo/a2a-fixture-venv/bin/python');
test('T01 PTY composer streams independent tasks and cancellation and restores modes',{timeout:20000,skip:process.platform==='win32'||!existsSync(python)},async()=>{
 for(const extra of [[],['auto']]) {
 const {stdout}=await exec(python,[join(root,'tests/fixtures/tui/basic.py'),binary,...extra],{env:cleanEnv(),timeout:15000});
 assert.deepEqual(JSON.parse(stdout),{surface:'tui',tasks:3,cancelledThroughRunToken:true,freshTaskContext:true,streamed:true,oscNeutralized:true,terminalRestored:true});
 }
});
