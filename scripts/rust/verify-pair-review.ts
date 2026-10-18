/** Live review recovery on an unchanged MH6 candidate; no source/evidence fabrication. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,existsSync,readdirSync} from 'node:fs';
import {join} from 'node:path';
import {randomUUID,createHash} from 'node:crypto';
import type {TestContext} from 'node:test';
import {NativeDaemon} from './daemon-client';
import {until} from './workflow-support';
import {nativeUi} from './native-ui-driver';
import {inspectOwned,inspectCheckedOwned} from './process-adapter';
const setup=JSON.parse(readFileSync(process.argv[2],'utf8'));
const prior=JSON.parse(readFileSync(join(setup.seed,'pair-result.json'),'utf8'));
const runtimeRoot=join(setup.fixture,'runtime');
const runtimeConfig=JSON.parse(readFileSync(join(setup.fixture,'data/native-runtime.json'),'utf8'));
runtimeConfig.mail=setup.mail;
const client=new NativeDaemon({authority:join(setup.fixture,'data'),roles:join(setup.fixture,'roles.json'),runtime:runtimeConfig});
const report:any={passed:false,scope:'Actual same-candidate live revalidation after container-owner repair',priorCandidate:prior.candidate,binaries:JSON.parse(readFileSync(join(setup.output,'binaries.json'),'utf8'))};
const runId=prior.candidate.runId;
const context={diagnostic:console.log} as TestContext;
let finalRuntime:any,record:any;
let candidate=prior.candidate;
report.steps=[];
async function state(){return client.command('snapshot',{target:runId});}
async function idle(){return until(async()=>{const r=await state();if(r.run.status==='attention_required')throw new Error(r.run.attentionReason);return !r.busy&&r.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;},180000);}
try{
 await client.ready;const before=await state();report.before=before;
 const ui=(action:string,extra:any={})=>nativeUi(context,setup.fixture,{descriptor:join(setup.fixture,'data/native-connection.json'),taskId:before.run.taskId,runId,action,...extra});
 await ui('resume');await idle();
 record=await client.command('record',{target:runId});assert.deepEqual(record.candidates,[prior.candidate]);
 assert.ok(!(await state()).packageWriter);
 async function ask(name:string,instruction:string,done:()=>Promise<boolean>){
  await idle();const assignment=(await state()).assignments.find((a:any)=>a.name===name&&!a.retiredAt);assert.ok(assignment,name);
  const attempt=assignment.currentAttemptId;
  const host=JSON.parse(readFileSync(join(runtimeRoot,attempt,'host.json'),'utf8'));
  const endpoint=JSON.parse(readFileSync(join(runtimeRoot,attempt,'endpoint.json'),'utf8'));
  const inputId=randomUUID();console.log('STEP '+name+': '+instruction.slice(0,160));
  const response=await fetch(`http://127.0.0.1:${endpoint.port}`,{method:'POST',headers:{Authorization:`Bearer ${host.token}`},body:JSON.stringify({op:'input',id:inputId,message:JSON.stringify({instruction:'STEP: '+instruction})}),signal:AbortSignal.timeout(15000)});assert.ok(response.ok,await response.text());
  report.steps.push({attemptId:attempt,inputId,instruction});
  await until(done,240000);await idle();
 }
 const rec=async()=>client.command('record',{target:runId});
 if(setup.repairUi){
  report.scope='Actual UI repair after independent changes-requested verdict, fresh pair generation and documentation handoff';
  assert.equal(prior.job,'ui');assert.equal(record.reviews.at(-1).verdict,'changes_requested');
  const members=structuredClone((await state()).choreography.schedule.assignments);
  for(const member of members)member.name+='Repair';
  const schedule={...(await state()).choreography.schedule,assignments:members};
  async function team(names:string[],retire:string[]=[]){
   const proposal={specialists:names.map(name=>{const m=members.find((a:any)=>a.name===name);return {name,roleId:m.roleId,input:m.input,dependsOn:[],criteria:[{index:0,obligation:before.run.requirementSnapshot?.brief??'Repair the accessible responsive people filter and preserve all accepted behavior.',checkIds:['required']}]};}),retire:retire.map(name=>({name,reason:'Retire reconciled previous generation for requested repair'}))};
   await ask('Coordinator','Call propose_team with '+JSON.stringify(proposal)+'. Stop.',async()=>{const r=await state();return names.every(name=>r.assignments.some((a:any)=>a.name===name&&!a.retiredAt))&&retire.every(name=>r.assignments.find((a:any)=>a.name===name)?.retiredAt);});
  }
  await team([],['Review']);
  await ask('Coordinator','Read choreography_status, then propose_choreography with current revision, reopenPackages:["fixture"], schedule:'+JSON.stringify(schedule)+'. The retained review requires a fresh repair generation. Stop.',async()=>(await state()).choreography.schedule.assignments.some((a:any)=>a.name==='WriterRepair'));
  await team(['WriterRepair','NavigatorRepair','InterfaceRepair']);
  await ask('Coordinator','Claim implementation for WriterRepair to address the retained independent review, then record_package kind=reopen for fixture using current choreography revision, summary describing the empty-state accessibility repair and artifacts=[]. Stop.',async()=>{const r=await state();return !!r.packageWriter&&r.choreography.packages[0].generation===2;});
  await ask('WriterRepair','Inspect the retained source, test, independent-review findings and README. Send pair_exchange kind=approach to NavigatorRepair for fixture, explaining a concrete fix to use one persistent live empty-state region with initial silence and no duplicate exact message. Keep trimmed live filtering, Escape/focus and mobile behavior. Stop without editing.',async()=>(await state()).choreography.exchanges.some((e:any)=>e.generation===2&&e.kind==='approach'));
  await ask('NavigatorRepair','Independently inspect source and the proposed accessibility repair. Send pair_exchange kind=challenge to WriterRepair for fixture with a concrete edge case/regression and source artifact. Stop.',async()=>(await state()).choreography.exchanges.some((e:any)=>e.generation===2&&e.kind==='challenge'));
  await ask('WriterRepair','Read the navigator challenge; repair index.html and executable tests using write_file. Make #empty the single live region (role=status, aria-live=polite), keep initial silence and avoid duplicate exact text. Preserve all accepted behavior. Read /opt/check-ui.cjs using shell if needed; do not weaken the required check. Run the real browser check with run_shell with command="/usr/local/bin/node /opt/check-ui.cjs". Inspect actual output and repair until it passes. Send pair_exchange kind=response to NavigatorRepair with challengeId and actual change/check evidence plus source artifact. Do not checkpoint or freeze. Stop.',async()=>(await state()).choreography.exchanges.some((e:any)=>e.generation===2&&e.kind==='response'));
  await ask('Coordinator','Inspect current source, generation-2 challenge and response. Resolve the concrete concern using pair_exchange kind=resolution to NavigatorRepair for fixture with challengeId,responseId and source artifact. Stop.',async()=>(await state()).choreography.exchanges.some((e:any)=>e.generation===2&&e.kind==='resolution'));
  for(const name of ['WriterRepair','NavigatorRepair','InterfaceRepair'])await ask(name,'Independently inspect current source and resolved challenge. '+(name==='InterfaceRepair'?'Run actual browser check using run_shell with command="/usr/local/bin/node /opt/check-ui.cjs", inspect output and report all 1280/390 filtering, empty announcement, Escape/focus and overflow results honestly. ':'')+'Record_package kind=report for fixture using current revision, meaningful findings and current repository_read path/sha256 artifacts. Stop.',async()=>(await state()).choreography.reports.some((p:any)=>p.name===name&&p.generation===2));
  const count=(await rec()).completions.length;
  await ask('WriterRepair','Checkpoint_implementation with summary of repaired behavior and successor DocsRepair. Stop immediately; do not freeze.',async()=>(await rec()).completions.slice(count).some((c:any)=>c.mode==='checkpoint'&&c.state==='completed'));
  await team(['DocsRepair'],['WriterRepair','NavigatorRepair','InterfaceRepair']);
  await ask('Coordinator','Claim implementation for DocsRepair to update usage and validation documentation after the source repair. Stop.',async()=>!!(await state()).packageWriter);
  await ask('DocsRepair','Inspect current source and actual browser reports. Update README.md to describe repaired behavior and observed verification, retaining a brief history of the superseded failed candidate. Remove claims that the resolved defect is still present, but do not invent checks. Record_package kind=report for fixture with current README path/sha256 and documentation {disposition:"changed",reason:<specific impact>}. Do not freeze. Stop.',async()=>(await state()).choreography.reports.some((p:any)=>p.name==='DocsRepair'&&p.generation===2));
  await ask('Coordinator','Inspect current reports and artifacts. Record_package kind=complete for fixture using current revision, meaningful summary and artifacts=[]. Stop.',async()=>(await state()).choreography.packages[0].state==='complete');
  await ask('DocsRepair','Finish_implementation with concise repair and documentation summary. End immediately and let Crewroom freeze a fresh candidate.',async()=>(await rec()).candidates.length===2);
  candidate=(await rec()).candidates.at(-1);report.repairedCandidate=candidate;
  assert.notEqual(candidate.id,prior.candidate.id);assert.notEqual(candidate.revision.head,prior.candidate.revision.head);
  await team(['ReviewRepair'],['DocsRepair']);
  await ask('Coordinator','Assign independent review to ReviewRepair for candidate '+candidate.id+' with instructions to inspect repaired source, tests, documentation and all criterion 0 behaviors. Stop.',async()=>(await rec()).reviewAssignments.some((a:any)=>a.candidateId===candidate.id));
 }
 const oldReviews=(await rec()).reviews.length;
 await ask(setup.repairUi?'ReviewRepair':'Review',`Independently inspect exact candidate ${candidate.id}, tests and documentation. Prior failed evidence remains retained. Crewroom now pins the local snapshot owner's UID/GID while preserving CapDrop=ALL and private modes. Call run_check checkId=required with a new tool call, inspect its actual result, then submit_review for this exact candidate using your honest verdict, concrete findings and criterion 0 coverage with checkIds:["required"]. Do not edit source or weaken validation. Stop.`,async()=>(await rec()).reviews.length>oldReviews);
 record=await rec();report.assessment=await client.command('evidence',{target:runId});report.record=record;
 assert.equal(report.assessment.ready,true,JSON.stringify(report.assessment));assert.equal(record.candidates.at(-1).id,candidate.id);
 finalRuntime=await state();assert.ok(!finalRuntime.packageWriter);
 assert.ok(finalRuntime.messages.every((m:any)=>m.disposition==='received'));
 for(const width of [1280,390])await ui('packages',{width,labels:['Transferred · Successor claim recorded','Documentation: changed','Delivery: received']});
 const events=await client.command('events',{target:runId});writeFileSync(join(setup.output,'agent-events.json'),JSON.stringify(events,null,2)+'\n',{mode:0o600});
 report.eventsSha256=createHash('sha256').update(JSON.stringify(events)).digest('hex');report.finalRuntime=finalRuntime;
 await client.command('pause',{target:runId});report.passed=true;
}catch(error){report.error=error instanceof Error?error.stack:String(error);}
finally{
 try{await client.close();const ownership=[];
  for(const name of readdirSync(runtimeRoot)){const dir=join(runtimeRoot,name);if(!existsSync(join(dir,'host-intent.json')))continue;
   const descriptor=JSON.parse(readFileSync(join(dir,'host-intent.json'),'utf8')).descriptor;
   assert.equal((await inspectCheckedOwned(descriptor)).state,'stopped');
   const host=JSON.parse(readFileSync(join(dir,'host.json'),'utf8'));const adapter=host.model?.resolved?.adapter;
   const runner=adapter==='codex'?'codex-process':adapter==='claude-code'?'claude-process':'omp-process';
   assert.equal((await inspectOwned(join(dir,runner))).state,'stopped');ownership.push(name);
  }report.cleanup={allOwnedProcessesStopped:true,attemptIds:ownership};
 }catch(error){report.passed=false;report.cleanupError=String(error);}
 writeFileSync(join(setup.output,'result.json'),JSON.stringify(report,null,2)+'\n',{mode:0o600});
 if(report.passed){const combined={...prior,passed:true,candidate,initialValidationError:prior.error,reviewRecovery:setup.output,assignments:finalRuntime.assignments,attempts:finalRuntime.attempts,choreography:finalRuntime.choreography,messages:finalRuntime.messages,checks:record.checks,reviews:record.reviews,assessment:report.assessment,cleanup:report.cleanup};delete combined.error;writeFileSync(join(setup.seed,'pair-result-validated.json'),JSON.stringify(combined,null,2)+'\n',{mode:0o600});}
 console.log(JSON.stringify({passed:report.passed,error:report.error,cleanup:report.cleanup,output:setup.output}));
}
if(!report.passed)process.exitCode=1;
