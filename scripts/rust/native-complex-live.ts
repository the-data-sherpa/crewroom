import { randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import {nativeScenarioBackend,nativeIdentities} from './native-scenario-backend';
import { deliveryProviderFixture } from '../delivery-provider-fixture';
import { createRuntimeObserver } from '../runtime-observer';
const config=JSON.parse(readFileSync(process.argv[2],'utf8'));
const checkout=join(config.root,'operator');mkdirSync(checkout);
const git=(...args:string[])=>execFileSync('git',['-C',checkout,...args],{encoding:'utf8',stdio:'pipe'}).trim();
git('init','-b','main');git('config','user.name','Fixture');git('config','user.email','fixture@example.invalid');
for(const [path,content] of [['filters.mjs','export function exportFilters() { return "not implemented"; }\n'],['index.html','<!doctype html><title>Saved filters</title><h1>Saved filters</h1>\n'],['GUIDE.md','# Saved filters\nExport is not available.\n']]) writeFileSync(join(checkout,path),content);
for(const path of ['unit.mjs','browser.mjs']) writeFileSync(join(checkout,path),readFileSync(join('docs/testing/fixtures/saved-filter-app',path)));
writeFileSync(join(checkout,'AGENTS.md'),'Implement only saved-filter export and its guide. Run both configured unit and browser checks.\n');
git('add','.');git('commit','-m','saved-filter fixture');const base=git('rev-parse','HEAD');
const provider=await deliveryProviderFixture(config.root,checkout);provider.fault.bitbucket='available';provider.fault.pipeline='successful';
const app=await nativeScenarioBackend(config,provider.options.publication);
const request=async(path:string,body?:unknown):Promise<any>=>{
 const response=await fetch(app.origin+'/api'+path,body===undefined?{headers:{Authorization:`Bearer ${app.token}`}}:{method:'POST',headers:{Authorization:`Bearer ${app.token}`,Origin:app.origin,'Content-Type':'application/json'},body:JSON.stringify(body)});
 const value=await response.json();assert.ok(response.ok,JSON.stringify(value));return value;
};
const observe=createRuntimeObserver(request);
const completed=(state:any,name:string,marker:string)=>{
 const id=state.assignments.find((a:any)=>a.name===name)?.currentAttemptId;
 return state.attempts.find((a:any)=>a.id===id)?.state==='idle' && state.events.some((e:any)=>e.attemptId===id && e.event.type==='message_end' && e.event.message?.role==='assistant' && e.event.message.content?.some((p:any)=>p.type==='text' && p.text===marker));
};
try {
 const roles=(await request('/workspace')).roles;
 await request('/roles',{...roles.find((r:any)=>r.id==='triage'),id:'guide-specialist',name:'Guide specialist',instructions:'Plan and independently inspect the user guide. Route implementation through the coordinator.'});
 const image=config.browserImage;assert.match(image,/^sha256:[a-f0-9]{64}$/);
 const checks=[{id:'unit',image,command:'/usr/local/bin/node',args:['--test','unit.mjs'],timeoutMs:15000,required:true},{id:'browser',image,command:'/usr/local/bin/node',args:['browser.mjs'],timeoutMs:30000,required:true,memoryMiB:2048,tmpExecutable:true}];
 const repo=await request('/repositories',{name:'Saved filters fixture',path:checkout,targetBranch:'main',checks});
 const task=await request('/tasks',{title:'Saved-filter export',description:'Export saved filters with validation and a guide.',repositoryId:repo.id});
 const criteria=['Export saved filters with an explicit empty state','Reject malformed input without stale output','Update the user guide with empty and invalid input behavior'];
 await request(`/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:'Implement saved-filter export, empty state, input validation and the user guide.',criteria});
 const run=await request(`/tasks/${task.id}/runs`,{revision:2});const owned=await request(`/runs/${run.id}/repository`,{mode:'new'});
 await request(`/runs/${run.id}/start`,{team:[
  {name:'Coordinator',roleId:'triage',coordinator:true,input:'Get a guide plan, delegate staged implementation under one writer, obtain independent unit/browser checks and review, then the guide assessment before publication.'},
  {name:'SpecialistOne',roleId:'implementer',coordinator:false,input:'Wait for the coordinator writer grant; implement the three criteria.',criteria:criteria.map((_,index)=>({index,obligation:'Implement this criterion under one writer lease.',checkIds:index<2?['unit','browser']:['browser']}))},
  {name:'SpecialistTwo',roleId:'reviewer',coordinator:false,input:'Independently run both checks and review every criterion.',criteria:criteria.map((_,index)=>({index,obligation:'Inspect the exact candidate and execute the relevant checks.',checkIds:index<2?['unit','browser']:['browser']})),dependsOn:['SpecialistOne']},
  {name:'Guide',roleId:'guide-specialist',coordinator:false,input:'Plan the guide first; after implementation inspect its contract.',criteria:[{index:2,obligation:'Specify and assess guide behavior.',checkIds:['browser']}]}]});
 let state:any,coding:any;const until=Date.now()+120000;
 do {
  state=await observe(run.id);coding=await request(`/runs/${run.id}/coding`);
  assert.ok(state.assignments.filter((a:any)=>!a.coordinator && !a.retiredAt).length<=3);
  if(coding.assessment.ready && completed(state,'Coordinator','COMPLEX_COMPLETE') && completed(state,'Guide','Guide assessed.')) break;
  await new Promise(resolve=>setTimeout(resolve,100));
 } while(Date.now()<until);
 assert.equal(coding.assessment.ready,true,JSON.stringify({coding,attempts:state.attempts,error:state.error}));
 assert.ok(completed(state,'Coordinator','COMPLEX_COMPLETE'),'Current coordinator did not finish.');
 assert.ok(completed(state,'Guide','Guide assessed.'),'Current Guide did not finish its assessment.');
 const guide=state.assignments.find((a:any)=>a.name==='Guide');const coordinator=state.assignments.find((a:any)=>a.coordinator);
 for(const body of ['Document empty export as [] and explain required name/query fields and invalid input.','The saved guide matches empty-state and input-validation behavior.']) assert.ok(state.messages.some((m:any)=>m.from===guide.id && m.to===coordinator.id && m.body===body && m.disposition==='received' && m.transportId));
 assert.ok(state.events.some((e:any)=>e.attemptId===guide.currentAttemptId && e.event.type==='tool_execution_end' && e.event.toolName==='repository_read' && e.event.isError===false && JSON.stringify(e.event).includes('GUIDE.md')));
 assert.equal(coding.record.candidates.length,1);assert.equal(coding.record.claims.length,1);assert.equal(coding.record.edits.length,3);
 assert.equal(new Set(coding.record.edits.map((edit:any)=>edit.attemptId)).size,1);
 assert.equal(coding.record.checks.length,2);
 for(const check of coding.record.checks){assert.equal(check.candidateId,coding.assessment.candidate.id);assert.equal(check.exitCode,0);assert.ok(check.started && check.completed && check.sourceIntact);}
 assert.equal(coding.record.reviews.length,1);assert.equal(coding.record.reviews[0].coverage.length,3);
 assert.ok(!coding.assessment.candidate.implementationAttempts.includes(coding.record.reviews[0].attemptId));
 for(const path of ['filters.mjs','index.html','GUIDE.md']) assert.equal(readFileSync(join(owned.path,path),'utf8'),readFileSync(join('docs/testing/fixtures/saved-filter-app',path),'utf8'));
 assert.equal(git('rev-parse','HEAD'),base);assert.equal(git('status','--porcelain'),'');
 assert.equal(new Set(state.assignments.map((a:any)=>a.model.resolved.selector)).size,3);
 for(const assignment of state.assignments){const attempt=state.attempts.find((a:any)=>a.id===assignment.currentAttemptId);assert.ok(state.events.some((e:any)=>e.attemptId===attempt.id && e.event.type==='host_ready'));}
 for(let n=0;n<4;n++){await request(`/runs/${run.id}/publication/reconcile`,{});if((await request(`/runs/${run.id}/result`)).publication.record?.ready)break;await new Promise(resolve=>setTimeout(resolve,100));}
 const result=await request(`/runs/${run.id}/result`);assert.equal(result.publication.record.ready,true,JSON.stringify(result.publication));
 assert.equal(provider.receipts().prPosts,1);assert.ok(!provider.requests.some(path=>/merge|deploy/.test(path)));
 const initialIdentities=await nativeIdentities(config,state);
 await request(`/runs/${run.id}/pause`,{});
 let drift;
 if(config.targetDrift){
  const old=coding.assessment.candidate;
  const oldBytes=JSON.stringify(old);
  const other=join(config.root,'other');execFileSync('git',['clone','--branch','main',provider.remote,other],{stdio:'pipe'});
  const otherGit=(...args:string[])=>execFileSync('git',['-C',other,...args],{encoding:'utf8',stdio:'pipe'}).trim();
  otherGit('config','user.name','Other fixture');otherGit('config','user.email','other@example.invalid');
  writeFileSync(join(other,'unrelated.txt'),'Remote target advance.\n');otherGit('add','.');otherGit('commit','-m','Advance target');otherGit('push','origin','main');
  const advanced=otherGit('rev-parse','HEAD');
  await request(`/runs/${run.id}/publication/reconcile`,{});
  assert.equal((await request(`/runs/${run.id}/result`)).publication.record.ready,false);
  const refresh=await request(`/runs/${run.id}/target-refresh`,{requestId:randomUUID()});assert.equal(refresh.changed,true);
  const fresh=await request(`/runs/${run.id}/coding`);assert.equal(fresh.assessment.ready,false);
  assert.equal(fresh.assessment.candidate.refreshedFrom,old.id);assert.equal(fresh.assessment.candidate.revision.base,advanced);assert.notEqual(fresh.assessment.candidate.revision.head,old.revision.head);
  writeFileSync(join(config.root,'drift-ready'),'ready');const stagedBy=Date.now()+5000;
  while(!existsSync(join(config.root,'drift-scripted')) && Date.now()<stagedBy)await new Promise(resolve=>setTimeout(resolve,20));assert.ok(existsSync(join(config.root,'drift-scripted')));
  provider.fault.hostedAt='2026-10-02T01:00:00Z';provider.fault.pipelineNumber=2;
  await request(`/runs/${run.id}/resume`,{});
  const checkedBy=Date.now()+90000;let updated:any;
  do{
   state=await observe(run.id);updated=await request(`/runs/${run.id}/result`);
   if(updated.publication.record?.candidateId===fresh.assessment.candidate.id && completed(state,'Coordinator','DRIFT_COMPLETE'))break;
   await new Promise(resolve=>setTimeout(resolve,100));
  }while(Date.now()<checkedBy);
  assert.ok(completed(state,'Coordinator','DRIFT_COMPLETE'),'Current coordinator did not finish target-drift recovery.');
  await request(`/runs/${run.id}/publication/reconcile`,{});updated=await request(`/runs/${run.id}/result`);
  assert.equal(updated.publication.record.ready,true,JSON.stringify(updated.publication));assert.equal(updated.assessment.candidate.id,fresh.assessment.candidate.id);
  assert.equal(JSON.stringify(updated.candidates.find((c:any)=>c.id===old.id)),oldBytes);
  const freshChecks=updated.checks.filter((c:any)=>c.candidateId===fresh.assessment.candidate.id);assert.equal(freshChecks.length,2);assert.ok(freshChecks.every((c:any)=>c.started && c.completed && c.sourceIntact && c.exitCode===0));
  assert.equal(updated.reviews.filter((r:any)=>r.candidateId===fresh.assessment.candidate.id && r.verdict==='pass').length,1);
  assert.equal(provider.receipts().prPosts,1);assert.equal(provider.receipts().prPuts,1);
  const refreshedIdentities=await nativeIdentities(config,state);
  writeFileSync(join(config.root,'refreshed-identities.json'),JSON.stringify(refreshedIdentities,null,2));
  await request(`/runs/${run.id}/pause`,{});const attempts=(await observe(run.id)).attempts.length;
  provider.fault.pipeline='failed';provider.fault.hostedAt='2026-10-02T02:00:00Z';provider.fault.pipelineNumber=3;
  await request(`/runs/${run.id}/publication/reconcile`,{});const failed=await request(`/runs/${run.id}/result`);assert.equal(failed.publication.record.ready,false);assert.equal(failed.publication.record.reviewInvalidated,true);
  const feedback=await request(`/runs/${run.id}/feedback`);assert.ok(JSON.stringify(feedback).includes('Current CI failed.'));
  provider.fault.pipeline='successful';provider.fault.hostedAt='2026-10-02T00:00:00Z';provider.fault.pipelineNumber=1;provider.fault.pipelineRevision={head:old.revision.head,base:old.revision.base};
  await request(`/runs/${run.id}/publication/reconcile`,{});const late=await request(`/runs/${run.id}/result`);assert.equal(late.publication.record.ready,false);assert.match(late.publication.record.error,/older observations/);
  assert.equal(late.publication.record.observation.statuses[0].state,'FAILED');
  const quiet=await observe(run.id);assert.equal(quiet.attempts.length,attempts);assert.ok(quiet.attempts.every((a:any)=>a.ownership==='stopped'));
  assert.equal(git('rev-parse','HEAD'),base);assert.equal(git('status','--porcelain'),'');
  drift={passed:true,from:old.revision,to:updated.assessment.candidate.revision,checks:freshChecks,oldSuccessRejected:true,currentFailureRetained:true,feedbackVisible:true,noAutomaticRestart:true};
 }
 const identities=initialIdentities;
 const report={passed:true,messages:state.messages,scenario:config.targetDrift?'E2E-02 + E2E-09':'E2E-02',browserImage:image,runId:run.id,criteria,drift,identities,checks:coding.record.checks,review:coding.record.reviews[0],candidate:coding.assessment.candidate,pr:result.publication.record.pr,oneWriter:true,operatorUnchanged:true,threeSpecialists:true,scriptedModelBoundary:true};writeFileSync(join(config.root,'scenario-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
} finally {
 await app.close();await provider.close();
}
