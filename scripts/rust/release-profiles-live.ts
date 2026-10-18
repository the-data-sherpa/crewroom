/** RTE-02 / MSG-01: actual native daemon + OMP + Mail; bounded scripted model replies. */
import assert from 'node:assert/strict';
import {existsSync,lstatSync,readFileSync,renameSync,writeFileSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {setTimeout as sleep} from 'node:timers/promises';
import {NativeDaemon} from './daemon-client';
import {join,dirname} from 'node:path';
import test from 'node:test';
import {randomUUID} from 'node:crypto';
import {workflowFixture,until} from './workflow-support';
import {helper,inspectOwned} from './process-adapter';
const live=JSON.parse(readFileSync(process.argv[2],'utf8'));
test(`native ${live.profile} selected team binds every process and completes four addressed Mail hops`,{timeout:live.observation?540000:120000},async t=>{
 const f=await workflowFixture(t,'research',3,false,{parent:live.root,service:true,configure({config,roles,team}){
  for(const role of roles){const selected=live.roles[role.id];role.model=selected.selector;role.effort=selected.effort;}
  for(const [i,member] of team.entries())member.roleId=live.selectedRoles[i];
  if(live.playbooks)writeFileSync(join(dirname(config.roles),'playbooks.json'),JSON.stringify([live.playbooks]),{mode:0o600});
  if(live.principles){
   for(const role of roles)if(live.selectedRoles.includes(role.id))Object.assign(role,{principles:[live.principles.id]});
   writeFileSync(join(dirname(config.roles),'principles.json'),JSON.stringify([live.principles]),{mode:0o600});
  }
  if(live.modelJobs){
   const model=live.roles.coordinator.selector;
   for(const role of roles)if(live.modelJobs[role.id]){Object.assign(role,{job:live.modelJobs[role.id],model:'',effort:'default'});}
   writeFileSync(join(dirname(config.roles),'model-jobs.json'),JSON.stringify({revision:1,budget:'large',jobs:Object.fromEntries(['code','judgment','review','fast'].map(job=>[job,model]))}),{mode:0o600});
  }
  writeFileSync(config.roles,JSON.stringify(roles),{mode:0o600});Object.assign(config.runtime,live.runtime);config.runtime.mail.projectRoot=live.root+'/mail-projects';
 }});
 assert.equal(f.c.proc.spawnfile,join(live.binaryDirectory,'crewroomd'));
 assert.equal(helper,join(live.binaryDirectory,'crewroom-process'));
 await f.start();
 let runtime=await until(async()=>{const r=await f.snapshot();return r.messages.length===4&&r.messages.every((m:any)=>m.disposition==='received')?r:false;},60000);
 runtime=await f.idle();
 // A selected specialist is a real idle process before receiving work. Give the
 // fourth role a harmless, authenticated fixture input to observe its wire model.
 const fourth=runtime.assignments.find((a:any)=>a.name==='Specialist3');
 await f.input(fourth.currentAttemptId,{kind:'profile-binding-probe',body:'Confirm the configured fixture identity.'},'native-profile-probe');
 await until(async()=>(await f.c.command('events',{target:f.run.id})).some((e:any)=>e.attemptId===fourth.currentAttemptId&&e.event.type==='prompt_result'&&e.event.id==='native-profile-probe'));
 runtime=await f.idle();assert.equal(runtime.assignments.length,4);assert.equal(runtime.attempts.length,4);
 assert.equal(new Set(runtime.attempts.map((a:any)=>a.hostPid)).size,4);assert.equal(new Set(runtime.attempts.map((a:any)=>a.ompPid)).size,4);
 assert.equal(new Set(runtime.attempts.flatMap((a:any)=>[a.hostPid,a.ompPid])).size,8);
 const identities=[];
 for(const a of runtime.assignments){
  const p=runtime.attempts.find((p:any)=>p.id===a.currentAttemptId),expected=live.roles[a.role.id];
  assert.ok(p.sessionId);assert.equal(a.model.resolved.selector,expected.selector);assert.equal(p.model.resolved.selector,expected.selector);
  const host=f.host(p.id),endpoint=JSON.parse(readFileSync(join(f.runtime,p.id,'endpoint.json'),'utf8'));
  const intent=JSON.parse(readFileSync(join(f.runtime,p.id,'host-intent.json'),'utf8'));
  assert.equal(intent.spec.command,join(live.binaryDirectory,'crewroom-host'));assert.equal(host.helper,helper);
  const response=await fetch(`http://127.0.0.1:${endpoint.port}`,{method:'POST',headers:{Authorization:`Bearer ${host.token}`},body:JSON.stringify({op:'identity'}),signal:AbortSignal.timeout(5000)});
  assert.ok(response.ok);const observed:any=await response.json();
  assert.equal(observed.model.provider,expected.provider);assert.equal(observed.model.id,expected.id);assert.equal(observed.model.api,expected.api);
  assert.equal(p.model.resolved.effort,expected.effort);assert.equal(observed.thinkingLevel,expected.effort);
  assert.equal(observed.hostPid,p.hostPid);assert.equal(observed.ompPid,p.ompPid);
  identities.push({name:a.name,mailboxName:a.mailboxName,role:a.role.id,assignmentId:a.id,attemptId:p.id,hostPid:p.hostPid,ompPid:p.ompPid,sessionId:p.sessionId,planned:p.model.resolved,observed});
 }
 const byName=new Map(runtime.assignments.map((a:any)=>[a.name,a]));
 const hops=[['Coordinator','Specialist1'],['Specialist1','Specialist2'],['Specialist2','Specialist1'],['Specialist1','Coordinator']];
 for(const [i,[from,to]] of hops.entries()){
  const m=runtime.messages[i],sender:any=byName.get(from),recipient:any=byName.get(to);
  assert.equal(m.body,`NATIVE_HOP_${i+1}`);assert.equal(m.from,sender.id);assert.equal(m.to,recipient.id);
  assert.equal(m.attemptId,sender.currentAttemptId);assert.equal(m.recipientAttemptId,recipient.currentAttemptId);
  assert.ok(m.transportId>0);assert.equal(m.disposition,'received');
  if(i>0)assert.equal(m.replyTo,runtime.messages[i-1].id);
 }
 const events=await f.c.command('events',{target:f.run.id});
 for(const m of runtime.messages)assert.ok(events.some((e:any)=>e.attemptId===m.recipientAttemptId&&e.event.type==='prompt_result'&&e.event.id===m.id),'Recipient completion must name the delivered message.');
 for(const name of ['Coordinator','Specialist1','Specialist2']){
  const a:any=byName.get(name);assert.ok(events.some((e:any)=>e.attemptId===a.currentAttemptId&&e.event.type==='tool_execution_end'&&e.event.toolName==='send_message'&&e.event.isError===false));
 }
 const before=runtime.messages;await f.c.command('events',{target:f.run.id});assert.deepEqual((await f.snapshot()).messages,before);
 if(live.principles){
  assert.ok(f.c instanceof NativeDaemon);
  for(const a of runtime.assignments)assert.deepEqual(a.brief.principles,[live.principles]);
  const changed=await f.c.request('/api/principles',{...live.principles,text:'PRINCIPLE_UPDATED: new assignments only.'});assert.equal(changed.revision,2);
  for(const a of (await f.snapshot()).assignments)assert.deepEqual(a.brief.principles,[live.principles]);
  const workspace=await f.c.request('/api/workspace');const role=workspace.roles.find((r:any)=>r.id===live.selectedRoles[0]);
  const preview=await f.c.request('/api/role-preview',{role,taskId:f.task.id});assert.ok(preview.prompt.includes('PRINCIPLE_UPDATED'));assert.ok(!preview.prompt.includes('PRINCIPLE_SNAPSHOT_ORIGINAL'));
 }
 let playbookSnapshot:any;
 if(live.playbooks){
  assert.ok(f.c instanceof NativeDaemon);const workspace=await f.c.request('/api/workspace');const run=workspace.runs.find((r:any)=>r.id===f.run.id);
  assert.equal(run.playbookProposal.playbook.id,live.playbooks.id);assert.equal(run.playbook,undefined);
  const request={proposalId:run.playbookProposal.id,revision:0,reason:'Accept the fixture checklist',requestId:randomUUID()};
  const accepted=await f.c.request(`/api/runs/${run.id}/playbook`,request);assert.equal(accepted.version,1);assert.ok(accepted.steeringId);
  await until(async()=>{const r=(await f.snapshot()).run;return r.playbook?.steps[0].state==='done'&&r.playbook?.steps[1].state==='skipped'?r:false;},60000);
  runtime=await f.idle();playbookSnapshot=runtime.run.playbook;assert.equal(playbookSnapshot.steps[1].reason,'Research fixture has no delivery target.');
  const events=await f.c.command('events',{target:run.id});assert.ok(events.some((e:any)=>e.event.type==='tool_execution_end'&&e.event.toolName==='record_playbook_step'&&e.event.isError===true),'Missing skip reason must be refused by the actual tool');
  await f.c.request('/api/playbooks',{...live.playbooks,steps:[{id:'changed',title:'Future runs only'}]});assert.deepEqual((await f.snapshot()).run.playbook,playbookSnapshot);
  assert.deepEqual(await f.c.request(`/api/runs/${run.id}/playbook`,request),accepted);
 }
 if(live.modelJobs){
  assert.ok(f.c instanceof NativeDaemon);const beforeModels=runtime.assignments.map((a:any)=>a.model);
  for(const a of runtime.assignments){assert.equal(a.role.model,'');assert.equal(a.role.effort,'default');assert.equal(a.model.job,live.modelJobs[a.role.id]);assert.equal(a.model.budget,'large');}
  const workspace=await f.c.request('/api/workspace');
  const settings=await f.c.request('/api/model-jobs',{...workspace.modelJobs,budget:'small'});
  assert.equal(settings.revision,2);assert.equal(settings.budget,'small');
  assert.deepEqual((await f.snapshot()).assignments.map((a:any)=>a.model),beforeModels);
  const changed=await f.c.request('/api/workspace');assert.equal(changed.roleCatalog.coordinator.model.resolved.effort,'medium');
 }
 if(live.observation)await observe(f,identities,events);
 await f.kill();await f.reopen();runtime=await f.idle();
 assert.deepEqual(runtime.attempts.map((a:any)=>[a.id,a.hostPid,a.ompPid,a.sessionId]),identities.map(a=>[a.attemptId,a.hostPid,a.ompPid,a.sessionId]));
 if(live.principles)for(const a of runtime.assignments)assert.deepEqual(a.brief.principles,[live.principles]);
 if(live.playbooks)assert.deepEqual(runtime.run.playbook,playbookSnapshot);
 assert.deepEqual(runtime.messages,before);await f.pause();assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));
 let roleDrafts:any;
 if(live.roleDrafts){
  assert.ok(f.c instanceof NativeDaemon);
  const beforeRoles=(await f.c.request('/api/workspace')).roles.length;
  const good=await f.c.request('/api/role-drafts',{description:'ROLE_DRAFT_SUCCESS: Review scientific charts',requestId:randomUUID()});
  const settled=await until(async()=>{const r=await f.c.request(`/api/runs/${good.id}/runtime`);return r.run.status==='closed'?r:false;},60000);
  assert.equal(settled.assignments.length,1);assert.ok(settled.attempts.every((a:any)=>a.ownership==='stopped'));
  const workspace=await f.c.request('/api/workspace');assert.equal(workspace.roles.length,beforeRoles);
  const proposal=workspace.refinementProposals.find((p:any)=>p.id===settled.run.roleDraft.proposalId);assert.ok(proposal.roleDraft);
  const role={...proposal.roleDraft,purpose:'Operator edited: scientific charts',draftProposalId:proposal.id};
  const saved=await f.c.request('/api/roles',role);const created=saved.find((r:any)=>r.id===role.id);
  assert.equal(created.purpose,role.purpose);assert.equal(created.history.length,1);assert.equal(created.history[0].proposalId,proposal.id);
  const bad=await f.c.request('/api/role-drafts',{description:'ROLE_DRAFT_INVALID: fail validation',requestId:randomUUID()});
  await until(async()=>{const events=await f.c.command('events',{target:bad.id});return events.some((e:any)=>e.event.type==='tool_execution_end'&&e.event.toolName==='propose_refinement'&&e.event.isError);},60000);
  const failed=await f.c.request(`/api/runs/${bad.id}/runtime`);assert.equal(failed.run.roleDraft.proposalId,undefined);
  await f.c.request(`/api/runs/${bad.id}/close`,{});assert.equal((await f.c.request('/api/workspace')).roles.length,beforeRoles+1);
  const learning=await f.c.request('/api/learning',{roleId:role.id,revision:1,mode:'teach',change:'Name the exact chart revision.',rationale:'Operator note from fixture review',requestId:randomUUID()});
  assert.equal(learning.origin,'operator');assert.equal(learning.runId,null);
  const decision={decision:'approve',reason:'Review completed',requestId:randomUUID()};const approved=await f.c.request(`/api/learning/${learning.id}/decision`,decision);assert.deepEqual(await f.c.request(`/api/learning/${learning.id}/decision`,decision),approved);
  const final=(await f.c.request('/api/workspace')).roles.find((r:any)=>r.id===role.id);assert.equal(final.revision,2);assert.equal(final.history.length,2);assert.equal(final.history[1].proposalId,learning.id);
  roleDrafts={actualCoordinatorProcesses:2,successStoppedAutomatically:true,manualEditBeforeSave:true,invalidProposalCreatedNoRole:true,operatorApprovalExactlyOnce:true};
 }
 const report={passed:true,roleDrafts,playbooks:live.playbooks?{coordinatorProposed:true,operatorSteeringDelivered:true,skipReasonEnforced:true,catalogEditRetainedCopy:true,restartRetainedChecklist:true}:undefined,principles:live.principles?{copiedAtAdmission:true,editRetainedAssignments:true,previewUpdated:true,restartRetainedCopies:true}:undefined,modelJobs:live.modelJobs?{inheritedJobs:true,liveBudgetEditPreservedAssignments:true}:undefined,profile:live.profile,binaryDirectory:live.binaryDirectory,adjacentDaemonHostAndHelperVerified:true,identities,messages:before,eventCount:events.length,reattachedSameEightProcesses:true,pausedAllStopped:true,boundary:'Selected legal four-agent research team from accepted profile; actual native daemon/OMP/Mail, scripted provider replies; not all 19 roles launched and no live provider delivery.'};
 writeFileSync(join(live.root,'profile-report.json'),JSON.stringify(report,null,2)+'\n');t.diagnostic(JSON.stringify({profile:live.profile,hops:4,agents:4,events:events.length}));
});


