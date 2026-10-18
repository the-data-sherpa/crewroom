import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {existsSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';
import {NativeDaemon} from './daemon-client';

test('native scheduler restarts a killed initial admission from its durable decision without a second team or worktree',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true});assert.ok(f.c instanceof NativeDaemon);f.holdMail();
 await f.c.request(`/api/runs/${f.run.id}/launch`,{requestId:randomUUID(),launch:{repository:{mode:'new'},team:f.team}});
 await until(()=>f.requests.length>0);const before=await f.snapshot();assert.equal(before.assignments.length,1);assert.ok(before.attempts.every((a:any)=>a.state==='admitted'));
 const work=await f.c.request(`/api/runs/${f.run.id}/repository`);await f.kill();f.releaseMail();await f.reopen();const after=await f.idle();
 assert.deepEqual(after.assignments.map((a:any)=>a.id),before.assignments.map((a:any)=>a.id));assert.equal(after.attempts.length,1);assert.equal(after.run.schedule.id,before.run.schedule.id);
 assert.ok(f.c instanceof NativeDaemon);assert.equal((await f.c.request(`/api/runs/${f.run.id}/repository`)).path,work.path);assert.equal(after.run.status,'running');assert.ok(after.run.launchCompletedAt);
});

test('Pause crossing pending daemon admission fences host dispatch while health and observations remain responsive',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'research',1,false,{service:true});assert.ok(f.c instanceof NativeDaemon);const c=f.c;f.holdMail();
 const start=f.start();const failed=assert.rejects(start,/control|stopped|Pause|superseded/i);await until(()=>f.requests.length>0);
 const pause=f.pause();await until(async()=>(await f.snapshot()).run.desiredState==='paused');assert.equal((await c.request('/api/health')).executionAvailable,true);f.releaseMail();await failed;
 assert.equal((await pause).status,'paused');const state=await f.snapshot();assert.ok(state.attempts.every((a:any)=>a.state==='admitted'||a.ownership==='stopped'));
 assert.ok(state.attempts.every((a:any)=>!existsSync(join(f.runtime,a.id,'host.json'))));
});

test('SIGTERM fences the API and drains a pending daemon launch before releasing controller authority',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'research',0,false,{service:true});assert.ok(f.c instanceof NativeDaemon);const c=f.c;f.holdMail();
 const starting=f.start().then(value=>({value}),error=>({error:String(error)}));await until(()=>f.requests.length>0);
 c.proc.kill('SIGTERM');await until(async()=>{try{await c.request('/api/health');return false;}catch{return true;}});f.releaseMail();await starting;await c.exit;
 assert.equal(c.proc.exitCode,0,c.stderr);assert.equal(existsSync(join(f.config.authority,'native-connection.json')),false);
 await f.reopen();const state=await f.snapshot();assert.ok(state.attempts.every((a:any)=>a.state==='admitted'||a.ownership==='stopped'));assert.equal(state.run.desiredState,'paused');assert.equal(state.run.status,'paused');
});
