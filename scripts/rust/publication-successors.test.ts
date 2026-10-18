/** Fresh native revision/check/review evidence for every same-PR successor. */
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {defaultRoles} from '../../server/roles';
import {setup,summary} from './publication-support';
import {body} from './publication-workflow-fixture';
type Fixture=Awaited<ReturnType<typeof setup>>;
async function pause(r:Fixture,run=r.f.seed.run.id,cancel=false,completion=false){
 if(!completion)await r.f.c.command('desired',{target:run,input:cancel?'cancelled':'paused'});await r.f.c.command(cancel?'cancel':'pause',{target:run});for(const a of (await r.f.c.command('runtime',{target:run})).attempts)if(a.ownership!=='stopped')await r.f.c.command('ownership',{target:a.id});await r.f.c.command('finish_pause',{target:run});await r.f.c.command('reconcile_writer',{target:run});await r.f.c.command('checkpoint',{target:run});
}
async function resume(r:Fixture,run=r.f.seed.run.id){
 await r.f.c.command('desired',{target:run,input:'running'});const agents:any={};for(const a of (await r.f.c.command('runtime',{target:run})).assignments)agents[a.name.toLowerCase()]=await r.f.c.command('replace',{target:a.id,input:{automatic:false}});await r.f.c.command('continue',{target:run});await r.f.c.command('activate',{target:run});return agents;
}
async function validate(r:Fixture,candidate:any,agents:any,run=r.f.seed.run.id){
 const key=randomUUID();const check=await r.f.c.command('execute_check',{target:agents.reviewer.id,key,input:{checkId:'required'}});assert.equal(check.completed,true);assert.equal(check.sourceIntact,true);assert.equal(check.exitCode,0);
 await r.f.c.command('assign_review',{target:agents.coordinator.id,key:'review-'+key,input:{assignee:'Reviewer',candidateId:candidate.id,instruction:'Review the freshly changed revision.'}});
 await r.f.c.command('review',{target:agents.reviewer.id,input:{candidateId:candidate.id,verdict:'pass',summary:'Fresh candidate verified.',findings:[],coverage:[{criterion:0,assessment:'Checked the current implementation.',checkIds:['required']}]}});assert.equal((await r.f.c.command('assess',{target:run})).ready,true);
}
async function revise(r:Fixture,agents:any,run=r.f.seed.run.id,claim=true){
 if(claim)await r.f.c.command('claim_implementation',{target:agents.coordinator.id,key:'feedback-claim',input:{assignee:'Writer',instruction:'Cover the captured review feedback.'}});else await r.f.c.command('claim',{target:agents.writer.id});
 const read=await r.f.c.command('read',{target:run,path:'source.txt'});await r.f.c.command('edit',{target:agents.writer.id,key:'next-edit',input:{path:'source.txt',expectedHash:read.sha256,content:'after\nAdditional reviewed case\n'}});
 const completion=await r.f.c.command('request_completion',{target:agents.writer.id,key:'next-complete',input:{summary:'Cover the additional reviewed case'}});await pause(r,run,false,true);return r.f.c.command('complete',{target:run,key:completion.id});
}
function comment(id=51,text='Cover an additional edge case'){return {id,created_on:'2026-10-03T00:00:00Z',updated_on:'2026-10-03T00:00:00Z',deleted:false,content:{raw:text},user:{display_name:'Reviewer'}};}

test('native target refresh publishes a freshly checked candidate on the same PR and reconciles a lost description reply',{timeout:120000},async t=>{
 const r=await setup(t),first=await r.publish();await pause(r);
 writeFileSync(join(r.f.operator,'target.txt'),'new target\n');r.f.gitAt(r.f.operator,'add','target.txt');r.f.gitAt(r.f.operator,'commit','-m','Target advances');r.f.gitAt(r.f.operator,'push',r.f.remote,'main');
 const refreshed=await r.f.c.command('target_request',{target:first.runId,input:{requestId:randomUUID()}});assert.equal(refreshed.changed,true);const candidate=(await r.f.c.command('record',{target:first.runId})).candidates.at(-1);assert.notEqual(candidate.id,first.candidateId);assert.equal((await r.f.c.command('assess',{target:first.runId})).ready,false);const agents=await resume(r);await validate(r,candidate,agents);
 await r.f.prepare({candidateId:candidate.id,summary:summary+' Updated for the current target.'},agents.coordinator.id);
 let lost=true;r.state.intercept=async(req,res)=>{if(req.method!=='PUT'||!lost)return false;lost=false;Object.assign(r.state.prs[0],await body(req));r.state.puts++;res.destroy();return true;};
 await r.f.flush();assert.equal((await r.f.record()).ready,false);assert.equal(r.reference(first.branch),candidate.revision.head);assert.equal(r.state.posts,1);assert.equal(r.state.puts,1);await r.f.flush();const p=await r.f.record();assert.equal(p.ready,true,JSON.stringify(p));assert.equal(p.id,first.id);assert.equal(p.pr.id,first.pr.id);assert.equal(p.history[0].candidateId,first.candidateId);assert.equal(r.state.puts,1);assert.match(p.description,/Rebased onto the target/);
});

