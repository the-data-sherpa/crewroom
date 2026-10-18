import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync,writeFileSync,mkdirSync,existsSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';

test('native workflow admits real independent hosts, delivers Mail and steering, then resumes with fresh current identities',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();let runtime=await f.idle();
 const coordinator=runtime.assignments.find((a:any)=>a.coordinator),specialist=runtime.assignments.find((a:any)=>!a.coordinator);
 assert.equal(runtime.attempts.length,2);assert.equal(new Set(runtime.attempts.map((a:any)=>a.hostPid)).size,2);assert.ok(runtime.attempts.every((a:any)=>a.ompPid&&a.ompPid!==a.hostPid));
 await f.input(coordinator.currentAttemptId,{kind:'tool',name:'send_message',toolCallId:'send-once',arguments:{to:specialist.name,body:'Verify the criterion.'}},'delegate');
 await until(async()=>{const v=await f.snapshot();return v.messages[0]?.disposition==='received';});assert.equal(f.sent.size,1);
 const steering=await f.c.command('steer',{target:f.run.id,input:{requestId:randomUUID(),body:'Preserve the research-only outcome.'}});
 await until(async()=>{const v=await f.c.command('steering',{target:f.run.id});return v.find((s:any)=>s.id===steering.id)?.delivery==='received';});
 const stopped=await f.pause();assert.equal(stopped.status,'paused');runtime=await f.snapshot();assert.ok(runtime.attempts.every((a:any)=>a.ownership==='stopped'));
 await assert.rejects(f.tool(coordinator.currentAttemptId,'agent-message',{to:specialist.name,body:'After pause'}),/not authorized|no longer/);
 await f.resume();runtime=await f.idle();assert.equal(runtime.attempts.length,4);assert.ok(runtime.assignments.every((a:any)=>a.automaticCycles===0));
 const newCoordinator=runtime.assignments.find((a:any)=>a.coordinator);assert.notEqual(newCoordinator.currentAttemptId,coordinator.currentAttemptId);
 assert.equal(f.host(newCoordinator.currentAttemptId).runnerCwd,f.host(coordinator.currentAttemptId).runnerCwd);
 const record=readFileSync(join(f.runtime,newCoordinator.currentAttemptId,'rpc-requests.jsonl'),'utf8').trim().split('\n').map(s=>JSON.parse(s));
 const continuation=JSON.parse(record.find((r:any)=>r.type==='prompt').message);assert.ok(continuation.steering.some((s:any)=>s.id===steering.id));
 assert.equal((await f.c.command('cancel',{target:f.run.id})).status,'cancelled');await assert.rejects(f.resume(),/Pause|available|cancel/);
});

test('native workflow fences queued launch on immediate Pause',{timeout:90000},async t=>{
 const f=await workflowFixture(t);const gate=join(f.root,'lane');mkdirSync(gate);
 const hold=f.c.command('operator_hold',{target:f.run.id,root:gate});await until(()=>existsSync(join(gate,'waiting')));
 const start=f.start();const rejected=assert.rejects(start,/control request|stopped/);
 const pause=f.pause();await until(async()=>(await f.c.command('run',{target:f.run.id})).desiredState==='paused');
 writeFileSync(join(gate,'release'),'release');await hold;await rejected;assert.equal((await pause).status,'paused');
 assert.equal((await f.snapshot()).attempts.length,0);assert.equal(f.requests.filter(r=>r.name==='register_agent').length,0);
});

test('native restart reattaches live hosts and preserves paused history',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();const before=await f.idle();await f.kill();await f.reopen();
 const after=await until(async()=>{const v=await f.snapshot();return v.observedAttemptIds.length===2?v:false;});
 assert.deepEqual(after.assignments.map((a:any)=>a.currentAttemptId),before.assignments.map((a:any)=>a.currentAttemptId));
 assert.deepEqual(after.attempts.map((a:any)=>a.ompPid),before.attempts.map((a:any)=>a.ompPid));
 assert.equal((await f.c.command('run',{target:f.run.id})).status,'running');await f.pause();
 await f.kill();await f.reopen();assert.equal((await f.c.command('run',{target:f.run.id})).status,'paused');
});
