import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {NativeDaemon} from './daemon-client';
import {workflowFixture,until} from './workflow-support';
import {checkImage} from './docker-engine';
/** Creates proof through authenticated workflow tools and actual Git/Docker. */
export async function prepareContributedPair(f:Awaited<ReturnType<typeof workflowFixture>>,additional:any[]=[]){
 const image=checkImage();const definitions=[{id:'required',image,command:'/bin/sh',args:['-c','test "$(cat source.txt)" = after'],timeoutMs:10000,memoryMiB:256,tmpExecutable:false,required:true},...additional];
 if(f.c instanceof NativeDaemon){const repo=(await f.c.request('/api/workspace')).repositories.find((r:any)=>r.id===f.repository.id);await f.c.request('/api/repositories/'+repo.id,{requestId:randomUUID(),revision:repo.revision,name:repo.name,path:repo.path,targetBranch:repo.targetBranch,checks:definitions});
 // The check configuration is part of the accepted repository basis. Explicitly
 // accept a fresh fixture plan and replace the unadmitted queued request.
 const task=await f.c.command('task',{target:f.task.id});
 const source=await f.c.command('plan_source',{target:task.id,input:{revision:task.revision}});
 const prior=(await f.c.request('/api/workspace')).intake[task.id].plans.at(-1);
 const plan=await f.c.command('plan_save',{target:task.id,input:{requestId:randomUUID(),revision:task.revision,sourceId:source.id,predecessorId:prior.id,content:prior.content}});
 await f.c.command('plan_decide',{target:plan.id,input:{requestId:randomUUID(),revision:task.revision,sourceId:source.id,decision:'accept'}});
 await f.c.request(`/api/queue/${f.run.id}/remove`,{requestId:randomUUID()});
 Object.assign(f.run,await f.c.request(`/api/tasks/${task.id}/runs`,{revision:task.revision,requestId:randomUUID()}));
}
 const work=await f.c.command('prepare',{target:f.run.id,input:{mode:'new'}});
 if(!(f.c instanceof NativeDaemon))await f.c.command('configure',{target:f.run.id,input:definitions});
 // New coding admission starts only the coordinator. The accepted packages
 // then bind the pair and documentation before any specialist is admitted.
 f.team.splice(1);
 await f.start();let runtime=await f.idle();let coordinator=runtime.assignments[0].currentAttemptId;
 const scheduled=(name:string,roleId:string,responsibility:string,stage:number,writes=false)=>({name,roleId,responsibility,stage,writes,packages:['fixture'],rationale:'Fixture '+responsibility,input:'Fulfil the '+responsibility+' obligation'});
 const schedule={uiRequired:false,diagnosisReason:null,assignments:[scheduled('Specialist1','implementer','writer',0,true),scheduled('Navigator','tech-writer','navigator',0),scheduled('Docs','tech-writer','documentation',0),scheduled('Specialist2','reviewer','reviewer',1)]};
 await f.tool(coordinator,'coding/choreography-propose',{revision:0,schedule},'schedule');
 const proposal=(names:string[],retire:string[]=[])=>({specialists:names.map(name=>{const m=schedule.assignments.find(m=>m.name===name)!;return {name,roleId:m.roleId,input:m.input,criteria:[{index:0,obligation:'Verify the accepted criterion',checkIds:[]}],dependsOn:[]};}),retire:retire.map(name=>({name,reason:'Contribution reconciled'}))});
 await f.tool(coordinator,'team-proposal',proposal(['Specialist1','Navigator','Docs']),'pair');
 runtime=await until(async()=>{const r=await f.idle();return r.assignments.length===4?r:false;});coordinator=runtime.assignments[0].currentAttemptId;
 const current=(name:string)=>runtime.assignments.find((a:any)=>a.name===name).currentAttemptId;
 const writer=current('Specialist1');
 const status=()=>f.tool(coordinator,'coding/choreography',{});
 const packageAction=async(who:string,kind:string,extra:any={})=>f.tool(who,'coding/package',{revision:(await status()).choreography.revision,packageId:'fixture',kind,summary:'Source transition examined',artifacts:[],...extra});
 await packageAction(coordinator,'start');

 await f.tool(coordinator,'coding/claim',{assignee:'Specialist1',instruction:'Apply the accepted change'},'claim');const source=await f.tool(writer,'coding/read',{path:'source.txt'});await f.tool(writer,'coding/edit',{path:'source.txt',expectedHash:source.sha256,content:'after\n'},'edit');
 const exchange=async(who:string,kind:string,to:string,extra:any={})=>{
  const result=await f.tool(who,'coding/pair-exchange',{revision:(await status()).choreography.revision,packageId:'fixture',kind,to,artifact:'source.txt',body:'The exact output requires a trailing newline; check the byte boundary.',proposedChange:'Assert the complete source content.',...extra});
  await until(async()=>(await f.snapshot()).messages.find((m:any)=>m.id===result.message.id)?.disposition==='received');await f.idle();return result.exchange;
 };
 await exchange(writer,'approach','Navigator');const challenge=await exchange(current('Navigator'),'challenge','Specialist1');const response=await exchange(writer,'response','Navigator',{challengeId:challenge.id});await exchange(coordinator,'resolution','Navigator',{challengeId:challenge.id,responseId:response.id});
 await packageAction(writer,'report');await packageAction(current('Navigator'),'report');await packageAction(current('Docs'),'report',{documentation:{disposition:'no_change',reason:'Internal fixture source has no public documentation.'}});await packageAction(coordinator,'complete');
 return {work,coordinator,writer,proposal};
}
export async function reviewedCandidate(f:Awaited<ReturnType<typeof workflowFixture>>,additional:any[]=[]){
 let {coordinator,writer,proposal}=await prepareContributedPair(f,additional);let runtime:any;
 const completion=await f.tool(writer,'coding/complete',{summary:'Update the source'},'complete');
 await until(async()=>{const record=await f.c.command('record',{target:f.run.id});return record.completions.find((c:any)=>c.id===completion.id)?.state==='completed';});
 runtime=await until(async()=>{const r=await f.snapshot();return !(await f.c.command('busy',{target:f.run.id}))&&r.assignments[0].currentAttemptId!==coordinator&&r.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;});coordinator=runtime.assignments[0].currentAttemptId;await f.tool(coordinator,'team-proposal',proposal(['Specialist2'],['Specialist1','Navigator','Docs']),'review-stage');runtime=await until(async()=>{const r=await f.idle();return r.assignments.some((a:any)=>a.name==='Specialist2')?r:false;});coordinator=runtime.assignments[0].currentAttemptId;const reviewer=runtime.assignments.find((a:any)=>a.name==='Specialist2').currentAttemptId;
 const record=await f.c.command('record',{target:f.run.id});assert.equal(record.candidates.length,1);const candidate=record.candidates[0];assert.deepEqual(candidate.implementationAttempts,[writer]);assert.equal((await f.c.command('evidence',{target:f.run.id})).ready,false);
 await assert.rejects(f.tool(coordinator,'coding/review-assign',{assignee:'Specialist1',candidateId:candidate.id,instruction:'Self review'},'self-review'),/independent|admitted specialist/);
 await f.input(reviewer,{kind:'tool',name:'run_check',toolCallId:'actual-check',arguments:{checkId:'required'}},'check-prompt');await until(async()=>{const r=await f.c.command('record',{target:f.run.id});return r.checks.length===1;},45000).catch(async error=>{throw new Error(error.message+'; '+JSON.stringify({record:await f.c.command('record',{target:f.run.id}),runtime:await f.snapshot()}));});await f.idle();
 const checked=await f.c.command('record',{target:f.run.id});assert.equal(checked.checks[0].candidateId,candidate.id);assert.equal(checked.checks[0].exitCode,0,JSON.stringify(checked.checks[0]));assert.equal(checked.checks[0].sourceIntact,true);
 await f.tool(coordinator,'coding/review-assign',{assignee:'Specialist2',candidateId:candidate.id,instruction:'Review exact source'},'review');await f.tool(reviewer,'coding/review',{candidateId:candidate.id,verdict:'pass',summary:'Exact source and check passed',findings:[],coverage:[{criterion:0,assessment:'Exact source verified',checkIds:['required']}]},'review-result');assert.equal((await f.c.command('evidence',{target:f.run.id})).ready,true);
 return {candidate,coordinator,reviewer};
}

