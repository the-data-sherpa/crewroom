import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {realpathSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until,acceptFixturePlan} from './workflow-support';
import {NativeDaemon} from './daemon-client';
import {seedRepository} from '../../tests/fixtures/repository';

// FOR-79: per-project settings resolve at admission in the production daemon. A coordinator started
// in a project captures that project's instructions and principles (project override over workspace);
// a coordinator in another project captures the workspace values; later changes never alter the
// admitted assignments, and a project that does not allow the coordinator role refuses its start.
// Agents are the scripted fixture runner with synthetic provider replies (simulated, not live).
test('admission captures each project\'s resolved settings and later changes never alter them',{timeout:180000},async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true});
 const client=()=>{assert.ok(f.c instanceof NativeDaemon);return f.c;};
 await client().request(`/api/queue/${f.run.id}/remove`,{requestId:randomUUID()});
 const second=join(f.root,'second-source');seedRepository(second);
 const beta=await client().request('/api/repositories',{name:'Second project',path:realpathSync(second),targetBranch:'main'});
 const alpha=f.repository.id;
 await client().request('/api/principles',{id:'small-changes',name:'Small changes',text:'Prefer small reviewable changes.',revision:0});
 await client().request('/api/project-settings',{repositoryId:null,set:{instructions:'Workspace rule.',principles:['small-changes']}});
 await client().request('/api/project-settings',{repositoryId:alpha,set:{instructions:'Alpha rule.',capacity:2}});
 const queue=async(title:string,repositoryId:string)=>{const c=client();const task=await c.request('/api/tasks',{title,description:'',repositoryId});await c.request(`/api/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:title,criteria:['Checked change']});await acceptFixturePlan(c,task.id);return c.request(`/api/tasks/${task.id}/runs`,{revision:2,launch:{repository:{mode:'new'},team:[f.team[0]]}});};
 const state=(id:string)=>client().request(`/api/runs/${id}/runtime`);
 const running=(id:string)=>until(async()=>{const v=await state(id);return v.run.status==='running'&&!v.busy&&v.attempts.length?v:false;},60000);
 const a=await queue('Alpha ticket',alpha);const b=await queue('Beta ticket',beta.id);
 const [alphaRun,betaRun]=await Promise.all([running(a.id),running(b.id)]);
 const role=(v:any)=>v.assignments[0].role;
 assert.deepEqual(role(alphaRun).projectSettings.instructions,{text:'Alpha rule.',source:'project'});
 assert.deepEqual(role(betaRun).projectSettings.instructions,{text:'Workspace rule.',source:'workspace'});
 for(const v of [alphaRun,betaRun]){assert.ok(role(v).principles.includes('small-changes'));assert.equal(v.assignments[0].brief.principles.find((p:any)=>p.id==='small-changes').text,'Prefer small reviewable changes.');}
 const workspace=await client().request('/api/workspace');
 assert.equal(workspace.queue.capacity.projects.find((p:any)=>p.repositoryId===alpha).capacity,2);
 // Changing the override and the workspace values leaves the admitted assignments as captured.
 await client().request('/api/project-settings',{repositoryId:alpha,set:{instructions:'Changed later.'}});
 await client().request('/api/project-settings',{repositoryId:null,reset:['instructions','principles']});
 assert.deepEqual(role(await state(a.id)),role(alphaRun));assert.deepEqual(role(await state(b.id)),role(betaRun));
 // A project that does not allow the coordinator role cannot admit it.
 await client().request('/api/project-settings',{repositoryId:beta.id,set:{'roles.coordinator':false}});
 const task=await client().request('/api/tasks',{title:'Beta blocked',description:'',repositoryId:beta.id});
 await client().request(`/api/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:'Beta blocked',criteria:['Checked change']});await acceptFixturePlan(client(),task.id);
 const blocked=await client().request(`/api/tasks/${task.id}/runs`,{revision:2});
 await client().request(`/api/runs/${b.id}/cancel`,{});
 await assert.rejects(client().request(`/api/runs/${blocked.id}/start`,{team:[f.team[0]]}),/unavailable/i);
 assert.equal((await state(blocked.id)).attempts.length,0,'No agent is admitted for a role the project does not allow.');
 await client().request(`/api/runs/${a.id}/cancel`,{});
 t.diagnostic(JSON.stringify({alpha:role(alphaRun).projectSettings,beta:role(betaRun).projectSettings}));
});
