import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile, spawn } from 'node:child_process';
import { once } from 'node:events';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';
import { existsSync } from 'node:fs';
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';
import { body, cleanEnv, server } from './fixtures/telemetry.ts';

const exec=promisify(execFile);
const root=fileURLToPath(new URL('../',import.meta.url));
const binary=join(root,'target/debug/pablo');
const python=join(root,'.pablo/mcp-fixture-venv/bin/python');
const fixture=join(root,'tests/fixtures/mcp/server.py');
const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
const config=()=>`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="NEVER_READ_PROVIDER"}]
[credentials.mcp]
consumer="mcp.env"
sources=[{kind="environment",name="MCP_FIXTURE_TOKEN"}]
[options.shell]
enabled=false
[options.filesystem]
enabled=false
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
[options.mcp.servers.local]
transport="stdio"
command=${JSON.stringify(python)}
args=[${JSON.stringify(fixture)}]
env={FIXTURE_TOKEN="mcp"}
`;

test('M04 denied tools stay out of host catalogs and ACP selection is revalidated before launch',{skip:!existsSync(python)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-selection-')));let calls=0;
  const gateway=await server(async(req,res)=>{const value=JSON.parse((await body(req)).toString());calls++;assert.equal(value.tools,undefined);res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({content:'No tool admitted.'})+frame({},'stop')+'data: [DONE]\n\n');});
  try{
    const entry=join(cwd,'entry.toml');const denied=config()+'\n[[options.mcp.policies]]\n[options.mcp.policies.tools]\ndefault="deny"\n';
    await writeFile(entry,denied);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];const env={...cleanEnv(),MCP_FIXTURE_TOKEN:'synthetic-mcp-token'};
    assert.equal((await exec(binary,['run','test',...args],{cwd,env,timeout:5000})).stdout.trim(),'No tool admitted.');
    assert(existsSync(join(cwd,'pid')));assert(!existsSync(join(cwd,'context.json')));
    await rm(join(cwd,'pid'));
    const two=denied+'\n[options.mcp.servers.unselected]\ntransport="stdio"\ncommand="/bin/sh"\nargs=["-c","/usr/bin/touch unselected-started"]\n';await writeFile(entry,two);
    await withPablo({binary,args,env},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      const definition={name:'local',command:python,args:[fixture],env:[]};
      await assert.rejects(cx.request('session/new',{cwd,mcpServers:[{...definition,name:'unconfigured'}]}));
      await assert.rejects(cx.request('session/new',{cwd,mcpServers:[{...definition,args:[]}]}));
      assert(!existsSync(join(cwd,'pid')));
      const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[definition]}) as {sessionId:string};
      assert(!existsSync(join(cwd,'pid')));
      assert.equal(outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'test'}]})).status,'completed');
      const pid=Number(await readFile(join(cwd,'pid'),'utf8'));assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');
      assert(!existsSync(join(cwd,'context.json')));assert(!existsSync(join(cwd,'unselected-started')));await rm(join(cwd,'pid'));
      const next=await cx.request('session/new',{cwd,mcpServers:[definition]}) as {sessionId:string};
      await writeFile(entry,two.replace(`args=[${JSON.stringify(fixture)}]`,'args=[]'));
      await assert.rejects(cx.request('session/prompt',{sessionId:next.sessionId,prompt:[{type:'text',text:'test'}]}),(error:any)=>error.code===-32602&&String(error.data).includes('config_authority_violation'));
      assert(!existsSync(join(cwd,'pid')));assert(!existsSync(join(cwd,'unselected-started')));
    });
    assert.equal(calls,2);
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('M04 HTTP fixture overrides reject invalid destinations and definitions before credentials or launch',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-override-')));
  try{
    const entry=join(cwd,'entry.toml');await writeFile(entry,config().replace(JSON.stringify(python),'"/bin/sh"').replace(`args=[${JSON.stringify(fixture)}]`,'args=["-c","/usr/bin/touch started"]')+'\n[options.mcp.servers.remote]\ntransport="http"\nurl="https://synthetic.example/mcp"\n');
    const base=['run','test','--config',entry,'--bind',`workspace=${cwd}`];
    for(const mapping of ['unknown=http://127.0.0.1:1/mcp','local=http://127.0.0.1:1/mcp','remote=http://example.com/mcp','remote=http://user@127.0.0.1/mcp','remote=http://127.0.0.1/mcp?secret=x','remote=http://127.0.0.1/mcp#fragment']){
      await assert.rejects(exec(binary,[...base,'--fixture-endpoint','http://127.0.0.1:1','--fixture-mcp-endpoint',mapping],{cwd,env:cleanEnv(),timeout:5000}),(error:any)=>error.code===2&&error.stderr.includes('fixture_mcp_endpoint'));
      assert(!existsSync(join(cwd,'started')));
    }
    for(const extra of [['--fixture-mcp-endpoint','remote=http://127.0.0.1:1/mcp'],['--fixture-endpoint','http://127.0.0.1:1','--fixture-mcp-endpoint','remote=http://127.0.0.1:1/mcp','--fixture-mcp-endpoint','remote=http://127.0.0.1:2/mcp']])await assert.rejects(exec(binary,[...base,...extra],{cwd,env:cleanEnv(),timeout:5000}),(error:any)=>error.code===2);
    assert(!existsSync(join(cwd,'started')));
    await writeFile(entry,config().split('[options.mcp.servers.local]')[0]);
    await assert.rejects(exec(binary,[...base,'--fixture-endpoint','http://127.0.0.1:1','--fixture-mcp-endpoint','unknown=http://127.0.0.1:1/mcp'],{cwd,env:cleanEnv(),timeout:5000}),(error:any)=>error.code===2&&error.stderr.includes('fixture_mcp_endpoint'));
  }finally{await rm(cwd,{recursive:true,force:true});}
});

