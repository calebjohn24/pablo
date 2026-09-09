import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';
import { body, cleanEnv, server, traceId, parentId, traceparent } from './fixtures/telemetry.ts';
import { withPablo, outcomeOf } from '../examples/acp-client.ts';

const exec=promisify(execFile);
const binary=fileURLToPath(new URL('../target/debug/pablo',import.meta.url));
const frame=(delta:object,finish:string|null=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
const answer=(res:import('node:http').ServerResponse)=>{
  res.writeHead(200,{'content-type':'text/event-stream'});
  res.end(frame({content:'verified'})+frame({},'stop')+'data: [DONE]\n\n');
};
const preset=(endpoint:string,extra='')=>`schema_version=1
[deployment]
locked=true
allowed_run_overrides=["input"]
[credentials.gateway]
consumer="provider.vercel"
sources=[{kind="environment",name="PRIVATE_PROVIDER"}]
[credentials.collector]
consumer="otel.headers"
sources=[{kind="file",path={base="config",path="headers.token"},encoding="utf8"}]
[options.shell]
enabled=false
[options.filesystem]
enabled=false
[options.interfaces]
cli_output="json"
[options.trace]
path={base="workspace",path="{session_id}.jsonl"}
[options.otel]
exporter="otlp"
endpoint="${endpoint}"
headers="collector"
service_name="configured-service"
resource_attributes={"fixture.resource"="explicit-resource"}
sampler="always_on"
max_queue_size=32
max_export_batch_size=32
schedule_delay_ms=60000
${extra}`;
const ambient=(endpoint:string)=>({...cleanEnv(),
  PRIVATE_PROVIDER:'synthetic-provider-must-not-route',OTEL_TRACES_EXPORTER:'none',
  OTEL_SDK_DISABLED:'true',OTEL_TRACES_SAMPLER:'always_off',OTEL_TRACES_SAMPLER_ARG:'0',
  OTEL_EXPORTER_OTLP_ENDPOINT:endpoint,OTEL_EXPORTER_OTLP_TRACES_ENDPOINT:endpoint,
  OTEL_EXPORTER_OTLP_PROTOCOL:'grpc',OTEL_EXPORTER_OTLP_TRACES_PROTOCOL:'http/json',
  OTEL_EXPORTER_OTLP_HEADERS:'x-ambient=ambient-private',OTEL_EXPORTER_OTLP_TRACES_HEADERS:'x-traces=ambient-private',
  OTEL_EXPORTER_OTLP_COMPRESSION:'gzip',OTEL_EXPORTER_OTLP_TRACES_COMPRESSION:'gzip',
  OTEL_EXPORTER_OTLP_TIMEOUT:'1',OTEL_EXPORTER_OTLP_TRACES_TIMEOUT:'1',
  OTEL_BSP_MAX_QUEUE_SIZE:'1',OTEL_BSP_MAX_EXPORT_BATCH_SIZE:'1',OTEL_BSP_SCHEDULE_DELAY:'1',
  OTEL_SERVICE_NAME:'ambient-service',OTEL_RESOURCE_ATTRIBUTES:'fixture.resource=ambient-resource',
  OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT:'0',OTEL_ATTRIBUTE_COUNT_LIMIT:'0',OTEL_PROPAGATORS:'none'});

async function files(cwd:string) {
  const names=(await readdir(cwd)).filter(n=>n.endsWith('.jsonl'));
  return Promise.all(names.map(async n=>(await readFile(join(cwd,n),'utf8')).trim().split('\n').map(l=>JSON.parse(l))));
}

test('configured OTLP ignores ambient SDK/exporter values and sends only scoped explicit headers', {timeout:20000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-explicit-otel-')));
  const payloads:Buffer[]=[];let forbidden=0;let compression='none';
  const receiver=await server(async(req,res)=>{
    assert.equal(req.url,'/configured');
    assert.equal(req.headers['content-type'],'application/x-protobuf');
    assert.equal(req.headers['content-encoding'],compression==='gzip'?'gzip':undefined);
    assert.equal(req.headers.authorization,'Bearer synthetic collector');
    assert.equal(req.headers['x-once'],'%20');
    assert.equal(req.headers['x-ambient'],undefined);assert.equal(req.headers['x-traces'],undefined);
    const raw=await body(req);payloads.push(compression==='gzip'?gunzipSync(raw):raw);
    res.end();
  });
  const rejected=await server((_req,res)=>{forbidden++;res.end();});
  const gateway=await server(async(req,res)=>{
    assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');
    assert.equal(req.headers['x-once'],undefined);await body(req);answer(res);
  });
  try {
    await writeFile(join(cwd,'headers.token'),'authorization=Bearer%20synthetic%20collector,x-once=%2520\n');
    const file=join(cwd,'entry.toml');
    const args=['--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
    for(compression of ['none','gzip']) {
      await writeFile(file,preset(receiver.url+'/configured',`compression="${compression}"`));
      const result=await exec(binary,['run','private synthetic task',...args,'--traceparent',traceparent],{env:ambient(rejected.url+'/forbidden'),timeout:5000});
      assert.equal(JSON.parse(result.stdout).outcome.status,'completed');assert.equal(result.stderr,'');
    }
    assert.equal(payloads.length,2);assert.equal(forbidden,0);
    for(const payload of payloads){
      for(const expected of ['configured-service','explicit-resource','invoke_agent pablo','pablo.config.fingerprint'])assert(payload.includes(Buffer.from(expected)),expected);
      assert(payload.includes(Buffer.from(traceId,'hex')));assert(payload.includes(Buffer.from(parentId,'hex')));
      for(const secret of ['ambient-private','ambient-resource','ambient-service','synthetic collector','private synthetic task','synthetic-provider-must-not-route'])assert(!payload.includes(Buffer.from(secret)),secret);
    }
    for(const events of await files(cwd)) {
      assert.equal(events[0].trace_id,traceId);assert.equal(events[0].parent_span_id,parentId);
      assert.equal(events[0].trace_flags,'01');assert.equal(events.at(-1).outcome.status,'completed');
    }
  }finally{await gateway.close();await receiver.close();await rejected.close();await rm(cwd,{recursive:true,force:true});}
});

test('ACP refreshes exporter settings and privately rotated headers without changing portable identity', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-rotation-')));
  const received:{path:string,authorization:string|undefined,payload:Buffer}[]=[];
  const receiver=await server(async(req,res)=>{received.push({path:req.url!,authorization:req.headers.authorization,payload:await body(req)});res.end();});
  const gateway=await server(async(req,res)=>{assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');await body(req);answer(res);});
  let diagnostics='';const identities:string[]=[];
  try {
    const file=join(cwd,'entry.toml');const token=join(cwd,'headers.token');
    await writeFile(token,'authorization=first-private-key');await writeFile(file,preset(receiver.url+'/first'));
    const args=['--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
    await withPablo({binary,args,env:cleanEnv(),onDiagnostic:s=>{diagnostics+=s;}},async cx=>{
      await cx.request('initialize',{protocolVersion:1,clientCapabilities:{_meta:{'pablo/v1':true}}});
      for(let i=0;i<3;i++) {
        if(i===1)await writeFile(file,preset(receiver.url+'/second').replace('configured-service','second-service'));
        if(i===2)await writeFile(token,'authorization=second-private-key');
        const {sessionId}=await cx.request('session/new',{cwd,mcpServers:[]});
        const response=await cx.request('session/prompt',{sessionId,prompt:[{type:'text',text:'synthetic task'}]});
        assert.equal(outcomeOf(response).status,'completed');
        identities.push((response._meta!['pablo/v1'] as any).deployment.fingerprint);
      }
    });
    assert.equal(diagnostics,'pablo: ACP connection closed; owned work settled\n');assert.notEqual(identities[0],identities[1]);assert.equal(identities[1],identities[2]);
    assert.deepEqual(received.map(r=>[r.path,r.authorization]),[['/first','first-private-key'],['/second','first-private-key'],['/second','second-private-key']]);
    for(let i=0;i<3;i++) {
      assert(received[i].payload.includes(Buffer.from(identities[i])));
      assert(received[i].payload.includes(Buffer.from(i===0?'configured-service':'second-service')));
      for(const secret of ['first-private-key','second-private-key'])assert(!received[i].payload.includes(Buffer.from(secret)));
    }
    const traces=await files(cwd);assert.equal(traces.length,3);
    for(const secret of ['first-private-key','second-private-key'])assert(!JSON.stringify(traces).includes(secret));
  }finally{await gateway.close();await receiver.close();await rm(cwd,{recursive:true,force:true});}
});

test('configured sampler, propagation and disabled SDK retain native outcomes and suppress exports', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-config-sampling-')));
  let received=0;const receiver=await server(async(req,res)=>{await body(req);received++;res.end();});
  const gateway=await server(async(req,res)=>{await body(req);answer(res);});
  try {
    const file=join(cwd,'entry.toml');await writeFile(join(cwd,'headers.token'),'authorization=synthetic');
    for(const mode of ['off','ratio-zero','disabled','no-propagation']) {
      let config=preset(receiver.url+'/traces');
      if(mode==='off')config=config.replace('sampler="always_on"','sampler="always_off"');
      if(mode==='ratio-zero')config=config.replace('sampler="always_on"','sampler="traceidratio"\nsampler_arg="0"');
      if(mode==='disabled')config+='\nsdk_disabled=true';
      if(mode==='no-propagation')config+='\npropagators=[]';
      await writeFile(file,config);
      await exec(binary,['run','synthetic','--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions','--traceparent',traceparent],{env:cleanEnv(),timeout:5000});
    }
    const traces=await files(cwd);assert.equal(traces.length,4);assert.equal(received,1);
    assert.equal(traces.filter(e=>e[0].trace_flags==='00').length,3);
    assert.equal(traces.filter(e=>e[0].trace_id===traceId).length,3);
    assert(traces.every(e=>e.at(-1).outcome.status==='completed'));
  }finally{await gateway.close();await receiver.close();await rm(cwd,{recursive:true,force:true});}
});

test('configured exporter errors respect explicit timeout and never redirect private headers', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-config-export-errors-')));
  let mode='redirect';let requests=0;let redirected=0;
  const forbidden=await server((_req,res)=>{redirected++;res.end();});
  const receiver=await server(async(req,res)=>{
    await body(req);requests++;
    if(mode==='redirect'){res.writeHead(307,{location:forbidden.url+'/private'});res.end();}
    else {res.writeHead(200);res.flushHeaders();}
  });
  const gateway=await server(async(req,res)=>{await body(req);answer(res);});
  try {
    const file=join(cwd,'entry.toml');await writeFile(join(cwd,'headers.token'),'authorization=private-synthetic-header');
    await writeFile(file,preset(receiver.url+'/traces','timeout_ms=40\nexport_timeout_ms=30'));
    const args=['run','synthetic','--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'];
    for(mode of ['redirect','stall']) {
      const start=performance.now();const result=await exec(binary,args,{env:{...cleanEnv(),OTEL_EXPORTER_OTLP_TIMEOUT:'60000'},timeout:5000});
      assert.equal(JSON.parse(result.stdout).outcome.status,'completed');
      assert.match(result.stderr,/telemetry dropped/);assert(!result.stderr.includes('private-synthetic-header'));
      assert(performance.now()-start<1500);
    }
    assert.equal(requests,2);assert.equal(redirected,0);
    for(const invalid of ['authorization=private%0Aheader','authorization=one,Authorization=two','host=private-host','authorization=bad%zz']) {
      await writeFile(join(cwd,'headers.token'),invalid);
      await assert.rejects(exec(binary,args,{env:cleanEnv(),timeout:5000}),(error:any)=>error.code===2&&error.stdout===''&&error.stderr.includes('config_')&&!error.stderr.includes(invalid));
    }
    assert.equal(requests,2);assert.equal((await files(cwd)).length,2);
  }finally{await gateway.close();await receiver.close();await forbidden.close();await rm(cwd,{recursive:true,force:true});}
});

test('configured credentials stay out of shell environment, native events and exported spans', {timeout:15000},async()=>{
  const cwd=await realpath(await mkdtemp(join(tmpdir(),'pablo-config-private-shell-')));
  const payloads:Buffer[]=[];const secrets=['synthetic-provider-must-not-route','synthetic-exporter-secret'];
  const receiver=await server(async(req,res)=>{assert.equal(req.headers.authorization,secrets[1]);payloads.push(await body(req));res.end();});
  let calls=0;const gateway=await server(async(req,res)=>{
    assert.equal(req.headers.authorization,'Bearer pablo-local-fixture');const request=JSON.parse((await body(req)).toString());calls++;
    for(const secret of secrets)assert(!JSON.stringify(request).includes(secret));
    if(request.messages.at(-1).role==='tool') {
      const result=JSON.parse(request.messages.at(-1).content);assert.equal(result.shell.exit_code,0);
      assert(!result.shell.stdout.includes('PRIVATE_PROVIDER'));assert(!result.shell.stdout.includes('OTEL_'));answer(res);
    }else{res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({tool_calls:[{index:0,id:'env_call',type:'function',function:{name:'shell_run',arguments:'{"command":"env","cwd":"."}'}}]},'tool_calls')+'data: [DONE]\n\n');}
  });
  try{
    const file=join(cwd,'entry.toml');await writeFile(join(cwd,'headers.token'),'authorization='+secrets[1]);
    await writeFile(file,preset(receiver.url+'/traces').replace('[options.shell]\nenabled=false','[options.shell]\nenabled=true'));
    const result=await exec(binary,['run','synthetic task','--config',file,'--bind',`workspace=${cwd}`,'--fixture-endpoint',gateway.url+'/v1/chat/completions'],{env:ambient(receiver.url+'/ambient'),timeout:5000});
    assert.equal(JSON.parse(result.stdout).outcome.status,'completed');assert.equal(calls,2);assert.equal(payloads.length,1);
    const native=JSON.stringify(await files(cwd));
    for(const secret of secrets){assert(!native.includes(secret));assert(!result.stdout.includes(secret));assert(!result.stderr.includes(secret));assert(!payloads[0].includes(Buffer.from(secret)));}
  }finally{await gateway.close();await receiver.close();await rm(cwd,{recursive:true,force:true});}
});
