/** Opt-in LIVE native harness through the full Rust workflow and a real external Mail server. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {randomUUID} from 'node:crypto';
import {spawnSync} from 'node:child_process';
import type {TestContext} from 'node:test';
import {workflowFixture,until} from './workflow-support';
import {NativeDaemon} from './daemon-client';

const setup=JSON.parse(readFileSync(process.argv[2],'utf8'));
const kind=setup.kind??'codex';assert.ok(['omp','codex','claude-code'].includes(kind));
const provider=kind==='omp'?setup.catalogProfile.provider:kind;
const runnerPid=(attempt:any)=>attempt.runnerPid??attempt.ompPid;
const cleanup:Array<()=>Promise<void>>=[];
const context={after:(f:()=>Promise<void>)=>cleanup.push(f),diagnostic:console.log} as TestContext;
const report:any={scope:`LIVE ${kind}, ${setup.catalogSelection?'native catalog picker and production Rust daemon':'full Rust workflow'}, real external Rust Mail`,passed:false,checks:[]};
let finishing:Promise<void>|undefined;
function finish(){return finishing??= (async()=>{
 for(const close of cleanup.reverse()){try{await close();}catch(error){report.passed=false;report.cleanupError=String(error);}}
 writeFileSync(join(setup.root,'workflow-result.json'),JSON.stringify(report,null,2)+'\n',{mode:0o600});
 console.log(JSON.stringify({...report,final:undefined},null,2));
})();}
process.once('SIGTERM',()=>{report.passed=false;report.error='Fixture exceeded its external time limit';void finish().then(()=>process.exit(1));});
try {
 const f=await workflowFixture(context,'research',1,false,{parent:setup.root,service:!!setup.catalogSelection,configure:({config,roles,team})=>{
  if(kind==='codex')config.runtime.codex={binary:setup.codex,authFile:setup.authFile};
  else if(kind==='claude-code')config.runtime.claudeCode={binary:setup.claude,home:setup.nativeHome};
  config.runtime.mail=setup.mail;
  delete config.runtime.models;delete config.runtime.ompBinary;delete config.runtime.launchPrefix;delete config.runtime.commandEngine;
  if(kind==='omp'){config.runtime.ompBinary=setup.omp;config.runtime.models=setup.ompModels;}
  config.runtime.catalog=[setup.catalogProfile??{adapter:kind,provider:kind,id:setup.model,...(kind==='claude-code'?{resolvedModel:setup.resolvedModel}:{}),api:kind==='codex'?'codex-app-server':'claude-code-stream',inputs:['text'],efforts:['low'],tools:true}];
  for(const role of roles){role.model=provider+'/'+setup.model;role.effort='low';role.instructions='This is a bounded acceptance fixture. Do not start research or finish the run unless the operator explicitly asks. Use no tools unless the operator explicitly asks for a named tool. Reply READY to initial assignment or resume context. Reply ACK to delivered peer messages without sending another message.';}
  for(const member of team)member.input='Reply READY. Do not call tools or finish the run. Wait for the next explicit operator instruction.';
  writeFileSync(config.roles,JSON.stringify(roles),{mode:0o600});
 }});
 report.fixtureRoot=f.root;
 if(setup.catalogSelection){
  const selection=`offering:${kind}:${encodeURIComponent(provider)}:${encodeURIComponent(setup.model)}`;
  const picker=join(setup.root,'picker.json'),receipt=join(setup.root,'picker-receipt.json');
  writeFileSync(picker,JSON.stringify({descriptor:join(f.root,'data/native-connection.json'),selection,receipt}),{mode:0o600});
  const selected=spawnSync('cargo',['test','--locked','-p','crewroom-desktop','-j6','native_catalog_external_live_selection','--','--ignored','--test-threads=1'],{encoding:'utf8',timeout:240000,env:{...process.env,CREWROOM_MH4_PICKER:picker}});
  writeFileSync(join(setup.root,'picker.log'),selected.stdout+selected.stderr,{mode:0o600});
  assert.equal(selected.status,0,`Native picker failed; see ${join(setup.root,'picker.log')}`);
  report.picker=JSON.parse(readFileSync(receipt,'utf8'));
  const workspace=await f.c.command('all');
  for(const member of f.team)Object.assign(member,{modelPreview:workspace.roleCatalog[member.roleId].model.selection.previewRevision});
  report.checks.push('native keyboard model picker, refresh, low effort, persisted Code job and coordinator/specialist selections');
 }
 await f.start();
 const idle=()=>until(async()=>{const s=await f.snapshot();const run=await f.c.command('run',{target:f.run.id});if(run.status==='attention_required')throw new Error(run.attentionReason);const events=await f.c.command('events',{target:f.run.id});return !(await f.c.command('busy',{target:f.run.id}))&&s.assignments.length===2&&s.assignments.every((a:any)=>s.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle'&&(!a.coordinator||events.some((e:any)=>e.attemptId===a.currentAttemptId&&e.event.type==='session_settled')))?s:false;},120000);
 let runtime=await idle();
 const active=()=>runtime.assignments.map((a:any)=>runtime.attempts.find((p:any)=>p.id===a.currentAttemptId));
 const before=active();assert.equal(new Set(before.flatMap((a:any)=>[a.hostPid,runnerPid(a)])).size,4);
 assert.ok(before.every((a:any)=>(a.runnerKind??'omp')===kind&&a.ownership==='held'));
 report.identities=before.map((a:any)=>({id:a.id,hostPid:a.hostPid,runnerPid:runnerPid(a),sessionId:a.sessionId,runnerProfile:a.runnerProfile,observedModel:a.observedModel,model:a.model}));
 const frozen=runtime.assignments.map((a:any)=>a.model);
 if(setup.catalogSelection){
  for(const model of frozen){assert.equal(model.selection.intendedModel,report.picker.selection);assert.equal(model.resolved.effort,'low');assert.equal(model.resolved.adapter,kind);}
  const daemon=f.c as NativeDaemon;
  const jobs=(await daemon.request('/api/workspace')).modelJobs;jobs.budget='unlimited';
  await daemon.request('/api/model-jobs',jobs);await daemon.request('/api/model-catalog/refresh',{});
  await until(async()=>(await daemon.request('/api/model-catalog')).state!=='refreshing',120000);
  assert.deepEqual((await f.snapshot()).assignments.map((a:any)=>a.model),frozen);
  report.checks.push('refresh and saved budget changes leave both executing assignments byte-for-byte equal');
 }
 report.checks.push('model-based route, native login, exact separately owned host/runner pairs');
 const coordinator=runtime.assignments.find((a:any)=>a.coordinator),recipient=runtime.assignments.find((a:any)=>!a.coordinator);
 const host=f.host(coordinator.currentAttemptId);const endpoint=JSON.parse(readFileSync(join(f.runtime,coordinator.currentAttemptId,'endpoint.json'),'utf8'));
 const marker='MAIL_'+randomUUID();const input=randomUUID();
 const response=await fetch(`http://127.0.0.1:${endpoint.port}`,{method:'POST',headers:{Authorization:`Bearer ${host.token}`},body:JSON.stringify({op:'input',id:input,message:`Call send_message exactly once with to=${JSON.stringify(recipient.name)} and body=${JSON.stringify(marker)}. Then reply SENT. Do not call other tools or finish the run.`}),signal:AbortSignal.timeout(30000)});
 assert.ok(response.ok,await response.text());
 await until(async()=>{const s=await f.snapshot();return s.messages.some((m:any)=>m.body===marker&&m.disposition==='received')?s:false;},120000);
 await idle();let messages=(await f.snapshot()).messages.filter((m:any)=>m.body===marker);assert.equal(messages.length,1);
 report.message={id:messages[0].id,disposition:messages[0].disposition};
 report.checks.push('real addressed host tool through workflow authorization and external Rust Mail to independent recipient');
 await f.kill();await f.reopen();runtime=await idle();
 const reattached=active();assert.deepEqual(reattached.map((a:any)=>[a.hostPid,runnerPid(a),a.sessionId]),before.map((a:any)=>[a.hostPid,runnerPid(a),a.sessionId]));
 assert.deepEqual(runtime.assignments.map((a:any)=>a.model),frozen);
 assert.equal((await f.snapshot()).messages.filter((m:any)=>m.body===marker).length,1);
 report.checks.push('controller SIGKILL and exact live-process reattachment without another Mail effect');
 await f.pause();const stopped=await f.snapshot();assert.ok(stopped.attempts.every((a:any)=>a.ownership==='stopped'));
 await f.resume();runtime=await idle();const resumed=active();
 assert.deepEqual(resumed.map((a:any)=>a.sessionId),before.map((a:any)=>a.sessionId));
 assert.ok(resumed.every((a:any)=>a.recoveryMode==='session'&&!before.some((b:any)=>runnerPid(b)===runnerPid(a))));
 assert.deepEqual(runtime.assignments.map((a:any)=>a.model),frozen);
 assert.equal((await f.snapshot()).messages.filter((m:any)=>m.body===marker).length,1);
 report.checks.push('pause verifies stopped ownership; resume uses new processes, same durable threads and original Mail effect');
 if(setup.catalogSelection){
  const prompt=async(attempt:string,message:string)=>{
   const host=f.host(attempt),endpoint=JSON.parse(readFileSync(join(f.runtime,attempt,'endpoint.json'),'utf8'));
   const response=await fetch(`http://127.0.0.1:${endpoint.port}`,{method:'POST',headers:{Authorization:`Bearer ${host.token}`},body:JSON.stringify({op:'input',id:randomUUID(),message}),signal:AbortSignal.timeout(30000)});
   assert.ok(response.ok,await response.text());
  };
  const specialist=runtime.assignments.find((a:any)=>!a.coordinator),lead=runtime.assignments.find((a:any)=>a.coordinator);
  const artifact={title:'Bounded model selection verification',kind:'research',body:'# Verification\nThe selected specialist received and acknowledged the addressed fixture message. The controller verified its frozen model and separate process identity.',citations:[],limitations:'Disposable model and message acceptance only; no repository research or Jira/PR delivery.',coverage:[{criterionId:0,section:'Verification'}]};
  await prompt(specialist.currentAttemptId,`Call submit_research_artifact exactly once with this JSON: ${JSON.stringify(artifact)}. Then reply RECORDED. Do not call other tools.`);
  await until(async()=>{
   try {const v=await f.c.command('research',{target:f.run.id});return v.assessment?.ready===true?v:false;}
   catch(error){if(String(error)==='Error: 409: Research evidence changed during verification.')return false;throw error;}
  },120000);
  await idle();
  await prompt(lead.currentAttemptId,'Call request_research_completion exactly once with {}. The bounded acceptance artifact is ready. Do not call other tools.');
  await until(async()=>{const s=await f.snapshot();return s.run.status==='completed'&&s.attempts.every((a:any)=>a.ownership==='stopped')?s:false;},120000);
  report.completion=await f.c.command('research',{target:f.run.id});
  await f.kill();await f.reopen();
  const completed=await f.snapshot();assert.equal(completed.run.status,'completed');assert.deepEqual(completed.assignments.map((a:any)=>a.model),frozen);assert.equal(completed.messages.filter((m:any)=>m.body===marker).length,1);
  report.checks.push('specialist submits addressed artifact; coordinator completion is durable after controller restart; no duplicate external Mail effect');
 } else await f.pause();
 report.final=await f.snapshot();report.passed=true;
} catch(error) {report.error=String(error);}
finally {
 await finish();
}
if(!report.passed)process.exitCode=1;
