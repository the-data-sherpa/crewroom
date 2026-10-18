import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {mkdirSync,writeFileSync,existsSync,readFileSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';

test('cancelled start and Pause callers cannot abandon accepted work or ownership cleanup',{timeout:90000},async t=>{
 const f=await workflowFixture(t);f.holdMail();const id=randomUUID();
 const started=f.c.command('start',{target:f.run.id,input:{team:f.team},roles:f.roles},id);const cancelled=assert.rejects(started,/waiter cancelled/);
 await until(()=>f.requests.length>0);await f.c.command('cancel_waiter',{requestId:id});await cancelled;f.releaseMail();
 await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='running');await f.idle();
 const gate=join(f.root,'pause-lane');mkdirSync(gate);const held=f.c.command('operator_hold',{target:f.run.id,root:gate});await until(()=>existsSync(join(gate,'waiting')));
 const stopId=randomUUID();const stopped=f.c.command('pause',{target:f.run.id},stopId);const stopCancelled=assert.rejects(stopped,/waiter cancelled/);
 await until(async()=>(await f.c.command('run',{target:f.run.id})).desiredState==='paused');await f.c.command('cancel_waiter',{requestId:stopId});await stopCancelled;
 writeFileSync(join(gate,'release'),'release');await held;await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='paused');
 assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));
});

test('cancelled close drains initialization and workflow work and rejects a queued backend mutation',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();const runtime=await f.idle();const coordinator=runtime.assignments[0];
 const gate=join(f.root,'close-lane');mkdirSync(gate);const held=f.c.command('operator_hold',{target:f.run.id,root:gate});await until(()=>existsSync(join(gate,'waiting')));
 const late=f.tool(coordinator.currentAttemptId,'agent-message',{to:'Specialist1',body:'Too late'});const lateRejected=assert.rejects(late,/stopping|no longer|authorized/);
 const initializing=f.c.command('initialize');const initResult=initializing.then(value=>({value}),error=>({error}));
 const id=randomUUID();const close=f.c.command('close_runtime',{},id);const closeCancelled=assert.rejects(close,/waiter cancelled/);
 await until(async()=>(await f.c.command('run',{target:f.run.id})).desiredState==='paused');await f.c.command('cancel_waiter',{requestId:id});await closeCancelled;
 assert.equal(await f.c.command('busy',{target:f.run.id}),true);await assert.rejects(f.resume(),/stopping/);
 writeFileSync(join(gate,'release'),'release');await held;await lateRejected;await initResult;
 const report=await f.c.command('close_runtime');assert.deepEqual(report,{stopped:true,errors:[]});assert.equal(await f.c.command('busy',{target:f.run.id}),false);
 assert.equal((await f.snapshot()).messages.length,0);assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));
});

test('unavailable Mail holds launch with no executing agents and initialization preserves unstarted plans',{timeout:60000},async t=>{
 const f=await workflowFixture(t);const initial=await f.c.command('run',{target:f.run.id});await f.c.command('initialize');
 assert.deepEqual(await f.c.command('run',{target:f.run.id}),initial);assert.equal((await f.snapshot()).attempts.length,0);
 f.offline(true);await assert.rejects(f.start(),/Mail|503|HTTP/);const run=await f.c.command('run',{target:f.run.id});assert.equal(run.status,'paused');assert.equal(run.desiredState,'paused');
 assert.ok((await f.snapshot()).attempts.every((a:any)=>a.state==='admitted'||a.ownership==='stopped'));assert.equal(existsSync(join(f.runtime,(await f.snapshot()).attempts[0].id,'host.json')),false);
 f.offline(false);await f.resume();await f.idle();assert.equal((await f.c.command('run',{target:f.run.id})).status,'running');
});

test('actual runner exits consume only the failed assignment repair budget and eventually hold',{timeout:120000},async t=>{
 const f=await workflowFixture(t);await f.start();let runtime=await f.idle();const coordinator=runtime.assignments.find((a:any)=>a.coordinator).id;
 for(let cycle=0;cycle<3;cycle++){
  const assignment=runtime.assignments.find((a:any)=>!a.coordinator);const attempt=assignment.currentAttemptId;
  await f.input(attempt,{kind:'exit'}).catch(()=>{});
  if(cycle<2){runtime=await until(async()=>{const r=await f.snapshot();return !(await f.c.command('busy',{target:f.run.id}))&&r.assignments.find((a:any)=>a.id===assignment.id).currentAttemptId!==attempt&&r.assignments.every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;}).catch(async e=>{t.diagnostic(JSON.stringify({run:await f.c.command('run',{target:f.run.id}),runtime:await f.snapshot()}));throw e;});assert.equal(runtime.assignments.find((a:any)=>a.id===assignment.id).automaticCycles,cycle+1);assert.equal(runtime.assignments.find((a:any)=>a.id===coordinator).automaticCycles??0,0);}
 }
 await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='attention_required');runtime=await f.snapshot();assert.ok(runtime.attempts.every((a:any)=>a.ownership==='stopped'));assert.match((await f.c.command('run',{target:f.run.id})).attentionReason,/repair|cycle|budget|limit/i);
});

test('active budget stops owned processes without using a repair cycle',{timeout:60000},async t=>{
 const f=await workflowFixture(t);await f.c.command('limit',{target:f.run.id,input:{activeLimitMs:1000}});await f.start();
 await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='paused');const runtime=await f.snapshot();assert.ok(runtime.attempts.every((a:any)=>a.ownership==='stopped'));assert.ok(runtime.assignments.every((a:any)=>(a.automaticCycles??0)===0));
 const run=await f.c.command('run',{target:f.run.id});assert.equal(run.desiredState,'paused');assert.match(JSON.stringify(run),/budget/);
});
