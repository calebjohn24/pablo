import {test} from 'node:test';
import {extensibilityFixture,type E01Mode} from './fixtures/extensibility.ts';
for(const mode of ['complete','cancel','denied'] as E01Mode[])test(`E01 ACP combines MCP, Skill, children, handoff and compaction: ${mode}`,{timeout:20000},async()=>{
  const fixture=await extensibilityFixture(mode);
  try{await fixture.verify(await fixture.run());}finally{await fixture.close();}
});