test('M04 built CLI and reused ACP consume fresh stdio catalogs and actual results', {skip:!existsSync(python)}, async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-host-')));
  const requests:any[]=[];
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());requests.push(request);
    assert.equal(request.model,'zai/glm-5.3-flash');
    assert.equal(request.tools.length,1);
    assert.match(request.tools[0].function.name,/^mcp_[0-9a-f]{48}$/);
    res.writeHead(200,{'content-type':'text/event-stream'});
    if(request.messages.at(-1).role==='tool'){
      const result=JSON.parse(request.messages.at(-1).content);
      assert.equal(result.status,'completed',JSON.stringify(result.mcp));
      const text=result.mcp.content.structured.text;
      assert.match(text,/^fresh evidence [012]$/);
      res.end(frame({content:text})+frame({},'stop')+'data: [DONE]\n\n');
    }else{
      res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:request.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');
    }
  });
  try{
    const entry=join(cwd,'entry.toml');await writeFile(entry,config());
    const paths=await Promise.all([0,1,2].map(async i=>{const path=join(cwd,String(i));await mkdir(path);await writeFile(join(path,'evidence.txt'),`fresh evidence ${i}`);return path;}));
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const env={...cleanEnv(),MCP_FIXTURE_TOKEN:'synthetic-mcp-token',AI_GATEWAY_API_KEY:'ambient-provider-secret',OTEL_EXPORTER_OTLP_HEADERS:'ambient-exporter-secret'};
    const cli=await exec(binary,['run','Read the evidence','--workspace',paths[0],...args],{cwd,env,timeout:10000});
    assert.equal(cli.stdout.trim(),'fresh evidence 0');
    const pids:number[]=[Number(await readFile(join(paths[0],'pid'),'utf8'))];
    await withPablo({binary,args,env},async cx=>{
      const init=await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      assert.equal(init.agentCapabilities?.mcpCapabilities?.http,true);
      assert(!init.agentCapabilities?.mcpCapabilities?.sse);
      for(let i=1;i<3;i++){
        const {sessionId}=await cx.request('session/new',{cwd:paths[i],mcpServers:[{name:'local',command:python,args:[fixture],env:[]}]});
        const outcome=outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read the evidence'}]}));
        assert.equal(outcome.status,'completed');if(outcome.status==='completed')assert.equal(outcome.output,`fresh evidence ${i}`);
        const pid=Number(await readFile(join(paths[i],'pid'),'utf8'));pids.push(pid);
        assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');
        const trace=await readFile(join(paths[i],`${sessionId}.jsonl`),'utf8');
        assert(!trace.includes('fresh evidence'));assert(!trace.includes('synthetic-mcp-token'));assert(!trace.includes('ambient-'));
      }
    });
    assert.equal(new Set(pids).size,3);assert.equal(requests.length,6);
    for(const pid of pids)assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('M04 malformed catalogs, bounded progress and tool/protocol errors retain their disposition across hosts',{skip:!existsSync(python)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-errors-host-')));let calls=0;let mode='';
  const gateway=await server(async(req,res)=>{
    const value=JSON.parse((await body(req)).toString());calls++;res.writeHead(200,{'content-type':'text/event-stream'});
    if(value.messages.at(-1).role==='tool'){
      const result=JSON.parse(value.messages.at(-1).content);assert.equal(result.status,mode==='tool_error'?'recoverable_error':'completed');
      res.end(frame({content:'Handled.'})+frame({},'stop')+'data: [DONE]\n\n');
    }else res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:value.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');
  });
  try{
    for(mode of ['frame_overflow','duplicate','schema','cursor','rpc_error','invalid_result','unsupported','progress_overflow','progress','tool_error']){
      const startup=['frame_overflow','duplicate','schema','cursor'].includes(mode);const success=['progress','tool_error'].includes(mode);
      const workspace=join(cwd,mode);await mkdir(workspace);const entry=join(workspace,'entry.toml');
      await writeFile(entry,config().replace(JSON.stringify(fixture),`${JSON.stringify(join(root,'tests/fixtures/mcp/adversarial.py'))},${JSON.stringify(mode)}`));
      const args=['--config',entry,'--bind',`workspace=${workspace}`,'--fixture-endpoint',gateway.url];const env={...cleanEnv(),MCP_FIXTURE_TOKEN:'synthetic-mcp-token'};const before=calls;
      let result;try{result=await exec(binary,['run','test','--json',...args],{cwd:workspace,env,timeout:10000});}catch(error:any){result=error;}
      if(startup)assert.match(result.stderr,/config_mcp_startup/);else assert.equal(JSON.parse(result.stdout).outcome.status,success?'completed':'failed');
      await withPablo({binary,args,env},async cx=>{
        await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});const {sessionId}=await cx.request('session/new',{cwd:workspace,mcpServers:[]});
        const prompt=cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'test'}]});
        if(startup)await assert.rejects(prompt,(error:any)=>JSON.stringify(error).includes('config_mcp_startup'));
        else {let outcome;try{outcome=outcomeOf(await prompt);}catch(error:any){outcome=error.data?.['pablo/v1']?.outcome;assert(outcome);}assert.equal(outcome.status,success?'completed':'failed');}
      });
      assert.equal(calls-before,startup?0:success?4:2,mode);
      const pid=Number(await readFile(join(workspace,'pid'),'utf8'));assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');
      for(const name of await readdir(workspace))if(name.endsWith('.jsonl')){const trace=await readFile(join(workspace,name),'utf8');assert(!trace.includes('PRIVATE_PEER_ERROR'));assert(!trace.includes('synthetic-mcp-token'));}
    }
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

