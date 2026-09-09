import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, readdir, mkdir, realpath, rm, symlink, unlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';

const exec = promisify(execFile);
const binary = fileURLToPath(new URL('../target/debug/pablo', import.meta.url));
const corpus = fileURLToPath(new URL('../docs/project/fixtures/c3-deployment/', import.meta.url));

const preset = (extra='') => `schema_version=1
[deployment]
locked=true
allowed_run_overrides=["input","limits.max_run_duration_ms"]
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="SYNTHETIC_PROVIDER_KEY"}]
[options.shell]
enabled=false
[options.filesystem]
enabled=false
[options.interfaces]
cli_output="json"
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
${extra}`;
const answer = (res: import('node:http').ServerResponse) => {
  res.writeHead(200,{'content-type':'text/event-stream'});
  res.end('data: '+JSON.stringify({choices:[{index:0,delta:{content:'configured answer'},finish_reason:null}]})+'\n\ndata: '+JSON.stringify({choices:[{index:0,delta:{},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
};

test('configured CLI and ACP use one lifecycle, identical identity and synthetic-only fixture credentials', {timeout:15000}, async () => {
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-configured-')));
  const seen: any[]=[];
  const gateway=await server(async (req,res)=>{
    assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');
    const payload=JSON.parse((await body(req)).toString()); seen.push(payload);
    assert.equal(payload.model,'google/gemini-3.8-flash');
    assert(!payload.tools || payload.tools.length===0);
    answer(res);
  });
  try {
    const file=join(cwd,'entry.toml'); await writeFile(file,preset());
    const args=['--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
    const env={...cleanEnv(),SYNTHETIC_PROVIDER_KEY:'real-provider-synthetic-sentinel',AI_GATEWAY_API_KEY:'ambient-private-sentinel',OTEL_TRACES_EXPORTER:'otlp',OTEL_EXPORTER_OTLP_ENDPOINT:gateway.url+'/forbidden',OTEL_EXPORTER_OTLP_HEADERS:'private=ambient-private-sentinel'};
    const explained=await invoke(['config','explain',...args],env); assert.equal(explained.code,0,explained.stderr);
    const expected=JSON.parse(explained.stdout).fingerprint;
    const cli=await invoke(['run','synthetic identical input',...args],env); assert.equal(cli.code,0,cli.stderr);
    assert.equal(JSON.parse(cli.stdout).outcome.output,'configured answer');
    let identity:unknown;
    await withPablo({binary,args,env},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'synthetic identical input'}]});
      const outcome=outcomeOf(response); assert.equal(outcome.status,'completed');
      assert(outcome.status==='completed'); assert.equal(outcome.output,'configured answer');
      identity=(response._meta?.['pablo/v1'] as any).deployment;
    });
    assert.deepEqual(identity,{schema_version:1,contract_revision:'c3.4',fingerprint:expected});
    assert.equal(seen.length,2); assert.deepEqual(seen[0],seen[1]);
    const traces=(await readdir(cwd)).filter(name=>name.endsWith('.jsonl'));assert.equal(traces.length,2);
    for(const name of traces){
      const text=await readFile(join(cwd,name),'utf8');const events=text.trim().split('\n').map(line=>JSON.parse(line));
      assert.equal(events[0].deployment.fingerprint,expected);
      assert.equal(events.filter(e=>e.type==='run.finished').length,1);
      for(const secret of ['real-provider-synthetic-sentinel','ambient-private-sentinel',cwd]) assert(!text.includes(secret));
    }
  } finally {await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('legacy and configured CLI and ACP read beyond the former filesystem file cap', {timeout:20000}, async () => {
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-fs-unlimited-')));
  let calls=0;
  const gateway=await server(async (req,res)=>{
    const request=JSON.parse((await body(req)).toString()); calls++;
    if(request.messages.at(-1).role==='tool') {
      const result=JSON.parse(request.messages.at(-1).content);
      assert.equal(result.status,'completed');
      assert.equal(result.filesystem.text,'evidence');
      assert.equal(result.filesystem.size_bytes,9*1024*1024+8);
      assert.equal(result.filesystem.truncated,true);
      answer(res);
    } else {
      res.writeHead(200,{'content-type':'text/event-stream'});
      res.end('data: '+JSON.stringify({choices:[{index:0,delta:{tool_calls:[{index:0,id:'read_large',type:'function',function:{name:'fs_read',arguments:JSON.stringify({path:'large.txt',max_bytes:8})}}]},finish_reason:'tool_calls'}]})+'\n\ndata: [DONE]\n\n');
    }
  });
  try {
    await writeFile(join(cwd,'large.txt'),'evidence'+'x'.repeat(9*1024*1024));
    const file=join(cwd,'entry.toml'); await writeFile(file,preset().replace('[options.filesystem]\nenabled=false','[options.filesystem]\nenabled=true'));
    for(const configured of [false,true]) {
      const args=configured?['--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions']:['--no-shell'];
      const env={...cleanEnv(),OTEL_TRACES_EXPORTER:'none',PABLO_FIXTURE_ENDPOINT:gateway.url+'/v1/chat/completions'};
      const cli=await invoke(['run','Read evidence',...args,...(configured?[]:['--workspace',cwd,'--json'])],env);
      assert.equal(cli.code,0,cli.stderr); assert.equal(JSON.parse(cli.stdout).outcome.status,'completed');
      await withPablo({binary,args,env},async cx=>{
        await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
        const result=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read evidence'}]});
        assert.equal(outcomeOf(result).status,'completed');
      });
    }
    assert.equal(calls,8);
  } finally {await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('configured CLI rejects forbidden equal assignments, bad credentials and trace limits before effects', async () => {
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-config-denial-')));
  let requests=0;const gateway=await server((_req,res)=>{requests++;answer(res);});
  try{
    const file=join(cwd,'entry.toml');await writeFile(file,preset());
    const args=['run','task','--config',file,'--bind',`workspace=${cwd}`];
    for(const extras of [['--no-shell'],['--timeout','3601'],['--env-file','/no/read'],['--json']]){
      const result=await invoke([...args,'--fixture-endpoint',gateway.url+'/v1/chat/completions',...extras]);
      assert.equal(result.code,2);assert.equal(result.stdout,'');assert.match(result.stderr,/config_/);
    }
    const invalid=await invoke(args,{...cleanEnv(),SYNTHETIC_PROVIDER_KEY:''});
    assert.equal(invalid.code,2);assert.equal(invalid.stdout,'');assert.match(invalid.stderr,/config_credential_invalid/);
    await writeFile(file,preset().replace('path={base="workspace",path="{session_id}.jsonl"}','path={base="workspace",path="{session_id}.jsonl"}\nmax_bytes=1'));
    const tiny=await invoke([...args,'--fixture-endpoint',gateway.url+'/v1/chat/completions']);
    assert.equal(tiny.code,2);assert.equal(tiny.stdout,'');
    assert.equal(requests,0);assert.deepEqual((await readdir(cwd)).filter(name=>name.endsWith('.jsonl')),[]);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
async function invoke(args: string[], env = cleanEnv()) {
  try { return { code: 0, ...await exec(binary, args, { env, timeout: 5000, maxBuffer: 9 * 1024 * 1024 }) }; }
  catch (error) {
    const result = error as { code: number; stdout: string; stderr: string };
    assert.equal(typeof result.code, 'number');
    return result;
  }
}

test('offline inspection preserves the golden config, renders deployably and ignores ambient secrets/exporters', async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-inspection-')));
  let requests = 0;
  const receiver = await server((_req, res) => { requests++; res.end(); });
  try {
    const env = { ...cleanEnv(), AI_GATEWAY_API_KEY: 'synthetic-private-key',
      OTEL_TRACES_EXPORTER: 'otlp', OTEL_EXPORTER_OTLP_ENDPOINT: receiver.url,
      PABLO_FIXTURE_ENDPOINT: receiver.url, OTEL_EXPORTER_OTLP_HEADERS: 'private=synthetic-private-header' };
    const flags = ['--config', join(corpus, 'production.toml'), '--bind', `workspace=${cwd}`];
    const validation = await invoke(['config', 'validate', ...flags], env);
    assert.equal(validation.code, 0, validation.stderr);
    const explained = await invoke(['config', 'explain', ...flags], env);
    assert.equal(explained.code, 0, explained.stderr);
    const golden = JSON.parse(await readFile(join(corpus, 'production.resolved.json'), 'utf8'));
    assert.deepEqual(JSON.parse(explained.stdout), golden);
    const rendered = await invoke(['config', 'render', ...flags], env);
    assert.equal(rendered.code, 0, rendered.stderr);
    assert(!rendered.stdout.includes('synthetic-private'));
    assert(!rendered.stdout.includes(cwd));
    const entry = join(cwd, 'rendered.toml');
    await writeFile(entry, rendered.stdout);
    const reloaded = await invoke(['config', 'explain', '--config', entry, '--bind', `workspace=${cwd}`], env);
    assert.equal(reloaded.code, 0, reloaded.stderr);
    const actual = JSON.parse(reloaded.stdout);
    assert.deepEqual(actual.config, golden.config);
    assert.equal(actual.fingerprint, golden.fingerprint);
    assert.notEqual(actual.input_fingerprint, golden.input_fingerprint);
    assert.equal(requests, 0);
  } finally { await receiver.close(); await rm(cwd, { recursive: true, force: true }); }
});

test('inspection errors are bounded exit 2 diagnostics with empty stdout including --json', async () => {
  for (const args of [
    ['config', 'validate'],
    ['config', 'explain', '--config', join(corpus, 'invalid/unknown-option.toml'), '--json'],
    ['config', 'validate', '--config', join(corpus, 'production.toml')],
    ['config', 'render', '--config', join(corpus, 'production.toml'), '--bind', 'workspace=/tmp', '--bind', 'workspace=/tmp'],
  ]) {
    const result = await invoke(args);
    assert.equal(result.code, 2);
    assert.equal(result.stdout, '');
    assert.match(result.stderr, /config_/);
    assert(result.stderr.length < 1100);
  }
});

test('only declared environment values enter inspection and changed files affect the next invocation', async () => {
  const cwd = await realpath(await mkdtemp(join(tmpdir(), 'pablo-config-env-')));
  try {
    const entry = join(cwd, 'entry.toml');
    const text = `schema_version=1\n[credentials.gateway]\nconsumer="provider.vercel"\nsources=[{kind="environment",name="SYNTHETIC_ABSENT"}]\n[environment.TEST_CALLS]\noption="limits.max_tool_calls"\nrequired=true\n`;
    await writeFile(entry, text);
    const args = ['config', 'explain', '--config', entry, '--bind', `workspace=${cwd}`];
    assert.equal((await invoke(args)).code, 2);
    const first = await invoke(args, { ...cleanEnv(), TEST_CALLS: '3' });
    assert.equal(first.code, 0, first.stderr);
    assert.equal(JSON.parse(first.stdout).config.options.limits.max_tool_calls, 3);
    await writeFile(entry, text + '\n[options.shell]\nenabled=false\n');
    const second = await invoke(args, { ...cleanEnv(), TEST_CALLS: '3' });
    assert.equal(second.code, 0, second.stderr);
    assert.notEqual(JSON.parse(first.stdout).fingerprint, JSON.parse(second.stdout).fingerprint);
    const invalid = await invoke(args, { ...cleanEnv(), TEST_CALLS: 'synthetic-private-invalid' });
    assert.equal(invalid.code, 2);
    assert.equal(invalid.stdout, '');
    assert(!invalid.stderr.includes('synthetic-private-invalid'));
  } finally { await rm(cwd, { recursive: true, force: true }); }
});

test('ACP keeps active policy snapshot and applies edits only to later tasks', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-policy-snapshot-')));
  const arrived=Promise.withResolvers<void>();const release=Promise.withResolvers<void>();
  let requests=0;
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());requests++;
    if(requests===1){arrived.resolve();await release.promise;}
    if(request.messages.at(-1).role==='tool') {
      assert.equal(JSON.parse(request.messages.at(-1).content).filesystem.text,'snapshot evidence');answer(res);
    } else {
      res.writeHead(200,{'content-type':'text/event-stream'});
      res.end('data: '+JSON.stringify({choices:[{index:0,delta:{tool_calls:[{index:0,id:'snapshot_read',type:'function',function:{name:'fs_read',arguments:'{"path":"evidence.txt"}'}}]},finish_reason:'tool_calls'}]})+'\n\ndata: [DONE]\n\n');
    }
  });
  try {
    const file=join(cwd,'entry.toml');const initial=preset().replace('[options.filesystem]\nenabled=false','[options.filesystem]\nenabled=true');
    await writeFile(file,initial);await writeFile(join(cwd,'evidence.txt'),'snapshot evidence');
    const identities:string[]=[];
    await withPablo({binary,args:['--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'],env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      const first=await cx.request('session/new',{cwd,mcpServers:[]});
      const active=cx.request('session/prompt',{sessionId:first.sessionId,prompt:[{type:'text',text:'Read evidence'}]});
      await arrived.promise;
      await writeFile(file,initial+'\n[options.policy.tools]\ndefault="allow"\ndeny=[{id="later.deny",value="fs.read"}]\n');
      release.resolve();const response=await active;assert.equal(outcomeOf(response).status,'completed');
      identities.push((response._meta!['pablo/v1'] as any).deployment.fingerprint);
      const second=await cx.request('session/new',{cwd,mcpServers:[]});
      const denied=await cx.request('session/prompt',{sessionId:second.sessionId,prompt:[{type:'text',text:'Read evidence'}]});
      assert.equal(outcomeOf(denied).status,'policy_denied');
      identities.push((denied._meta!['pablo/v1'] as any).deployment.fingerprint);
    });
    assert.notEqual(identities[0],identities[1]);assert.equal(requests,3);
  }finally{release.resolve();await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('configured startup and session errors are bounded and do not admit a run', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-config-errors-')));
  const outside=await realpath(await mkdtemp(join(tmpdir(),'pablo-config-outside-')));
  let requests=0;const gateway=await server((_req,res)=>{requests++;answer(res);});
  try {
    const file=join(cwd,'entry.toml');const child=join(cwd,'child');await mkdir(child);
    const base=preset().replace('allowed_run_overrides=["input","limits.max_run_duration_ms"]','allowed_run_overrides=["input","run.workspace"]');
    const args=['--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
    await writeFile(file,'schema_version=1\nprivate-invalid-source = [');
    const invalid=await invoke(['acp','--stdio',...args]);
    assert.equal(invalid.code,2);assert.equal(invalid.stdout,'');assert(!invalid.stderr.includes('private-invalid-source'));
    await writeFile(file,base+'\n[[authority]]\nid="host.workspace"\nworkspace_roots=[{base="binding",name="workspace",path="."}]\n');
    // Startup validation has no real session; a file named startup.jsonl must not collide.
    await writeFile(join(cwd,'startup.jsonl'),'existing evidence');
    await withPablo({binary,args,env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      await assert.rejects(cx.request('session/new',{cwd:outside,mcpServers:[]}),(error:any)=>error.code===-32602&&JSON.stringify(error).includes('config_'));
      const {sessionId}=await cx.request('session/new',{cwd:child,mcpServers:[]});
      await writeFile(file,base+'\n[options.model]\nunknown="private-invalid-source"');
      await assert.rejects(cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'task'}]}),(error:any)=>error.code===-32602&&!JSON.stringify(error).includes('private-invalid-source'));
    });
    assert.equal(requests,0);assert.deepEqual((await readdir(cwd)).filter(n=>n.endsWith('.jsonl')),['startup.jsonl']);
    assert.equal(await readFile(join(cwd,'startup.jsonl'),'utf8'),'existing evidence');
    assert.deepEqual(await readdir(child),[]);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});await rm(outside,{recursive:true,force:true});}
});

test('ACP refreshes physical workspace bindings without changing portable deployment identity', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-binding-rotation-')));
  let requests=0;const evidence:string[]=[];
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());requests++;
    if(request.messages.at(-1).role==='tool') {evidence.push(JSON.parse(request.messages.at(-1).content).filesystem.text);answer(res);}
    else {res.writeHead(200,{'content-type':'text/event-stream'});res.end('data: '+JSON.stringify({choices:[{index:0,delta:{tool_calls:[{index:0,id:'binding_read',type:'function',function:{name:'fs_read',arguments:'{"path":"evidence.txt"}'}}]},finish_reason:'tool_calls'}]})+'\n\ndata: [DONE]\n\n');}
  });
  try {
    const roots=[join(cwd,'first'),join(cwd,'second')];
    for(let i=0;i<2;i++){await mkdir(roots[i]);await writeFile(join(roots[i],'evidence.txt'),`workspace-${i}`);}
    const link=join(cwd,'workspace');await symlink(roots[0],link);
    const file=join(cwd,'entry.toml');await writeFile(file,preset().replace('[options.filesystem]\nenabled=false','[options.filesystem]\nenabled=true'));
    const identities:string[]=[];
    await withPablo({binary,args:['--config',file,'--bind',`workspace=${link}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'],env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      for(let i=0;i<2;i++) {
        if(i===1){await unlink(link);await symlink(roots[1],link);}
        const {sessionId}=await cx.request('session/new',{cwd:roots[i],mcpServers:[]});
        const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read evidence'}]});
        assert.equal(outcomeOf(response).status,'completed');identities.push((response._meta!['pablo/v1'] as any).deployment.fingerprint);
      }
    });
    assert.deepEqual(evidence,['workspace-0','workspace-1']);assert.equal(requests,4);assert.equal(identities[0],identities[1]);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('production preset admits equivalent CLI ACP and Rust core runs, and rejects forbidden overrides before delivery', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-production-admission-')));
  const requests:any[]=[];
  const gateway=await server(async(req,res)=>{requests.push(JSON.parse((await body(req)).toString()));assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');answer(res);});
  try {
    const endpoint=gateway.url+'/v1/chat/completions';
    const flags=['--config',join(corpus,'production.toml'),'--bind',`workspace=${cwd}`];
    const explained=await invoke(['config','explain',...flags]);assert.equal(explained.code,0);
    const expected=JSON.parse(explained.stdout);
    const cli=await invoke(['run','synthetic identical input',...flags,'--fixture-endpoint',endpoint]);
    assert.equal(cli.code,0,cli.stderr);assert.equal(cli.stdout,'configured answer\n');
    await withPablo({binary,args:[...flags,'--fixture-endpoint',endpoint],env:cleanEnv()},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'synthetic identical input'}]});
      assert.equal(outcomeOf(response).status,'completed');assert.equal((response._meta!['pablo/v1'] as any).deployment.fingerprint,expected.fingerprint);
    });
    const core=fileURLToPath(new URL('../target/debug/examples/measure',import.meta.url));
    const direct=JSON.parse((await exec(core,['http',endpoint,'synthetic identical input',...flags],{env:cleanEnv(),timeout:5000})).stdout);
    assert.equal(direct.outcome.status,'completed');assert.equal(direct.deployment.fingerprint,expected.fingerprint);
    assert.equal(requests.length,3);assert.deepEqual(requests[0],requests[1]);assert.deepEqual(requests[1],requests[2]);
    assert.deepEqual(requests[0].tools.map((t:any)=>t.function.name),['fs_read','fs_list','fs_search']);
    const narrowed=await invoke(['run','synthetic identical input',...flags,'--fixture-endpoint',endpoint,'--timeout','300']);
    assert.equal(narrowed.code,0,narrowed.stderr);assert.equal(requests.length,4);
    for(const override of [['--timeout','601'],['--no-shell'],['--allow-write'],['--capture-content'],['--policy',join(cwd,'must-not-open')]]) {
      const rejected=await invoke(['run','task',...flags,'--fixture-endpoint',endpoint,...override]);
      assert.equal(rejected.code,2);assert.equal(rejected.stdout,'');assert.match(rejected.stderr,/config_(authority_violation|override_forbidden)/);
    }
    assert.equal(requests.length,4);assert.deepEqual(await readdir(cwd),[]);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('CLI policy replacement keeps independent authority allowlists and deciding rule identities', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-policy-ceilings-')));
  let tool='fs_read';let calls=0;
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());calls++;
    if(request.messages.at(-1).role==='tool') {
      const result=JSON.parse(request.messages.at(-1).content);
      assert.equal(result.filesystem.text,'read evidence');
      assert(result.policy_decisions.includes('a.read'));assert(result.policy_decisions.includes('b.read'));answer(res);
    } else {
      const args=tool==='fs_read'?{path:'evidence.txt'}:tool==='fs_list'?{path:'.'}:{path:'.',query:'evidence'};
      res.writeHead(200,{'content-type':'text/event-stream'});res.end('data: '+JSON.stringify({choices:[{index:0,delta:{tool_calls:[{index:0,id:'ceiling_call',type:'function',function:{name:tool,arguments:JSON.stringify(args)}}]},finish_reason:'tool_calls'}]})+'\n\ndata: [DONE]\n\n');
    }
  });
  try {
    const file=join(cwd,'entry.toml');const policy=join(cwd,'ordinary.json');
    await writeFile(join(cwd,'evidence.txt'),'read evidence');await writeFile(policy,JSON.stringify({tools:{default:'allow',allow:[],deny:[]}}));
    await writeFile(file,preset().replace('locked=true','locked=false').replace('[options.filesystem]\nenabled=false','[options.filesystem]\nenabled=true')+`
[options.policy.tools]
default="deny"
[options.policy.read_roots]
default="deny"
[[authority]]
id="a"
[authority.policy.tools]
default="deny"
allow=[{id="a.read",value="fs.read"},{id="a.list",value="fs.list"}]
[[authority]]
id="b"
[authority.policy.tools]
default="deny"
allow=[{id="b.read",value="fs.read"},{id="b.search",value="fs.search"}]
`);
    for(tool of ['fs_read','fs_list','fs_search']) {
      const result=await invoke(['run','task','--config',file,'--bind',`workspace=${cwd}`,'--policy',policy,'--fixture-endpoint',gateway.url+'/v1/chat/completions']);
      assert.equal(result.code,tool==='fs_read'?0:1,result.stderr);
      assert.equal(JSON.parse(result.stdout).outcome.status,tool==='fs_read'?'completed':'policy_denied');
    }
    assert.equal(calls,4);
    const broken=await invoke(['run','task','--config',file,'--bind',`workspace=${cwd}`,'--policy',join(cwd,'missing')]);
    assert.equal(broken.code,2);assert.equal(broken.stdout,'');assert.match(broken.stderr,/config_invalid_value/);assert.equal(calls,4);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
