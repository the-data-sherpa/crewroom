/** Actual local OMP + Rust Mail. Model replies are scripted behind an OS network guard. */
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync,writeFileSync} from 'node:fs';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';
const live=JSON.parse(readFileSync(process.argv[2],'utf8'));
test('actual OMP and Mail deliver, steer, reattach and resume through the full native workflow',{timeout:150000},async t=>{
 const f=await workflowFixture(t,'research',1,false,{parent:live.root,service:live.service===true,configure({config,roles}){
  for(const role of roles){const selected=role.id==='tech-writer'?live.models.specialist:live.models.coordinator;role.model=selected.selector;role.effort=selected.effort;}
  writeFileSync(config.roles,JSON.stringify(roles),{mode:0o600});Object.assign(config.runtime,live.runtime);config.runtime.mail.projectRoot=live.root+'/mail-projects';
 }});
 await f.start();let runtime=await until(async()=>{const r=await f.snapshot();return r.messages[0]?.disposition==='received'?r:false;});await f.idle();assert.equal(runtime.messages.length,1);assert.equal(runtime.messages[0].body,'LIVE_NATIVE_HANDOFF');
 assert.equal(new Set(runtime.attempts.map((a:any)=>a.hostPid)).size,2);assert.equal(new Set(runtime.attempts.map((a:any)=>a.ompPid)).size,2);assert.ok(runtime.attempts.every((a:any)=>a.hostPid!==a.ompPid&&a.sessionId));
 const initial=runtime;await f.kill();await f.reopen();await f.idle();runtime=await f.snapshot();assert.deepEqual(runtime.attempts.map((a:any)=>[a.id,a.hostPid,a.ompPid,a.sessionId]),initial.attempts.map((a:any)=>[a.id,a.hostPid,a.ompPid,a.sessionId]));
 const steering=await f.c.command('steer',{target:f.run.id,input:{requestId:randomUUID(),body:'LIVE_OPERATOR_STEERING'}});await until(async()=>(await f.c.command('steering',{target:f.run.id})).some((s:any)=>s.id===steering.id&&s.delivery==='received'));await f.idle();
 await f.pause();assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));await f.resume();runtime=await f.idle();assert.equal(runtime.attempts.length,4);assert.ok(runtime.assignments.every((a:any)=>a.automaticCycles===0));assert.equal(runtime.messages.length,1);
 const coordinator=runtime.assignments.find((a:any)=>a.coordinator),latest=runtime.attempts.find((a:any)=>a.id===coordinator.currentAttemptId);assert.equal(latest.sessionId,initial.attempts.find((a:any)=>a.assignmentId===coordinator.id).sessionId);assert.equal(latest.recoveryMode,'session');
 const events=await f.c.command('events',{target:f.run.id});assert.ok(events.some((e:any)=>e.event.type==='agent_start'));assert.ok(events.some((e:any)=>e.event.type==='tool_execution_end'&&e.event.toolName==='send_message'&&e.event.isError===false));
 t.diagnostic(JSON.stringify({messages:runtime.messages.length,attempts:runtime.attempts.length,restoredSession:latest.sessionId,independentHostPids:runtime.attempts.map((a:any)=>a.hostPid),independentRunnerPids:runtime.attempts.map((a:any)=>a.ompPid)}));
});