test('native captured feedback produces fresh evidence and publishes the successor on its existing PR',{timeout:120000},async t=>{
 const r=await setup(t),first=await r.publish();await pause(r);await r.f.c.command('release_slot',{target:first.runId,candidateId:first.candidateId});r.state.comments=[comment()];await r.f.c.command('publication_feedback',{target:first.runId});assert.equal(await r.f.c.command('queue_feedback',{target:first.runId}),true);const captured=(await r.f.c.command('run',{target:first.runId})).feedbackContinuations[0];
 r.state.comments.push(comment(52,'A later uncaptured suggestion'));await r.f.c.command('publication_feedback',{target:first.runId});assert.equal((await r.f.c.command('run',{target:first.runId})).feedbackContinuations[0].feedback.comments.length,1);
 await r.f.c.command('reclaim_slot',{target:first.runId});await r.f.c.command('resume_state',{target:first.runId,input:'starting'});let agents=await resume(r);await r.f.c.command('feedback_handoff',{target:first.runId,key:captured.id});await r.f.c.command('clear_resume',{target:first.runId});const candidate=await revise(r,agents);assert.deepEqual(candidate.feedbackFrom,{continuationId:captured.id,sourceCandidateId:first.candidateId});assert.equal((await r.f.c.command('assess',{target:first.runId})).ready,false);
 agents=await resume(r);await assert.rejects(r.f.prepare({candidateId:candidate.id,summary},agents.coordinator.id),/locally validated/);await validate(r,candidate,agents);await r.f.prepare({candidateId:candidate.id,summary:summary+' Covers captured review feedback.'},agents.coordinator.id);await r.f.flush();const p=await r.f.record();assert.equal(p.ready,true,JSON.stringify(p));assert.equal(p.id,first.id);assert.equal(p.pr.id,first.pr.id);assert.equal(p.history[0].candidateId,first.candidateId);assert.equal(p.feedback.comments.length,2);assert.equal(r.state.posts,1);assert.equal(r.state.puts,1);
});

test('native cancelled continuation retains its PR but requires new candidate, target proof, checks and independent review',{timeout:150000},async t=>{
 const r=await setup(t),first=await r.publish();await pause(r,first.runId,true);await r.f.c.command('release_slot',{target:first.runId,reason:'cancelled'});
 const roles=defaultRoles.map(role=>({...role,model:'fixture/model',effort:'low'}));const task=await r.f.c.command('task',{target:r.f.seed.task.id});const next=await r.f.c.command('queue_later',{target:first.runId,input:{requestId:randomUUID(),revision:task.revision,roleId:'coordinator'},roles});assert.equal(next.continuedFrom.publicationId,first.id);await r.f.c.command('claim_slot');
 const work=await r.f.c.command('prepare',{target:next.id,input:next.launch.repository});assert.equal(work.branch,first.branch);await r.f.c.command('configure',{target:next.id,input:[r.proof.definition]});
 const agents:any={};for(const key of ['coordinator','writer','reviewer','reporter']){const a=r.f.seed[key].assignment;const admitted=await r.f.c.command('admit',{target:next.id,input:{role:a.role,model:a.model,name:a.name,coordinator:key==='coordinator',input:'Continue retained work with fresh validation.'}});agents[key]=admitted.attempt;}
 await r.f.c.command('activate',{target:next.id});const raw=await revise(r,agents,next.id,false);await assert.rejects(r.f.prepare({candidateId:raw.id,summary},agents.coordinator.id),/coordinator|validated/);
 await r.f.c.command('target_request',{target:next.id,input:{requestId:randomUUID()}});const candidate=(await r.f.c.command('record',{target:next.id})).candidates.at(-1);const validation=await resume(r,next.id);await validate(r,candidate,validation,next.id);const prepared=await r.f.prepare({candidateId:candidate.id,summary:summary+' Completes the cancelled predecessor work.'},validation.coordinator.id);assert.equal(prepared.id,first.id);await r.f.flush();const p=await r.f.record(next.id);assert.equal(p.ready,true,JSON.stringify(p));assert.equal(p.pr.id,first.pr.id);assert.equal(r.state.posts,1);assert.equal(r.state.puts,1);assert.equal((await r.f.record(first.runId)).candidateId,first.candidateId);
});

test('native feedback remains independent of a held flush and retains partial, older and racing comment observations',{timeout:120000},async t=>{
 const r=await setup(t),p=await r.publish();r.state.comments=[comment()];await r.f.c.command('publication_feedback',{target:p.runId});
 let entered!:()=>void;const waiting=new Promise<void>(resolve=>entered=resolve);let respond!:()=>void;let held=false;
 r.state.intercept=(req,res)=>{if(!req.url!.includes('/statuses')||held)return false;held=true;respond=()=>res.end(JSON.stringify({values:[{key:'build',state:'SUCCESSFUL',name:'Build',url:'https://example.invalid/build',updated_on:r.state.date}]}));entered();return true;};const flush=r.f.flush();await waiting;r.state.comments.push(comment(52,'Read while publication waits'));const feedback=await r.f.c.command('publication_feedback',{target:p.runId});assert.equal(feedback.comments.length,2);respond();await flush;assert.equal((await r.f.record()).feedback.comments.length,2);
 r.state.intercept=undefined;r.state.comments=[{...comment(),updated_on:'2026-10-02T00:00:00Z',content:{raw:'Older content'}}];const older=await r.f.c.command('publication_feedback',{target:p.runId});assert.equal(older.state,'partial');assert.equal(older.comments[0].content.raw,'Cover an additional edge case');assert.equal(older.comments[1].currentness,'unknown');
 r.state.comments=[comment(),{id:'invalid'}];const partial=await r.f.c.command('publication_feedback',{target:p.runId});assert.equal(partial.state,'partial');assert.equal(partial.comments.length,2);assert.equal((await r.f.record()).ready,true);
});
