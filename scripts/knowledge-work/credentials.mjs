/** Explicit benchmark-only credential reuse. Never returns file contents or secret diagnostics. */
import {open} from 'node:fs/promises';
import {parseEnv} from 'node:util';
export async function privateKey(path, name, environment={}) {
  if(!['OPENROUTER_API_KEY','OPENAI_API_KEY'].includes(name))throw Error('Unsupported credential name');
  if(environment[name])return environment[name];
  let file;
  const buffer=Buffer.alloc(65537);
  try {
    file=await open(path,'r');
    const {bytesRead}=await file.read(buffer,0,buffer.length,0);
    if(bytesRead>65536)throw Error();
    const source=buffer.subarray(0,bytesRead).toString('utf8');
    if((source.match(new RegExp('^\\s*(?:export\\s+)?'+name+'\\s*=', 'gm'))??[]).length!==1)throw Error();
    const key=parseEnv(source)[name];
    if(!key||/[\s$\x00-\x1f]/.test(key))throw Error();
    return key;
  } catch {throw Error('Cannot privately resolve the selected provider credential');}
  finally {buffer.fill(0);await file?.close();}
}

export const openRouterKey=(path,environment=process.env)=>privateKey(path,'OPENROUTER_API_KEY',environment);
