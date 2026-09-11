/** Fixture peer only. Rust drives Runtime directly; no root ACP transport here. */
import assert from 'node:assert/strict';
import {createInterface} from 'node:readline';
import {extensibilityFixture,type E01Mode} from './extensibility.ts';
const mode=process.argv[2] as E01Mode;assert(['complete','cancel','denied'].includes(mode));
const fixture=await extensibilityFixture(mode);
const lines=createInterface({input:process.stdin,crlfDelay:Infinity});
try{
  fixture.setCancel(async()=>{process.stdout.write(JSON.stringify({cancel:true})+'\n');});
  process.stdout.write(JSON.stringify({cwd:fixture.cwd,args:fixture.args})+'\n');
  for await(const line of lines){
    const {task}=JSON.parse(line);
    const proof=await fixture.verify(task);
    process.stdout.write(JSON.stringify({verified:true,counts:proof.counts})+'\n');
    break;
  }
}finally{lines.close();await fixture.close();}
