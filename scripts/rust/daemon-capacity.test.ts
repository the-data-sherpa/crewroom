import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {realpathSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until,acceptFixturePlan} from './workflow-support';
import {NativeDaemon} from './daemon-client';
import {seedRepository} from '../../tests/fixtures/repository';

// FOR-76: per-project run capacity. Two projects run coding tickets at once with separate
// coordinators, worktrees and supervision; a third ticket in a full project is queued with its
// reason, survives a daemon restart, and starts by itself when its project's slot frees because the
// holder paused with stopped ownership; the paused run's Resume then waits for the next free slot.
// Agents are the scripted fixture runner with synthetic provider replies (simulated, not live).
test('two projects execute concurrently and a queued ticket starts when its project frees a slot',{timeout:180000},async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true});
 const client=()=>{assert.ok(f.c instanceof NativeDaemon);return f.c;};
 await client().request(`/api/queue/${f.run.id}/remove`,{requestId:randomUUID()});
 const second=join(f.root,'second-source');seedRepository(second);
 const beta=await client().request('/api/repositories',{name:'Second project',path:realpathSync(second),targetBranch:'main'});
 const queue=async(title:string,repositoryId:string,launch=true)=>{const c=client();const task=await c.request('/api/tasks',{title,description:'',repositoryId});await c.request(`/api/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:title,criteria:['Checked change']});await acceptFixturePlan(c,task.id);return c.request(`/api/tasks/${task.id}/runs`,{revision:2,...(launch?{launch:{repository:{mode:'new'},team:[f.team[0]]}}:{})});};
 const state=(id:string)=>client().request(`/api/runs/${id}/runtime`);
 const running=(id:string)=>until(async()=>{const v=await state(id);return v.run.status==='running'&&!v.busy&&v.attempts.length?v:false;},60000);
 const plan=async(id:string)=>(await client().request('/api/workspace')).queue.plans.find((p:any)=>p.id===id);
 const a1=await queue('Alpha first',f.repository.id);const b1=await queue('Beta first',beta.id);
 const [alpha,betaRun]=await Promise.all([running(a1.id),running(b1.id)]);
 // Separate coordinators (distinct attempts and host processes) and separate worktrees.
 assert.equal(alpha.attempts.length,1);assert.equal(betaRun.attempts.length,1);
 assert.notEqual(alpha.attempts[0].id,betaRun.attempts[0].id);
 assert.ok(alpha.attempts[0].hostPid&&betaRun.attempts[0].hostPid);assert.notEqual(alpha.attempts[0].hostPid,betaRun.attempts[0].hostPid);
 const works=await Promise.all([a1.id,b1.id].map(id=>client().request(`/api/runs/${id}/repository`)));
 const worktrees=works.map((w:any)=>w.path);
 assert.ok(worktrees.every(Boolean));assert.notEqual(worktrees[0],worktrees[1]);assert.notEqual(works[0].repository.path,works[1].repository.path);
 const workspace=await client().request('/api/workspace');
 assert.equal(workspace.queue.slots.length,2);assert.deepEqual(new Set(workspace.queue.slots.map((s:any)=>s.runId)),new Set([a1.id,b1.id]));
 const projects=workspace.queue.capacity.projects;
 assert.equal(projects.find((p:any)=>p.repositoryId===f.repository.id).occupied,1);assert.equal(projects.find((p:any)=>p.repositoryId===beta.id).occupied,1);
 // A third ticket in the full project is queued with the reason, not started.
 const a2=await queue('Alpha second',f.repository.id);
 const held=await until(async()=>{const p=await plan(a2.id);return p?.holdReason?.startsWith('Waiting for a slot')?p:false;});
 assert.equal(held.holdReason,'Waiting for a slot in Native workflow: 1 of 1 running');
 assert.equal((await state(a2.id)).attempts.length,0,'A queued plan admits no agent.');
 // A manual start over capacity is queued with the same reason instead of refused.
 const b2=await queue('Beta manual',beta.id,false);
 const queued=await client().request(`/api/runs/${b2.id}/start`,{team:[f.team[0]]});
 assert.equal(queued.holdReason,'Waiting for a slot in Second project: 1 of 1 running');
 assert.equal((await state(b2.id)).attempts.length,0);
 await client().request(`/api/queue/${b2.id}/remove`,{requestId:randomUUID()});
 // The queue survives a hard restart of the service.
 await f.kill();await f.reopen();
 await running(a1.id);await running(b1.id);
 assert.equal((await plan(a2.id)).holdReason,'Waiting for a slot in Native workflow: 1 of 1 running');
 // Pausing the alpha holder with its ownership confirmed stopped frees the alpha slot: the queued
 // alpha ticket is admitted automatically; beta is unaffected.
 await client().request(`/api/runs/${a1.id}/pause`,{});
 const next=await running(a2.id);assert.equal(next.attempts.length,1);
 const paused=await state(a1.id);
 assert.equal(paused.run.status,'paused');assert.equal(paused.run.pauseReason,'operator');assert.equal(paused.run.slotRelease.reason,'paused');
 assert.ok(paused.attempts.every((a:any)=>a.ownership==='stopped'));
 assert.equal((await state(b1.id)).run.status,'running');
 const after=await client().request('/api/workspace');
 assert.deepEqual(new Set(after.queue.slots.map((s:any)=>s.runId)),new Set([b1.id,a2.id]));
 // Resume while alpha is full waits; cancelling the new holder frees the slot and the paused run resumes.
 await client().request(`/api/runs/${a1.id}/resume`,{});assert.equal((await state(a1.id)).run.resumeRequest.state,'queued');
 await client().request(`/api/runs/${a2.id}/cancel`,{});
 const resumed=await running(a1.id);assert.equal(resumed.run.slotRelease,undefined);
 await client().request(`/api/runs/${a1.id}/cancel`,{});await client().request(`/api/runs/${b1.id}/cancel`,{});
 t.diagnostic(JSON.stringify({worktrees,hostPids:[alpha.attempts[0].hostPid,betaRun.attempts[0].hostPid,next.attempts[0].hostPid]}));
});
