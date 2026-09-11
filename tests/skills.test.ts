import assert from 'node:assert/strict';
import {test} from 'node:test';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdtemp,mkdir,writeFile,readFile,realpath,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {cleanEnv} from './fixtures/telemetry.ts';
const exec=promisify(execFile);const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));

test('S01 configured inspection lists portable metadata without credentials, activation or ambient roots',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-skills-host-')));
  try{
    await mkdir(join(cwd,'.agents/skills/read'),{recursive:true});
    const file=join(cwd,'.agents/skills/read/SKILL.md');await writeFile(file,'---\nname: read\ndescription: Read synthetic evidence.\nmetadata:\n  version: "1"\n---\nPRIVATE_INSTRUCTIONS\n');
    const entry=join(cwd,'entry.toml');const config='schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="MISSING_KEY"}]\n';
    await writeFile(entry,config);const flags=['--config',entry,'--bind',`workspace=${cwd}`];const env=cleanEnv();
    const empty=JSON.parse((await exec(binary,['skills','list',...flags],{cwd,env})).stdout);assert.deepEqual(empty.entries,[]);assert.deepEqual(empty.roots,[]);
    await writeFile(entry,config+'[options.skills.roots]\nworkspace={base="workspace",path=".agents/skills"}\n');
    const first=(await exec(binary,['skills','list',...flags],{cwd,env})).stdout;const catalog=JSON.parse(first);
    assert.equal(catalog.entries.length,1);assert.equal(catalog.entries[0].qualified_name,'workspace/read');assert.equal(catalog.roots[0].path,join(cwd,'.agents/skills'));assert(!first.includes('PRIVATE_INSTRUCTIONS'));
    await writeFile(file,(await readFile(file,'utf8')).replace('PRIVATE_INSTRUCTIONS','OTHER_BODY'));
    assert.equal((await exec(binary,['skills','list',...flags],{cwd,env})).stdout,first,'body changes cannot affect a metadata-only catalog');
    const shown=JSON.parse((await exec(binary,['skills','show','read',...flags],{cwd,env})).stdout);assert.deepEqual(shown,catalog.entries[0]);
    await assert.rejects(exec(binary,['skills','show','missing',...flags],{cwd,env}));
    await writeFile(entry,config+'[options.skills.roots]\nworkspace={base="source",path=".agents/skills"}\n[[authority]]\nid="host"\nskill_roots=[{base="workspace",path=".agents/skills"}]\n');
    const rendered=(await exec(binary,['config','render',...flags],{cwd,env})).stdout;assert(rendered.includes('skill_roots'));assert(!rendered.includes('PRIVATE_INSTRUCTIONS'));
    assert.equal(JSON.parse((await exec(binary,['skills','list',...flags],{cwd,env})).stdout).entries.length,1);
    await writeFile(entry,(await readFile(entry,'utf8')).replace('skill_roots=[{base="workspace",path=".agents/skills"}]','skill_roots=[]'));
    await assert.rejects(exec(binary,['skills','list',...flags],{cwd,env}),(e:any)=>e.stderr.includes('config_authority_violation')&&e.stdout==='');
    await writeFile(entry,config+'[options.skills.roots]\nworkspace={base="workspace",path="../escape"}\n');
    await assert.rejects(exec(binary,['config','validate',...flags],{cwd,env}));
  }finally{await rm(cwd,{recursive:true,force:true});}
});