/** Scripted MH6 rework after existing check/review or explicit PR Resume authority. */
export async function reworkedCandidate(f:Awaited<ReturnType<typeof workflowFixture>>,change:(writer:string)=>Promise<void>){
 let runtime=await f.idle();const current=(name:string)=>runtime.assignments.find((a:any)=>a.name===name&&!a.retiredAt)?.currentAttemptId;
 const coordinator=()=>runtime.assignments.find((a:any)=>a.coordinator).currentAttemptId;
 const status=()=>f.tool(coordinator(),'coding/choreography',{});
 const prior=(await status()).choreography;
 const schedule=structuredClone(prior.schedule);for(const m of schedule.assignments)m.name+='Feedback';
 async function team(names:string[],retire:string[]){
  const old=coordinator();await f.tool(old,'team-proposal',{specialists:names.map(name=>{const m=schedule.assignments.find((m:any)=>m.name===name);return {name,roleId:m.roleId,input:m.input,criteria:[{index:0,obligation:'Cover the adopted rework',checkIds:['required']}],dependsOn:[]};}),retire:retire.map(name=>({name,reason:'Prior stage reconciled'}))});
  runtime=await until(async()=>{const r=await f.idle();return r.assignments.find((a:any)=>a.coordinator).currentAttemptId!==old&&names.every(name=>r.assignments.some((a:any)=>a.name===name&&!a.retiredAt))?r:false;});
 }
 await team([],runtime.assignments.filter((a:any)=>!a.coordinator&&!a.retiredAt).map((a:any)=>a.name));
 await f.tool(coordinator(),'coding/choreography-propose',{revision:(await status()).choreography.revision,schedule,reopenPackages:['fixture']});
 const by=(responsibility:string)=>schedule.assignments.find((m:any)=>m.responsibility===responsibility).name;
 const writer=by('writer'),navigator=by('navigator'),docs=by('documentation'),reviewer=by('reviewer');
 await team([writer,navigator,docs],[]);
 await f.tool(coordinator(),'coding/claim',{assignee:writer,instruction:'Implement the explicitly authorized current rework'});
 const action=async(who:string,kind:string,extra:any={})=>f.tool(who,'coding/package',{revision:(await status()).choreography.revision,packageId:'fixture',kind,summary:'Reconcile the new feedback artifact',artifacts:[],...extra});
 await action(coordinator(),'reopen');await change(current(writer));
 const exchange=async(name:string,kind:string,to:string,extra:any={})=>{const v=await f.tool(current(name),'coding/pair-exchange',{revision:(await status()).choreography.revision,packageId:'fixture',kind,to,artifact:'feedback.txt',body:'Preserve the original checked behavior and cover the additional feedback artifact.',proposedChange:'Verify source.txt still contains after and inspect the added regression artifact.',...extra});await until(async()=>(await f.snapshot()).messages.find((m:any)=>m.id===v.message.id)?.disposition==='received');await f.idle();return v.exchange;};
 await exchange(writer,'approach',navigator);const challenge=await exchange(navigator,'challenge',writer);const response=await exchange(writer,'response',navigator,{challengeId:challenge.id});await exchange('Coordinator','resolution',navigator,{challengeId:challenge.id,responseId:response.id});
 for(const name of [writer,navigator])await action(current(name),'report');
 await action(current(docs),'report',{documentation:{disposition:'no_change',reason:'This isolated regression artifact has no public documentation impact.'}});await action(coordinator(),'complete');
 const before=coordinator();const completion=await f.tool(current(writer),'coding/complete',{summary:'Cover retained feedback with renewed package evidence'});
 await until(async()=>(await f.c.command('record',{target:f.run.id})).completions.some((c:any)=>c.id===completion.id&&c.state==='completed'),45000);
 runtime=await until(async()=>{const r=await f.idle();return r.assignments.find((a:any)=>a.coordinator).currentAttemptId!==before?r:false;});
 await team([reviewer],[writer,navigator,docs]);
 return {coordinator:coordinator(),reviewer:current(reviewer),reviewerName:reviewer,candidate:(await f.c.command('record',{target:f.run.id})).candidates.at(-1)};
}