for(const mode of ['json','sse']) test(`M04 CLI and ACP use independent HTTP ${mode} sessions without stale authentication/catalogs`,{skip:!existsSync(python)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-http-host-')));
  const peer=spawn(python,[join(root,'tests/fixtures/mcp/http_server.py'),mode],{cwd,env:{},stdio:['ignore','ignore','pipe']});
  let diagnostics='';peer.stderr.on('data',chunk=>{diagnostics+=chunk.toString();});
  const exited=once(peer,'exit');
  let expected='';let count=0;
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());count++;
    assert.equal(request.tools.length,1);assert.match(request.tools[0].function.name,/^mcp_[0-9a-f]{48}$/);
    res.writeHead(200,{'content-type':'text/event-stream'});
    if(request.messages.at(-1).role==='tool'){
      const result=JSON.parse(request.messages.at(-1).content);assert.equal(result.mcp.content.structured.text,expected);assert.equal(result.mcp.remote_completion_uncertain,false);
      res.end(frame({content:expected})+frame({},'stop')+'data: [DONE]\n\n');
    }else res.end(frame({tool_calls:[{index:0,id:'http-read',type:'function',function:{name:request.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');
  });
  try{
    let endpoint='';
    for(let i=0;i<500;i++){
      try{endpoint=await readFile(join(cwd,'endpoint'),'utf8');if(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/.test(endpoint))break;}catch{}
      assert.equal(peer.exitCode,null,diagnostics);await delay(10);
    }
    assert.match(endpoint,/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
    const entry=join(cwd,'entry.toml');await writeFile(entry,`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="NEVER_READ_PROVIDER"}]
[credentials.mcp]
consumer="mcp.headers"
sources=[{kind="file",path={base="workspace",path="mcp.token"},encoding="utf8"}]
[options.shell]
enabled=false
[options.filesystem]
enabled=false
[options.mcp.servers.remote]
transport="http"
url="https://synthetic.example/mcp"
headers={x-fixture-token="mcp"}
`);
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--fixture-mcp-endpoint',`remote=${endpoint}`];
    const env={...cleanEnv(),MCP_HTTP_TOKEN:'synthetic-http-token',AI_GATEWAY_API_KEY:'ambient-provider-secret',OTEL_EXPORTER_OTLP_HEADERS:'ambient-exporter-secret'};
    expected='HTTP evidence 0';await writeFile(join(cwd,'evidence.txt'),expected);
    await writeFile(join(cwd,'mcp.token'),'synthetic-http-token-0');await writeFile(join(cwd,'expected-token'),'synthetic-http-token-0');
    const cli=await exec(binary,['run','Read evidence',...args],{cwd,env,timeout:10000});assert.equal(cli.stdout.trim(),expected);
    await withPablo({binary,args,env},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      for(let i=1;i<3;i++){
        expected=`HTTP evidence ${i}`;await writeFile(join(cwd,'evidence.txt'),expected);
        await writeFile(join(cwd,'mcp.token'),`synthetic-http-token-${i}`);await writeFile(join(cwd,'expected-token'),`synthetic-http-token-${i}`);
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[{type:'http',name:'remote',url:'https://synthetic.example/mcp',headers:[]}]});
        const outcome=outcomeOf(await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'Read evidence'}]}));
        assert.equal(outcome.status,'completed');if(outcome.status==='completed')assert.equal(outcome.output,expected);
      }
    });
    const records=(await readFile(join(cwd,'requests.jsonl'),'utf8')).trim().split('\n').map(line=>JSON.parse(line));
    assert.equal(records.length,15);assert.equal(count,6);
    assert.equal(records.filter(r=>r.rpc==='initialize').length,3);assert.equal(records.filter(r=>r.method==='DELETE').length,3);
    assert.equal(new Set(records.filter(r=>r.rpc==='tools/call').map(r=>r.session)).size,3);
  }finally{
    peer.kill('SIGTERM');const settled=await Promise.race([exited.then(()=>true),delay(5000).then(()=>false)]);if(!settled){peer.kill('SIGKILL');await exited;}
    await gateway.close();await rm(cwd,{recursive:true,force:true});
  }
});

