/** Real OMP provider failures crossing the production daemon; all responses are synthetic. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';
const live=JSON.parse(readFileSync(process.argv[2],'utf8'));
test(`production daemon handles actual OMP provider failure: ${live.failureMode}`,{timeout:150000},async t=>{
 const f=await workflowFixture(t,'research',1,false,{parent:live.root,service:true,configure({config,roles}){
  for(const role of roles){const selected=role.id==='tech-writer'?live.models.specialist:live.models.coordinator;role.model=selected.selector;role.effort=selected.effort;}
  writeFileSync(config.roles,JSON.stringify(roles),{mode:0o600});Object.assign(config.runtime,live.runtime);config.runtime.mail.projectRoot=live.root+'/mail-projects';
 }});
 await f.start().catch(()=>{});
 if(live.failureMode==='fallback'){
  const recovered=await until(async()=>{const r=await f.snapshot();const coordinator=r.assignments.find((a:any)=>a.coordinator);const current=r.attempts.find((a:any)=>a.id===coordinator?.currentAttemptId);return current?.fallback&&current.state==='idle'&&!r.busy?r:false;},45000);
  const coordinator=recovered.assignments.find((a:any)=>a.coordinator),attempt=recovered.attempts.find((a:any)=>a.id===coordinator.currentAttemptId);
  assert.equal(attempt.fallback.fromModel,live.models.coordinator.selector);assert.equal(attempt.model.resolved.selector,live.models.specialist.selector);assert.equal(coordinator.automaticCycles,1);
  assert.equal(recovered.assignments.find((a:any)=>!a.coordinator).automaticCycles??0,0);
  const before=recovered.attempts.map((a:any)=>[a.id,a.ompPid,a.ownership]);await f.kill();await f.reopen();await f.idle();assert.deepEqual((await f.snapshot()).attempts.map((a:any)=>[a.id,a.ompPid,a.ownership]),before);
 }else{
  const held=await until(async()=>{const r=await f.snapshot();return r.run.status==='attention_required'&&r.attempts.every((a:any)=>a.ownership==='stopped')?r:false;},45000);
  assert.match(held.run.attentionReason,live.failureMode==='nonretryable'?/not retryable|operator attention/:/No compatible configured runtime fallback/);
  assert.ok(held.attempts.every((a:any)=>!a.fallback));assert.ok(held.assignments.every((a:any)=>(a.automaticCycles??0)===0));
 }
 t.diagnostic(JSON.stringify({failureMode:live.failureMode,attempts:(await f.snapshot()).attempts.length,run:(await f.snapshot()).run.status}));
});
