import assert from 'node:assert/strict';
import test from 'node:test';
import {writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {workflowFixture,until} from './workflow-support';

// Production daemon, real supervised OS processes/Git and synthetic inference/Mail.
// Live provider acceptance is retained separately; this fixture tests the protocol.
for (const phase of ['unproposed','proposed','admitted']) test(`MH6 ${phase} operator adoption requires a current accepted plan and saves its binding`, {timeout:180000}, async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true,configure({root,roles}){
  roles.push({...roles.find(r=>r.id==='tech-writer')!,id:'navigator',name:'Navigator'});
  writeFileSync(join(root,'roles.json'),JSON.stringify(roles));
 }});
 await f.c.command('prepare',{target:f.run.id,input:{mode:'new'}});
 await f.start();
 const idle=()=>until(async()=>{const r=await f.snapshot();return !(await f.c.command('busy',{target:f.run.id}))&&r.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;},45000);
 let runtime=await idle();
 const id=(name:string)=>runtime.assignments.find((a:any)=>a.name===name).currentAttemptId;
 const status=()=>f.tool(id('Coordinator'),'coding/choreography',{});
 const member=(name:string,roleId:string,responsibility:string,stage:number,writes=false)=>({name,roleId,responsibility,stage,writes,packages:['fixture'],rationale:'Exercise '+responsibility,input:'Fulfil '+responsibility+' responsibility'});
 const schedule={uiRequired:false,diagnosisReason:null,assignments:[member('Writer','implementer','writer',0,true),member('Navigator','navigator','navigator',0),member('Docs','tech-writer','documentation',1,true),member('Review','reviewer','reviewer',2)]};
 if(phase!=='unproposed') await f.tool(id('Coordinator'),'coding/choreography-propose',{revision:0,schedule},'schedule');
 const team=async(names:string[],retire:string[]=[])=>{
  const prior=id('Coordinator');
  await f.tool(prior,'team-proposal',{specialists:names.map(name=>{const m=schedule.assignments.find(m=>m.name===name)!;return {name,roleId:m.roleId,input:m.input,criteria:[0,1].slice(0,name.endsWith('Rebriefed')?2:1).map(index=>({index,obligation:'Verify accepted criterion',checkIds:[]})),dependsOn:[]};}),retire:retire.map(name=>({name,reason:'Package contribution reconciled'}))});
  runtime=await until(async()=>{const r=await idle();return r.assignments.find((a:any)=>a.name==='Coordinator').currentAttemptId!==prior&&names.every(name=>r.assignments.some((a:any)=>a.name===name&&!a.retiredAt))?r:false;},45000);
 };
 if(phase==='admitted') await team(['Writer','Navigator']);
 let packageIds=['fixture'];
 const action=async(name:string,kind:string,extra:any={})=>{const state=await status();return f.tool(id(name),'coding/package',{revision:state.choreography.revision,packageId:packageIds[0],kind,summary:kind+' evidence',artifacts:[],...extra});};
 let file:any;
 if(phase==='admitted'){
 await action('Coordinator','start');
 await assert.rejects(f.tool(id('Coordinator'),'coding/claim',{assignee:'Navigator',instruction:'Write'}),/writing responsibility/);
 await f.tool(id('Coordinator'),'coding/claim',{assignee:'Writer',instruction:'Apply accepted behavior'},'claim-writer');
 file=await f.tool(id('Writer'),'coding/read',{path:'source.txt'});
 await f.tool(id('Writer'),'coding/edit',{path:'source.txt',expectedHash:file.sha256,content:'after\n'},'edit');
 await assert.rejects(f.tool(id('Writer'),'coding/complete',{summary:'Premature freeze'}),/not complete|report|navigator/);

 }
 await f.pause();
 const task=await f.c.command('task',{target:f.task.id});
 const changed=await (f.c as any).request(`/api/tasks/${task.id}/refinement`,{revision:task.revision,sourceRevision:task.sourceRevision,brief:task.brief+' Clarify supported behavior.',criteria:[...task.criteria,'New independently testable requirement']});
 const adopted=await f.c.command('adopt',{target:f.run.id,input:{requestId:crypto.randomUUID(),revision:changed.revision,reason:'Operator approved scope clarification'}});
 if(phase!=='unproposed') assert.notEqual(adopted.taskRevision,(await f.snapshot()).choreography.binding.requirementRevision);
 await f.resume();runtime=await idle();
 assert.equal((await f.snapshot()).packageWriter,null);
 if(phase==='admitted') await assert.rejects(f.tool(id('Writer'),'coding/edit',{path:'source.txt',expectedHash:file.sha256,content:'unauthorized'}),/writer|responsibility|claim/i);
 if(phase==='admitted') await team([],['Writer','Navigator']);
 if(phase==='proposed') packageIds=Array.from({length:50},(_,i)=>'replacement-'+i);
 for(const m of schedule.assignments){m.name+='Rebriefed';m.packages=packageIds;}
 const proposal={revision:(await status()).choreography?.revision??0,schedule,...(phase==='unproposed'?{}:{reopenPackages:[...new Set(['fixture',...packageIds])]})};
 await assert.rejects(f.tool(id('Coordinator'),'coding/choreography-propose',proposal,'stale-plan'),/Accept a current implementation plan/);
 const source=await f.c.command('plan_source',{target:task.id,input:{revision:changed.revision}});
 const prior=(await (f.c as any).request('/api/workspace')).intake[task.id].plans.at(-1);
 prior.content.workPackages=packageIds.map(id=>({...prior.content.workPackages[0],id,criteria:[0,1]}));
 const plan=await f.c.command('plan_save',{target:task.id,input:{requestId:crypto.randomUUID(),revision:changed.revision,sourceId:source.id,predecessorId:prior.id,content:prior.content}});
 await f.c.command('plan_decide',{target:plan.id,input:{requestId:crypto.randomUUID(),revision:changed.revision,sourceId:source.id,decision:'accept'}});
 const reconciled=await f.tool(id('Coordinator'),'coding/choreography-propose',proposal,'fresh-plan');
 assert.equal(reconciled.binding.requirementRevision,changed.revision);assert.equal(reconciled.binding.planId,plan.id);
 assert.equal((await f.snapshot()).run.implementationPlan.id,plan.id);
 assert.equal(reconciled.packages[0].generation,phase==='admitted'?2:1);assert.equal(reconciled.packages[0].state,'proposed');
 await team(['WriterRebriefed','NavigatorRebriefed']);
 assert.deepEqual((await f.snapshot()).assignments.find((a:any)=>a.name==='WriterRebriefed').choreography.packages[0].criteria,[0,1]);
 await action('Coordinator','start');
 await f.tool(id('Coordinator'),'coding/claim',{assignee:'WriterRebriefed',instruction:'Implement the explicitly adopted requirements'},'fresh-writer');
 const current=await f.tool(id('WriterRebriefed'),'coding/read',{path:'source.txt'});
 await f.tool(id('WriterRebriefed'),'coding/edit',{path:'source.txt',expectedHash:current.sha256,content:'adopted behavior\n'},'fresh-edit');
 assert.equal((await f.snapshot()).packageWriter.attemptId,id('WriterRebriefed'));
 t.diagnostic('Explicit adoption preserved historical briefs, blocked stale writer restoration, required a newly accepted plan, and admitted one fresh writer.');
});