test('M04 required startup fails before dispatch; optional and denied servers are omitted',async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-startup-host-')));
  let calls=0;
  const gateway=await server(async(req,res)=>{const value=JSON.parse((await body(req)).toString());calls++;assert.equal(value.tools,undefined);res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({content:'No external tool admitted.'})+frame({},'stop')+'data: [DONE]\n\n');});
  try{
    for(const mode of ['required','optional','denied']){
      const entry=join(cwd,`${mode}.toml`);await writeFile(entry,`schema_version=1
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="NEVER_READ_PROVIDER"}]
[options.shell]
enabled=false
[options.filesystem]
enabled=false
[options.mcp.servers.local]
transport="stdio"
command=${JSON.stringify(mode==='denied'?'/bin/sh':'/not-installed/never-launch')}
args=${mode==='denied'?'["-c","/usr/bin/touch denied-started"]':'[]'}
required=${mode==='required'}
${mode==='denied'?'[[options.mcp.policies]]\n[options.mcp.policies.servers]\ndefault="deny"':''}
`);
      const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
      const before=calls;
      if(mode==='required')await assert.rejects(exec(binary,['run','test',...args],{cwd,env:cleanEnv(),timeout:5000}),(error:any)=>error.code===2&&error.stderr.includes('config_mcp_startup'));
      else assert.equal((await exec(binary,['run','test',...args],{cwd,env:cleanEnv(),timeout:5000})).stdout.trim(),'No external tool admitted.');
      await withPablo({binary,args,env:cleanEnv()},async cx=>{
        await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
        const prompt=cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'test'}]});
        if(mode==='required')await assert.rejects(prompt,(error:any)=>JSON.stringify(error).includes('config_mcp_startup'));
        else assert.equal(outcomeOf(await prompt).status,'completed');
      });
      assert.equal(calls-before,mode==='required'?0:2);
      assert(!existsSync(join(cwd,'denied-started')));
    }
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('M04 cancellation joins an active MCP process through CLI and ACP', {skip:!existsSync(python)}, async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-cancel-host-')));
  const gateway=await server(async(req,res)=>{
    const request=JSON.parse((await body(req)).toString());
    res.writeHead(200,{'content-type':'text/event-stream'});
    res.end(frame({tool_calls:[{index:0,id:'pending',type:'function',function:{name:request.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');
  });
  const waitCall=async(path:string)=>{for(let i=0;i<500&&!existsSync(join(path,'called'));i++)await delay(10);assert(existsSync(join(path,'called')));return Number(await readFile(join(path,'pid'),'utf8'));};
  try{
    const entry=join(cwd,'entry.toml');await writeFile(entry,config().replace(JSON.stringify(fixture),`${JSON.stringify(join(root,'tests/fixtures/mcp/adversarial.py'))},"call_hang"`));
    const env={...cleanEnv(),MCP_FIXTURE_TOKEN:'synthetic-mcp-token'};
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url];
    const cliDir=join(cwd,'cli');const acpDir=join(cwd,'acp');await mkdir(cliDir);await mkdir(acpDir);
    const cli=spawn(binary,['run','test','--json','--workspace',cliDir,...args],{cwd,env});let stdout='';cli.stdout.on('data',chunk=>{stdout+=chunk.toString();});cli.stderr.resume();const exited=once(cli,'exit');
    try{const pid=await waitCall(cliDir);cli.kill('SIGINT');const [code]=await exited;assert.equal(code,130);assert.equal(JSON.parse(stdout).outcome.status,'cancelled');assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');}
    finally{if(cli.exitCode===null&&cli.signalCode===null){cli.kill('SIGKILL');await exited;}}
    await withPablo({binary,args,env},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      const {sessionId}=await cx.request('session/new',{cwd:acpDir,mcpServers:[]});
      const prompt=cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'test'}]});
      const pid=await waitCall(acpDir);await cx.notify('session/cancel',{sessionId});assert.equal(outcomeOf(await prompt).status,'cancelled');assert.throws(()=>process.kill(-pid,0),(error:any)=>error.code==='ESRCH');
    });
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

test('M04 MCP work closes on ACP stdin EOF and bounded output backpressure/disconnect',{skip:!existsSync(python)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-pipes-host-')));let scenario='';
  const gateway=await server(async(req,res)=>{
    const value=JSON.parse((await body(req)).toString());res.writeHead(200,{'content-type':'text/event-stream'});
    if(value.messages.at(-1).role==='tool')res.end(Array.from({length:1024},()=>frame({content:'x'.repeat(1024)})).join('')+frame({},'stop')+'data: [DONE]\n\n');
    else res.end(frame({tool_calls:[{index:0,id:'read',type:'function',function:{name:value.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');
  });
  try{
    for(scenario of ['stdin-eof','resume','stdout-disconnect']){
      const workspace=join(cwd,scenario);await mkdir(workspace);const entry=join(workspace,'entry.toml');
      await writeFile(entry,config().replace(JSON.stringify(fixture),`${JSON.stringify(join(root,'tests/fixtures/mcp/adversarial.py'))},${JSON.stringify(scenario==='stdin-eof'?'call_hang':'large_result')}`));
      const child=spawn(binary,['acp','--stdio','--config',entry,'--bind',`workspace=${workspace}`,'--fixture-endpoint',gateway.url],{cwd:workspace,env:{...cleanEnv(),MCP_FIXTURE_TOKEN:'synthetic-mcp-token'}});
      const exited=once(child,'exit');child.stderr.resume();let buffer='';const responses=new Map<number,any>();
      child.stdout.on('data',chunk=>{buffer+=chunk.toString();assert(buffer.length<20*1024*1024);let i;while((i=buffer.indexOf('\n'))>=0){const value=JSON.parse(buffer.slice(0,i));buffer=buffer.slice(i+1);if(value.id!==undefined)responses.set(value.id,value);}});
      const send=(id:number,method:string,params:object)=>child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');
      const reply=async(id:number)=>{for(let i=0;i<1000&&!responses.has(id);i++){assert.equal(child.exitCode,null);await delay(10);}assert(responses.has(id));return responses.get(id);};
      let pid:number|undefined;
      try{
        send(1,'initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});assert((await reply(1)).result);
        send(2,'session/new',{cwd:workspace,mcpServers:[]});const sessionId=(await reply(2)).result.sessionId;
        if(scenario!=='stdin-eof')child.stdout.pause();
        send(3,'session/prompt',{sessionId,prompt:[{type:'text',text:'test'}]});
        for(let i=0;i<500&&!existsSync(join(workspace,'called'));i++)await delay(10);assert(existsSync(join(workspace,'called')));pid=Number(await readFile(join(workspace,'pid'),'utf8'));
        if(scenario==='stdin-eof')child.stdin.end();
        else {await delay(150);if(scenario==='resume'){child.stdout.resume();assert.equal(outcomeOf((await reply(3)).result).status,'completed');child.stdin.end();}else child.stdout.destroy();}
        const timer=setTimeout(()=>child.kill('SIGKILL'),5000);try{await exited;}finally{clearTimeout(timer);}
        assert.notEqual(child.signalCode,'SIGKILL','MCP host failed to settle before watchdog');
        assert.throws(()=>process.kill(-pid!,0),(error:any)=>error.code==='ESRCH');
      }finally{
        if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await exited;}
        if(pid!==undefined){try{process.kill(-pid,'SIGKILL');}catch(error:any){assert.equal(error.code,'ESRCH');}}
      }
    }
  }finally{await gateway.close();await rm(cwd,{recursive:true,force:true});}
});

for(const mode of ['disconnect','call_hang'])test(`M04 HTTP ${mode} preserves remote uncertainty through CLI and ACP`,{skip:!existsSync(python)},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-mcp-http-failure-host-')));
  const peer=spawn(python,[join(root,'tests/fixtures/mcp/http_fault_server.py'),mode],{cwd,env:{},stdio:['ignore','ignore','ignore']});const peerExit=once(peer,'exit');
  const records=async()=>{try{return(await readFile(join(cwd,'requests.jsonl'),'utf8')).trim().split('\n').map(line=>JSON.parse(line));}catch{return[];}};
  const waitCalls=async(count:number)=>{for(let i=0;i<500;i++){if((await records()).filter(r=>r.method==='tools/call').length>=count)return;await delay(10);}assert.fail('HTTP tool was not dispatched');};
  let gatewayCalls=0;
  const gateway=await server(async(req,res)=>{const value=JSON.parse((await body(req)).toString());gatewayCalls++;res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({tool_calls:[{index:0,id:'pending',type:'function',function:{name:value.tools[0].function.name,arguments:'{}'}}]},'tool_calls')+'data: [DONE]\n\n');});
  try{
    let endpoint='';for(let i=0;i<500;i++){try{endpoint=await readFile(join(cwd,'endpoint'),'utf8');if(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/.test(endpoint))break;}catch{}assert.equal(peer.exitCode,null);await delay(10);}assert(endpoint);
    const entry=join(cwd,'entry.toml');await writeFile(entry,config().split('[options.mcp.servers.local]')[0].replace('mcp.env','mcp.headers').replace('MCP_FIXTURE_TOKEN','MCP_HTTP_TOKEN')+'[options.mcp.servers.local]\ntransport="http"\nurl="https://synthetic.example/mcp"\nheaders={x-fixture-token="mcp"}\n');
    const args=['--config',entry,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url,'--fixture-mcp-endpoint',`local=${endpoint}`];const env={...cleanEnv(),MCP_HTTP_TOKEN:'synthetic-http-token'};
    const cli=spawn(binary,['run','test','--json',...args],{cwd,env});let stdout='';cli.stdout.on('data',b=>{stdout+=b.toString();});cli.stderr.resume();const cliExit=once(cli,'exit');
    try{await waitCalls(1);if(mode==='call_hang')cli.kill('SIGINT');const timer=setTimeout(()=>cli.kill('SIGKILL'),5000);try{await cliExit;}finally{clearTimeout(timer);}assert.notEqual(cli.signalCode,'SIGKILL');assert.equal(JSON.parse(stdout).outcome.status,mode==='call_hang'?'cancelled':'failed');}
    finally{if(cli.exitCode===null&&cli.signalCode===null){cli.kill('SIGKILL');await cliExit;}}
    const updates:any[]=[];
    await withPablo({binary,args,env,onUpdate:n=>{updates.push(n);}},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
      const prompt=cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'test'}]}).then(value=>outcomeOf(value),(error:any)=>{const outcome=error.data?.['pablo/v1']?.outcome;assert(outcome);return outcome;});
      await waitCalls(2);if(mode==='call_hang')await cx.notify('session/cancel',{sessionId});assert.equal((await prompt).status,mode==='call_hang'?'cancelled':'failed');
    });
    assert(updates.some(n=>n.update.rawOutput?.mcp?.remote_completion_uncertain===true));
    const all=await records();assert.equal(all.filter(r=>r.method==='tools/call').length,2);assert.equal(all.filter(r=>r.method==='notifications/cancelled').length,2);assert.equal(all.filter(r=>r.method==='DELETE').length,2);assert.equal(gatewayCalls,2);assert.equal(peer.exitCode,null,'client must not claim ownership of the remote process');
    const traces=(await Promise.all((await readdir(cwd)).filter(name=>/^[0-9a-f-]+\.jsonl$/.test(name)).map(name=>readFile(join(cwd,name),'utf8')))).join('');assert(traces.includes('"remote_completion_uncertain":true'));assert(!traces.includes('synthetic-http-token'));assert(!traces.includes('synthetic-session'));
  }finally{peer.kill('SIGTERM');const timer=setTimeout(()=>peer.kill('SIGKILL'),3000);try{await peerExit;}finally{clearTimeout(timer);}await gateway.close();await rm(cwd,{recursive:true,force:true});}
});
