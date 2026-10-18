import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {alter,fixture} from './target-fixture';

// Schema 26 is already current, so the runtime must audit these old records;
// opening the database cannot invent a missing candidate or path identity.
for(const damage of ['candidate-split','missing-source','noncanonical-path','legacy-ownership']) {
 test(`schema 26 ${damage} remains recoverable and refuses conflict access`,{timeout:60000},async t=>{
  const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');
  const refresh=(await f.refresh(candidate)).refresh;const path=refresh.resolutionWorkspace.path;
  const content=readFileSync(join(path,'source.txt'),'utf8');const originalHead=f.gitAt(work.path,'rev-parse','HEAD');
  await f.c.close();
  alter(f,'repository_work',f.seed.run.id,value=>{
   const r=value.refreshes.at(-1);
   if(damage==='candidate-split')r.sourceCandidateId=randomUUID();
   if(damage==='missing-source')delete r.sourceCandidateId;
   if(damage==='noncanonical-path')r.resolutionWorkspace.path=path+'/../'+path.split('/').at(-1);
   if(damage==='legacy-ownership')r.resolutionWorkspace.legacyOwnershipUnknown=true;
  });
  await f.reopen();
  await assert.rejects(f.c.command('validate_conflict',{target:f.seed.run.id}),/changed|unverified/);
  await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'source.txt'}),/changed|unverified/);
  const saved=(await f.record()).refreshes.at(-1);
  assert.equal(saved.requestId,refresh.requestId);assert.equal(saved.fromHead,refresh.fromHead);
  assert.equal(readFileSync(join(path,'source.txt'),'utf8'),content);
  assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),originalHead);
  assert.equal(f.gitAt(work.path,'status','--porcelain'),'');
  const coding=await f.c.command('record',{target:f.seed.run.id});
  assert.ok(coding.candidates.some((value:any)=>value.id===candidate.id));
 });
}
