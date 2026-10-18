import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import type { PublicationRecord } from '../shared/publication';
import { hostedCiPending } from '../server/active-time';

// Controller-domain fixture: provider observations and process identities are explicit inputs.
// Actual hosted-provider and contained-process behavior is covered by the release scenarios.
test('LIM-01: pending hosted CI excludes settled wait across restart, but agent/tool work and uncertain stops consume the retained budget', t => {
  const root=mkdtempSync(join(tmpdir(),'crewroom-active-time-'));seedRepository(root);let now=1000;
  let store=new Workbench(join(root,'workflow.sqlite'),()=>now);
  t.after(()=>{store.close();rmSync(root,{recursive:true,force:true});});
  const repo=saveRepositoryFixture(store,{name:'Fixture',path:root,targetBranch:'main'});
  const task=store.createTask({title:'CI wait budget',description:'',repositoryId:repo.id});
  store.refine(task.id,{revision:1,sourceRevision:1,brief:'Exclude hosted waits',criteria:['Retain elapsed execution']});
  const run=store.queue(task.id,{revision:2});assert.equal(run.activeLimitMs,3600000);store.setActiveLimit(run.id,{activeLimitMs:2000});
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const {attempt}=store.admitAssignment(run.id,{name:'Coordinator',coordinator:true,role,model,input:'Observe current CI.'});
  store.claimAttempt(attempt.id);store.recordAttemptIdentity(attempt.id,{hostPid:101,ompPid:102,sessionId:'session',hostUrl:'http://127.0.0.1:9000',pane:'%0'});
  store.setDesiredState(run.id,'running');store.startExecution(run.id);let sequence=0;
  const event=(type:string,extra:Record<string,unknown>={})=>store.recordAgentEvent(attempt.id,++sequence,{type,...extra});
  event('agent_start');now=1500;event('session_settled');
  const head='a'.repeat(40),base='b'.repeat(40),repository={workspace:'fixture',slug:'fixture',uuid:'{00000000-0000-4000-8000-000000000001}'};
  const record:PublicationRecord={id:'publication',runId:run.id,attemptId:attempt.id,candidateId:'candidate',repositoryPath:root,repository,
    branch:'feature',target:'main',revision:{head,base,tree:'c'.repeat(40),worktreeHash:'hash',dirty:false,requirementsHash:'requirements',instructionsHash:'instructions'},
    title:'Fixture',description:'Fixture',policy:{statusKeys:['unit'],pipelineSteps:['unit'],optionalSteps:[]},createdAt:new Date(now).toISOString(),
    push:{state:'confirmed',expectedHead:head},pr:{state:'confirmed',id:1,providerState:'OPEN'},remoteSourceHead:head,remoteTargetHead:base,ready:false,blockers:['Hosted CI is running.'],
    observation:{observedAt:new Date(now).toISOString(),pr:{id:1,title:'Fixture',description:'Fixture',state:'OPEN',updated_on:new Date(now).toISOString(),source:{branch:{name:'feature'},commit:{hash:head},repository},destination:{branch:{name:'main'},commit:{hash:base},repository},links:{html:{href:'https://bitbucket.org/fixture/fixture/pull-requests/1'}}},
      statuses:[{key:'unit',state:'INPROGRESS',name:'Unit',url:'https://fixture.invalid/check',updated_on:new Date(now).toISOString()}],
      pipelines:[{uuid:'pipeline',build_number:1,created_on:new Date(now).toISOString(),state:{name:'IN_PROGRESS'},target:{type:'pipeline_pullrequest_target',commit:{hash:head},destination_commit:{hash:base},pullrequest:{id:1},selector:{type:'pull-requests',pattern:'**'}},steps:[]}]}};
  store.savePublication(record);assert.equal(store.accountTime(run.id).activeMs,500);
  const noPipeline=structuredClone(record);noPipeline.observation!.pipelines=[];
  assert.equal(hostedCiPending(noPipeline),true,'Current required status can establish CI wait before a pipeline is discovered.');
  const optionalSkip=structuredClone(record);optionalSkip.policy.optionalSteps=['Renovate'];optionalSkip.observation!.pipelines[0].steps=[{uuid:'optional',name:'Renovate',state:{name:'COMPLETED',result:{name:'SKIPPED'}}}];
  assert.equal(hostedCiPending(optionalSkip),true,'A skipped optional step does not charge required CI waiting.');
  const requiredFailure=structuredClone(optionalSkip);requiredFailure.observation!.pipelines[0].steps.push({uuid:'required',name:'unit',state:{name:'COMPLETED',result:{name:'FAILED'}}});
  assert.equal(hostedCiPending(requiredFailure),false,'Failed required work is not CI waiting.');
  const stale=structuredClone(noPipeline);stale.observation!.pr.source.commit.hash='d'.repeat(40);assert.equal(hostedCiPending(stale),false,'Old-head status cannot establish current wait.');
  now=100000;assert.equal(store.accountTime(run.id).activeMs,500,'Idle hosted CI wait consumes no execution budget.');
  store.close();now=200000;store=new Workbench(join(root,'workflow.sqlite'),()=>now);
  assert.equal(store.accountTime(run.id).activeMs,500,'Restart retains both prior usage and the wait.');
  event('agent_start');now+=200;event('session_settled');assert.equal(store.accountTime(run.id).activeMs,700,'Actual model execution during CI wait is charged.');
  now+=100000;event('tool_started',{toolCallId:'child'});event('session_settled');now+=300;
  assert.equal(store.accountTime(run.id).activeMs,1000,'A running child prevents idle-agent wait exclusion.');
  store.recordOwnership(attempt.id,true);now+=100000;
  assert.equal(store.accountTime(run.id).activeMs,1000,'Controller-confirmed stopped ownership covers a killed host with no tool_exited event.');
  store.savePublication({...record,error:'Current hosted observation is unavailable.'});now+=100;
  assert.equal(store.accountTime(run.id).activeMs,1100,'A stale observation cannot silently certify continued CI wait.');
  store.savePublication(record);now+=100000;assert.equal(store.accountTime(run.id).activeMs,1100);
  store.beginPause(run.id);store.recordOwnership(attempt.id,true);store.finishPause(run.id);now+=100000;
  store.close();store=new Workbench(join(root,'workflow.sqlite'),()=>now);assert.equal(store.accountTime(run.id).activeMs,1100,'Operator wait stays excluded after reopen.');
  const replacement=store.replaceAttempt(attempt.assignmentId,{automatic:false});store.claimAttempt(replacement.id);
  store.recordAttemptIdentity(replacement.id,{hostPid:201,ompPid:202,sessionId:'replacement',hostUrl:'http://127.0.0.1:9001',pane:'%1'});
  store.startExecution(run.id);store.recordAgentEvent(replacement.id,1,{type:'agent_start'});now+=100;store.recordAgentEvent(replacement.id,2,{type:'session_settled'});
  now+=100000;store.close();store=new Workbench(join(root,'workflow.sqlite'),()=>now);
  assert.equal(store.accountTime(run.id).activeMs,1200,'Settled replacement excludes CI wait despite the old lost tool-exit event.');
  store.recordOwnership(replacement.id,false,'Uncertain surviving child');now+=1100;
  const expired=store.accountTime(run.id);assert.equal(expired.activeMs,2300);assert.equal(expired.status,'pausing');assert.equal(expired.pauseReason,'budget');
  assert.equal(store.finishPause(run.id).status,'attention_required','Expiration never pretends uncertain ownership is stopped.');
});
