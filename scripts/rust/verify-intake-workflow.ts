/** Opt-in actual inference, production daemon and external Rust Mail. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {randomUUID} from 'node:crypto';
import type {TestContext} from 'node:test';
import {NativeDaemon} from './daemon-client';
import {workflowFixture,until} from './workflow-support';
const setup=JSON.parse(readFileSync(process.argv[2],'utf8'));
const kind=setup.kind,provider=kind==='omp'?setup.catalogProfile.provider:kind;
const cleanup:Array<()=>Promise<void>>=[];
const context={after:(f:()=>Promise<void>)=>cleanup.push(f),diagnostic:console.log} as TestContext;
const report:any={scope:'LIVE intake inference; production Rust daemon and external Rust Mail; no publication or Jira',kind,passed:false,checks:[]};
let finishing:Promise<void>|undefined;
function finish(){return finishing??=(async()=>{for(const close of cleanup.reverse())try{await close();}catch(error){report.passed=false;report.cleanupError=String(error);}writeFileSync(join(setup.root,'intake-result.json'),JSON.stringify(report,null,2)+'\n',{mode:0o600});console.log(JSON.stringify(report,null,2));})();}
process.once('SIGTERM',()=>{report.error='External time limit';report.passed=false;void finish().then(()=>process.exit(1));});
try {
 const f=await workflowFixture(context,'explore',0,true,{parent:setup.root,service:true,configure({config,roles}){
  if(kind==='codex')config.runtime.codex={binary:setup.codex,authFile:setup.authFile};
  if(kind==='claude-code')config.runtime.claudeCode={binary:setup.claude,home:setup.nativeHome};
  config.runtime.mail=setup.mail;delete config.runtime.models;delete config.runtime.ompBinary;delete config.runtime.launchPrefix;
  if(kind==='omp'){config.runtime.ompBinary=setup.omp;config.runtime.models=setup.ompModels;}
  config.runtime.catalog=[setup.catalogProfile??{adapter:kind,provider:kind,id:setup.model,...(kind==='claude-code'?{resolvedModel:setup.resolvedModel}:{}),api:kind==='codex'?'codex-app-server':'claude-code-stream',inputs:['text'],efforts:['low'],tools:true}];
  for(const role of roles){role.model=provider+'/'+setup.model;role.effort='low';role.instructions='Bounded intake acceptance fixture. On initial assignment call repository_read for source.txt, then ask_intake_questions exactly once using sourceId from your intake basis and one question: id=output, prompt="Should the output be text or JSON?", rationale="The representation determines implementation", choices=["Text","JSON"]. Stop after the question. After the operator answer is delivered, call repository_read for source.txt again then propose_implementation_plan with the same sourceId. Use schemaVersion=1, a goal, nonempty scope, exclusions/assumptions/decisions arrays, workPackages with id,name,dependsOn,outputs,criteria:[0],validation. Propose requirements with brief="Return the chosen output" and criteria=["The chosen output is checked"]. Use the operator answer in decisions. Do not ask again or implement. Stop after the proposal. If asked to apply a steering receipt, apply it.';}
  writeFileSync(config.roles,JSON.stringify(roles),{mode:0o600});
 }});
 report.fixtureRoot=f.root;const client=()=>f.c as NativeDaemon;
 const task=await client().request('/api/tasks',{title:'Choose fixture output',description:'The representation has not been chosen',repositoryId:f.repository.id});
 const queued=await client().request(`/api/tasks/${task.id}/runs`,{revision:1,requestId:randomUUID()});assert.equal(queued.purpose,'grill');
 const state=()=>client().request(`/api/runs/${queued.id}/runtime`);
 const view=async()=>(await client().request('/api/workspace')).intake[task.id];
 const waiting=await until(async()=>{const v=await view();if(v.intake.holdReason)throw new Error(v.intake.holdReason);const s=await state();if(s.run.status==='attention_required'&&!['waiting','review','handoff'].includes(v.intake.state))throw new Error(s.run.attentionReason);return v.state==='Waiting for you'?v:false;},180000);
 const before=await state();assert.equal(before.assignments.length,1);assert.ok(before.attempts.every((a:any)=>a.ownership==='stopped'));
 const first=before.attempts[0];assert.equal(first.runnerKind??'omp',kind);assert.equal(waiting.intake.questions.length,1);assert.ok(waiting.intake.inspection);
 report.initial={attemptId:first.id,hostPid:first.hostPid,runnerPid:first.runnerPid??first.ompPid,sessionId:first.sessionId,model:first.model,observedModel:first.observedModel};
 report.source=queued.intakeSource;delete report.source.instructions;
 await f.kill();await f.reopen();assert.equal((await view()).state,'Waiting for you');assert.equal((await state()).run.activeMs,before.run.activeMs);
 report.checks.push('actual harness inspection and durable question; verified stopped ownership; restart preserves wait without active-time charge');
 const resumedView=await view();
 const answer={requestId:randomUUID(),revision:resumedView.intake.revision,sourceId:queued.intakeSource.id,answers:[{id:resumedView.intake.questions[0].id,body:'Text. Keep the implementation bounded.'}]};
 const answered=await client().request(`/api/intakes/${waiting.intake.id}/answers`,answer);assert.deepEqual(await client().request(`/api/intakes/${waiting.intake.id}/answers`,answer),answered);
 const review=await until(async()=>{const v=await view();if(v.intake.holdReason)throw new Error(v.intake.holdReason);const s=await state();if(s.run.status==='attention_required'&&!['waiting','review','handoff'].includes(v.intake.state))throw new Error(s.run.attentionReason);return v.state==='Plan review'?v:false;},180000);
 const after=await state();assert.ok(after.attempts.every((a:any)=>a.ownership==='stopped'));assert.equal(after.assignments[0].id,before.assignments[0].id);
 const plan=review.plans.at(-1);assert.equal(plan.author.kind,'coordinator');
 report.plan={id:plan.id,version:plan.version,sha256:plan.sha256,basis:plan.basis.id,content:plan.content};
 report.recovery=after.attempts.map((a:any)=>({id:a.id,sessionId:a.sessionId,recoveryMode:a.recoveryMode,ownership:a.ownership,runnerKind:a.runnerKind??'omp'}));
 const messages=(await client().request('/api/workspace')).messages.filter((m:any)=>m.id===answered.pendingMessageId);assert.equal(messages.length,1);assert.equal(messages[0].delivery,'received');report.answerReceipt=messages[0].id;
 const decision={requestId:randomUUID(),revision:1,sourceId:queued.intakeSource.id,decision:'accept'};
 const accepted=await client().request(`/api/implementation-plans/${plan.id}/decision`,decision);assert.deepEqual(await client().request(`/api/implementation-plans/${plan.id}/decision`,decision),accepted);
 await f.kill();await f.reopen();
 const coding=await until(async()=>{const w=await client().request('/api/workspace');const jobs=w.runs.filter((r:any)=>r.taskId===task.id&&r.purpose==='coding');return jobs.length===1?jobs[0]:false;},30000);
 assert.equal(coding.implementationPlan.id,plan.id);assert.equal(coding.taskRevision,2);assert.equal(coding.status,'awaiting_runner');assert.equal((await state()).run.status,'closed');
 assert.equal((await client().request(`/api/runs/${coding.id}/runtime`)).attempts.length,0);
 report.codingRun={id:coding.id,implementationPlanId:plan.id,taskRevision:coding.taskRevision,admission:'Held for explicit launch details'};
 report.checks.push('actual resumed harness proposes valid plan using delivered answer; keyed acceptance; restart yields exactly one coding successor and preserves launch gate');
 report.passed=true;
}catch(error){report.error=String(error);}
finally{await finish();}
if(!report.passed)process.exitCode=1;