async function observe(f:Awaited<ReturnType<typeof workflowFixture>>,identities:any[],initialEvents:any[]){
 const options=live.observation,c=f.c;assert.ok(c instanceof NativeDaemon);
 const save=(name:string,value:any)=>{const path=join(live.root,name);writeFileSync(path+'.tmp',JSON.stringify(value,null,2)+'\n',{mode:0o600});renameSync(path+'.tmp',path);};
 const waitControl=async(name:string)=>{await until(()=>{const path=join(live.root,name);if(!existsSync(path))return false;const stat=lstatSync(path);assert.ok(stat.isFile()&&!stat.isSymbolicLink()&&stat.uid===process.getuid!(),'Control file must be a regular file owned by the fixture user');return true;},options.controlWaitMilliseconds);};
 try{
  for(const a of identities)for(const [name,pid] of [['host-process',a.hostPid],['omp-process',a.ompPid]] as const){const owned=await inspectOwned(join(f.runtime,a.attemptId,name));assert.equal(owned.state,'live');assert.equal(owned.owner.pid,pid);}
  const tmuxPid=Number(execFileSync('tmux',['-S',f.config.runtime.socket,'display-message','-p','#{pid}'],{encoding:'utf8'}).trim());assert.ok(Number.isInteger(tmuxPid)&&tmuxPid>1);
  const processes=[{label:'daemon',pid:c.proc.pid,binary:c.proc.spawnfile},{label:'mail',pid:options.mailPid,binary:options.mailBinary},{label:'tmux',pid:tmuxPid},...identities.flatMap(a=>[{label:`host-${a.name}`,pid:a.hostPid,attemptId:a.attemptId},{label:`omp-${a.name}`,pid:a.ompPid,attemptId:a.attemptId}])];
  for(const p of processes){assert.ok(p.pid&&p.pid>1);Object.assign(p,{observedIdentity:execFileSync('/bin/ps',['-p',String(p.pid),'-o','pid=,uid=,lstart=,command='],{encoding:'utf8'}).trim()});}
  save('observation-ready.json',{phase:'idle',recordedAt:new Date().toISOString(),descriptor:join(f.config.authority,'native-connection.json'),appArguments:['--connection',join(f.config.authority,'native-connection.json')],launcherArguments:['--data-dir',f.config.authority,'--roles',f.config.roles],dataDirectory:f.config.authority,rolePath:f.config.roles,taskId:f.task.id,runId:f.run.id,socket:f.config.runtime.socket,binaryDirectory:live.binaryDirectory,processes,identities,harness:{pythonPid:options.harnessPid,nodePid:process.pid},controls:{startLoad:join(live.root,'start-load'),loadStarted:join(live.root,'load-started.json'),loadComplete:join(live.root,'load-complete.json'),finishObservation:join(live.root,'finish-observation')},controlWaitMilliseconds:options.controlWaitMilliseconds,loadMilliseconds:options.loadMilliseconds,replyShape:{linesPerReply:24,targetChunks:12,chunkDelayMilliseconds:250},boundary:'Real production daemon and four actual OMP agents; scripted private provider replies. No Docker commands in this research workload. GUI PID must be separately observed by its launcher.'});
  await waitControl('start-load');
  const started=performance.now(),rounds:any[]=[];let cursor=Math.max(0,...initialEvents.map(e=>e.cursor));
  save('load-started.json',{startedAt:new Date().toISOString(),targetMilliseconds:options.loadMilliseconds});
  while(performance.now()-started<options.loadMilliseconds&&rounds.length<40){
   const number=rounds.length+1,requests=identities.map(a=>({attemptId:a.attemptId,name:a.name,id:`native-observe-${number}-${a.name}`}));
   await Promise.all(requests.map(a=>f.input(a.attemptId,{kind:'profile-observation',body:`NATIVE_OBSERVE_REQUEST round ${number}; emit only the harmless scripted Unicode reply.`},a.id)));
   const completions:any[]=[];
   await until(async()=>{
    for(;;){const page=await c.request(`/api/runs/${f.run.id}/events?limit=200&after=${cursor}`);cursor=page.after;
     for(const e of page.events){const request=requests.find(a=>a.id===e.event.id&&a.attemptId===e.attemptId);if(request&&e.event.type==='prompt_result'){assert.equal(e.event.status,'completed',JSON.stringify(e));assert.ok(!completions.some(c=>c.id===request.id),'Duplicate completion');completions.push({...request,event:e.event,cursor:e.cursor});}}
     if(!page.hasMore)break;
    }
    if(completions.length===4)return true;await sleep(100);return false;
   },30000);
   await f.idle();rounds.push({round:number,completedAt:new Date().toISOString(),completions});
   save('load-progress.json',{elapsedMilliseconds:performance.now()-started,rounds:rounds.length,completedInputs:rounds.length*4});
  }
  assert.ok(performance.now()-started>=options.loadMilliseconds,'Load reached round bound before its intended duration');
  const snapshot=await f.idle();assert.equal(snapshot.attempts.length,4);assert.deepEqual(snapshot.attempts.map((a:any)=>[a.hostPid,a.ompPid,a.sessionId]),identities.map(a=>[a.hostPid,a.ompPid,a.sessionId]));
  save('load-complete.json',{passed:true,completedAt:new Date().toISOString(),elapsedMilliseconds:performance.now()-started,roundCount:rounds.length,completedInputs:rounds.length*4,linesPerReply:24,expectedLinesPerAgent:rounds.length*24,rounds,errors:[],allFourAgentsIdle:true,boundary:'Harmless scripted Unicode replies through actual model gateways and OMP processes. Synthetic token counts are not real cost evidence.'});
  await waitControl('finish-observation');
 }catch(error){save('observation-error.json',{failedAt:new Date().toISOString(),error:error instanceof Error?{message:error.message,stack:error.stack}:String(error)});throw error;}
}
