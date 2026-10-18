/** MH5: production daemon and processes; scripted inference/Mail, no hosted writes. */
import assert from 'node:assert/strict';
import test from 'node:test';
import {randomUUID} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync,symlinkSync} from 'node:fs';
import {join} from 'node:path';
import {setTimeout as sleep} from 'node:timers/promises';
import {NativeDaemon} from './daemon-client';
import {workflowFixture,until} from './workflow-support';

export function planContent(criteria=[0]) {
 return {schemaVersion:1,goal:'Implement the chosen behavior',scope:['A bounded change'],exclusions:['Unrelated work'],assumptions:[],decisions:['Preserve prior behavior'],workPackages:[{id:'change',name:'Implement and verify',dependsOn:[],outputs:['Patch and evidence'],criteria,validation:['Run the fixture checks and independent review']}]};
}
test('intake stops for decisions, survives restart and hands off exactly once under queue authority',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'explore',0,true,{service:true,configure({source}){
  writeFileSync(join(source,'binary.bin'),Buffer.from([0,1,2]));writeFileSync(join(source,'large.txt'),'x'.repeat(1024*1024+1));symlinkSync('/etc/passwd',join(source,'link'));
  execFileSync('git',['-C',source,'add','.']);execFileSync('git',['-C',source,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','Read boundary fixtures'],{stdio:'ignore'});
 }});
 const client=()=>f.c as NativeDaemon;
 const task=await client().request('/api/tasks',{title:'Unplanned idea',description:'Decide the output',repositoryId:f.repository.id});
 const payload={requestId:randomUUID(),revision:1};
 const run=await client().request(`/api/tasks/${task.id}/runs`,payload);
 assert.equal(run.purpose,'grill');assert.deepEqual(await client().request(`/api/tasks/${task.id}/runs`,payload),run);
 const snapshot=()=>client().request(`/api/runs/${run.id}/runtime`);
 const view=async()=>(await client().request('/api/workspace')).intake[task.id];
 const idle=()=>until(async()=>{const s=await snapshot();return !s.busy&&s.run.status==='running'&&s.assignments.length===1&&s.attempts.find((a:any)=>a.id===s.assignments[0].currentAttemptId)?.state==='idle'?s:false;},30000);
 let runtime=await idle();const original=runtime.assignments[0];let attempt=original.currentAttemptId;
 assert.equal(runtime.run.purpose,'grill');assert.equal((await client().request('/api/workspace')).queue.slot??null,null);
 for(const path of ['../etc/passwd','link','binary.bin','large.txt'])await assert.rejects(f.tool(attempt,'intake/read',{path}),/path|regular|text|binary|1 MB|Unsupported/i);
 await assert.rejects(f.tool(attempt,'coding/edit',{path:'source.txt',content:'no'}),/authority|coding|available|authoriz|allowed|registered/i);
 const sourceId=run.intakeSource.id;
 await assert.rejects(f.tool(attempt,'intake/questions',{sourceId,questions:[{id:'output',prompt:'Which output?',rationale:'Defines the change',choices:['Text','JSON']}]}),/Inspect/);
 const read=await f.tool(attempt,'intake/read',{path:'source.txt'});assert.equal(read.content,'before\n');
 const question={sourceId,questions:[{id:'output',prompt:'Which output?',rationale:'Defines the change',choices:['Text','JSON']}]};
 const asked=await f.tool(attempt,'intake/questions',question,'ask-once');
 await until(async()=>{const v=await view();return v.state==='Waiting for you'&& (await snapshot()).attempts.every((a:any)=>a.ownership==='stopped');});
 const waiting=await snapshot();assert.equal(waiting.run.pauseReason,'intake');
 // Keep the real daemon/API available without a runner so the answer can be
 // durably recorded at a deterministic boundary before any delivery/resume.
 const configuredRuntime=f.config.runtime;
 await f.kill();delete f.config.runtime;await f.reopen();await sleep(300);
 assert.equal((await view()).state,'Waiting for you');assert.equal((await snapshot()).run.activeMs,waiting.run.activeMs);
 const answer={requestId:randomUUID(),revision:(await view()).intake.revision,sourceId,answers:[{id:'output',body:'Text'}]};
 const answered=await client().request(`/api/intakes/${asked.id}/answers`,answer);
 assert.equal(answered.state,'answer_pending');
 const pendingMessages=async()=>(await client().request('/api/workspace')).messages.filter((m:any)=>m.id===answered.pendingMessageId);
 assert.equal((await pendingMessages()).length,1);assert.equal((await pendingMessages())[0].delivery,'pending');
 await f.kill();await f.reopen();
 assert.deepEqual(await client().request(`/api/intakes/${asked.id}/answers`,answer),answered);
 assert.deepEqual((await view()).intake,answered);
 assert.equal((await pendingMessages()).length,1);assert.equal((await pendingMessages())[0].delivery,'pending');
 const pendingRuntime=await snapshot();
 assert.equal(pendingRuntime.attempts.length,1);assert.equal(pendingRuntime.run.activeMs,waiting.run.activeMs);
 assert.ok(pendingRuntime.attempts.every((a:any)=>a.ownership==='stopped'));
 await f.kill();f.config.runtime=configuredRuntime;await f.reopen();
 runtime=await idle();attempt=runtime.assignments[0].currentAttemptId;
 assert.equal(runtime.assignments[0].id,original.id);assert.notEqual(attempt,original.currentAttemptId);
 await until(async()=>(await client().request('/api/workspace')).messages.some((m:any)=>m.id===answered.pendingMessageId&&m.delivery==='received'));
 await sleep(500);
 assert.deepEqual(await client().request(`/api/intakes/${asked.id}/answers`,answer),answered);
 assert.equal((await pendingMessages()).length,1);assert.equal((await snapshot()).attempts.length,2);
 const delivered=(await pendingMessages())[0];
 assert.deepEqual(delivered.transitions.map((v:any)=>v.state),['dispatched','received']);
 const answerInputs=runtime.attempts.flatMap((a:any)=>readFileSync(join(f.runtime,a.id,'rpc-requests.jsonl'),'utf8').trim().split('\n').map(line=>JSON.parse(line))).filter((v:any)=>v.message?.includes(`Operator answers for intake ${asked.id}`));
 assert.equal(answerInputs.length,1);assert.equal(answerInputs[0].id,delivered.inputId);
 await f.tool(attempt,'intake/read',{path:'source.txt'});
 const proposal=await f.tool(attempt,'intake/plan',{sourceId,content:planContent(),requirements:{brief:'Return text',criteria:['Text output is checked']}});
 await until(async()=>(await view()).state==='Plan review');
 const decision={requestId:randomUUID(),revision:1,sourceId,decision:'accept'};
 const accepted=await client().request(`/api/implementation-plans/${proposal.id}/decision`,decision);
 assert.deepEqual(await client().request(`/api/implementation-plans/${proposal.id}/decision`,decision),accepted);
 await f.kill();await f.reopen();
 const handed=await until(async()=>{const w=await client().request('/api/workspace');const jobs=w.runs.filter((r:any)=>r.taskId===task.id&&r.purpose==='coding');return jobs.length===1?{w,job:jobs[0]}:false;});
 assert.equal(handed.job.status,'awaiting_runner');assert.equal(handed.job.implementationPlan.id,proposal.id);assert.equal(handed.job.taskRevision,2);
 assert.equal((await snapshot()).run.status,'closed');assert.equal((await client().request(`/api/runs/${handed.job.id}/runtime`)).attempts.length,0);
 assert.equal(handed.w.tasks.find((v:any)=>v.id===task.id).brief,'Return text');
 await client().request(`/api/queue/${handed.job.id}/remove`,{requestId:randomUUID()});
 const reused=await client().request(`/api/tasks/${task.id}/runs`,{requestId:randomUUID(),revision:2});assert.equal(reused.purpose,'coding');assert.equal(reused.implementationPlan.id,proposal.id);
 assert.equal((await client().request('/api/workspace')).runs.filter((r:any)=>r.taskId===task.id&&r.purpose==='grill').length,1);
 t.diagnostic(JSON.stringify({intakeId:asked.id,planId:proposal.id,sourceId,requirementsRevision:2,inspectionCommit:read.commit,waitActiveMs:waiting.run.activeMs,pendingAnswerRestart:{messageId:answered.pendingMessageId,runnerInputs:answerInputs.length,resumedAttempts:runtime.attempts.length-1,waitActiveMsAfterRestart:pendingRuntime.run.activeMs},scriptedInference:true,scriptedMail:true}));
});

test('accepted plan reaches real Git checks and independent review through the production daemon',{timeout:120000},async t=>{
 const {reviewedCandidate}=await import('./workflow-coding-fixture');
 const f=await workflowFixture(t,'coding',2,false,{service:true});
 const {candidate}=await reviewedCandidate(f);
 const runtime=await f.snapshot();
 assert.ok(runtime.run.implementationPlan.id);
 assert.ok(runtime.assignments.every((a:any)=>a.implementationPlan.id===runtime.run.implementationPlan.id));
 assert.equal((await f.c.command('evidence',{target:f.run.id})).ready,true);
 await f.pause();
 t.diagnostic(JSON.stringify({codingRunId:f.run.id,planId:runtime.run.implementationPlan.id,candidateId:candidate.id,revision:candidate.revision,actualGitAndDocker:true,independentReview:true,scriptedInference:true,publication:'Not requested'}));
});
