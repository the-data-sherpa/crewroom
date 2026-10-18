import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync,readFileSync,existsSync,rmSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';
import {reviewedCandidate} from './workflow-coding-fixture';

test('agent tool authorization enforces bearer, current attempt, coordinator, purpose and available services',{timeout:60000},async t=>{
 const f=await workflowFixture(t);await f.start();const runtime=await f.idle();const coordinator=runtime.assignments[0].currentAttemptId,specialist=runtime.assignments[1].currentAttemptId;
 await assert.rejects(f.c.command('tool',{route:'agent-message',authorization:'Bearer invalid',input:{attemptId:coordinator,toolCallId:'bad-token',arguments:{to:'Specialist1',body:'No'}}}),/not authorized/);
 await assert.rejects(f.c.command('tool',{route:'agent-message',authorization:`Bearer ${f.host(coordinator).token}`,input:{attemptId:specialist,toolCallId:'cross-token',arguments:{to:'Coordinator',body:'No'}}}),/not authorized/);
 for(const route of ['coding/read','publication/prepare','jira/report','refinement-proposal','unknown'])await assert.rejects(f.tool(coordinator,route,{}),/not registered/);
 for(const route of ['team-proposal','research/complete','agent-roles','learning-proposal'])await assert.rejects(f.tool(specialist,route,{}),/not registered/);
 await assert.rejects(f.tool(coordinator,'agent-message',{to:'Specialist1',body:'No',unknown:true}),/Unexpected/);
 assert.equal((await f.snapshot()).messages.length,0);await f.pause();await f.resume();await f.idle();await assert.rejects(f.tool(coordinator,'agent-message',{to:'Specialist1',body:'Stale'}),/not authorized|no longer/);
});

test('pinned research verifies source citations then stops every host before completing and preserves completion on restart and close',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'research',1,true);await f.start();const runtime=await f.idle();const coordinator=runtime.assignments[0].currentAttemptId,specialist=runtime.assignments[1].currentAttemptId;
 const source=await f.tool(specialist,'research/read',{path:'source.txt'});assert.equal(source.content,'before\n');
 const input={title:'Source assessment',kind:'research',body:'# Findings\nThe pinned source says before.',citations:[{path:'source.txt',commit:f.run.researchSource.commit,startLine:1,endLine:1,excerpt:'before'}],limitations:'Only the disposable source was inspected.',coverage:[{criterionId:0,section:'Findings'}]};
 const artifact=await f.tool(specialist,'research/submit',input,'research');assert.equal(artifact.state,'verified');assert.deepEqual(await f.tool(specialist,'research/submit',input,'research'),artifact);
 await f.tool(coordinator,'research/complete',{},'complete');await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='completed');assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));assert.equal(await f.c.command('record',{target:f.run.id}),null);
 const completed=await f.c.command('run',{target:f.run.id});await f.kill();await f.reopen();assert.deepEqual(await f.c.command('run',{target:f.run.id}),completed);
 const gate=join(f.root,'completed-lane');mkdirSync(gate);const held=f.c.command('operator_hold',{target:f.run.id,root:gate});await until(()=>existsSync(join(gate,'waiting')));const closing=f.c.command('close_runtime');writeFileSync(join(gate,'release'),'release');await held;assert.equal((await closing).stopped,true);assert.deepEqual(await f.c.command('run',{target:f.run.id}),completed);
});

test('Explore and Grill run one coordinator and save proposals independently of accepted requirements',{timeout:90000},async t=>{
 for(const purpose of ['explore','grill'] as const){
  const f=await workflowFixture(t,purpose,0);const before=await f.c.command('task',{target:f.task.id});await f.c.command('refine',{target:f.run.id});const runtime=await f.idle();assert.equal(runtime.assignments.length,1);assert.equal(runtime.assignments[0].coordinator,true);
  const attempt=runtime.assignments[0].currentAttemptId;const input={brief:'Proposed improvement',criteria:['Proposed criterion'],decisions:['Keep the source immutable'],unresolvedQuestions:['What is the operator preference?']};const proposal=await f.tool(attempt,'refinement-proposal',input,'propose');assert.equal(proposal.status,'pending');assert.deepEqual(await f.tool(attempt,'refinement-proposal',input,'propose'),proposal);assert.deepEqual(await f.c.command('task',{target:f.task.id}),before);
  await assert.rejects(f.tool(attempt,'research/submit',{}),/not registered/);await assert.rejects(f.tool(attempt,'coding/edit',{}),/not registered/);await f.pause();
 }
});

test('team insertion waits for current work, replaces identities once and retires the selected specialist',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();const original=await f.idle();const coordinator=original.assignments[0],specialist=original.assignments[1];
 const busy=join(f.runtime,coordinator.currentAttemptId,'busy');writeFileSync(busy,'busy');
 const input={specialists:[{name:'NewResearcher',roleId:'tech-writer',input:'Check the criterion independently.',criteria:[{index:0,obligation:'Verify',checkIds:[]}],dependsOn:[]}],retire:[{name:specialist.name,reason:'Replace this specialty'}]};
 const proposed=await f.tool(coordinator.currentAttemptId,'team-proposal',input,'insert');assert.equal(proposed.status,'pending');assert.deepEqual(await f.tool(coordinator.currentAttemptId,'team-proposal',input,'insert'),proposed);
 await assert.rejects(f.tool(coordinator.currentAttemptId,'team-proposal',{...input,retire:[]},'insert'),/different content/);assert.deepEqual((await f.snapshot()).assignments,original.assignments);
 rmSync(busy);await f.c.command('notify',{target:f.run.id});const runtime=await until(async()=>{const r=await f.snapshot();return !(await f.c.command('busy',{target:f.run.id}))&&r.assignments.some((a:any)=>a.name==='NewResearcher')&&r.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;});
 assert.ok(runtime.assignments.find((a:any)=>a.id===specialist.id).retiredAt);assert.notEqual(runtime.assignments.find((a:any)=>a.coordinator).currentAttemptId,coordinator.currentAttemptId);assert.equal(runtime.teamProposals.find((p:any)=>p.id===proposed.id).status,'applied');await assert.rejects(f.tool(specialist.currentAttemptId,'research/submit',{}),/not authorized|no longer/);
});

test('coding handoff creates an exact candidate and actual Docker checks plus independent review gate readiness',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'coding',2);
 await reviewedCandidate(f);
 assert.equal((await f.c.command('record',{target:f.run.id})).candidates.length,1);await f.pause();await f.kill();await f.reopen();assert.equal((await f.c.command('evidence',{target:f.run.id})).ready,true);
});

test('an unavailable inserted role is rejected before stopping the existing team',{timeout:60000},async t=>{
 const f=await workflowFixture(t);await f.start();const original=await f.idle();const busy=join(f.runtime,original.assignments[0].currentAttemptId,'busy');writeFileSync(busy,'busy');
 const input={requestId:crypto.randomUUID(),roleId:'tech-writer',input:'Inspect the criterion',criteria:[0],dependsOn:[]};const proposal=await f.c.command('insert',{target:f.run.id,input});assert.deepEqual(await f.c.command('insert',{target:f.run.id,input}),proposal);
 writeFileSync(f.config.roles,JSON.stringify(f.roles.map(r=>r.id==='tech-writer'?{...r,enabled:false}:r)));await f.c.command('notify',{target:f.run.id});await until(async()=>(await f.snapshot()).teamProposals.some((p:any)=>p.id===proposal.id&&p.status==='rejected'));rmSync(busy);await f.idle();
 const after=await f.snapshot();assert.equal(after.attempts.length,2);assert.deepEqual(after.attempts.map((a:any)=>a.hostPid),original.attempts.map((a:any)=>a.hostPid));assert.equal((await f.c.command('run',{target:f.run.id})).status,'running');assert.equal(after.assignments.length,2);
});
