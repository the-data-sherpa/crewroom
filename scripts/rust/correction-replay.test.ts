import assert from 'node:assert/strict';
import test from 'node:test';
import {writeFileSync,readdirSync} from 'node:fs';
import {join} from 'node:path';
import {workflowFixture,until} from './workflow-support';

// FOR-13: derived from the retained MH6 review diagnostic, SHA-256
// 9b9cd49105ebbaf0c52732eb3b9394c7bedf799f323912cf17553fff67630cbd.
// The diagnostic passed when the defect occurred; this fixture requires correct replay.
// Production daemon, real supervised OS processes/Git and synthetic inference/Mail.
// Live provider acceptance is retained separately; this fixture tests the protocol.
test('FOR-13 committed package replay precedes mutable artifact validation', {timeout:180000}, async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true,configure({root,roles}){
  roles.push({...roles.find(r=>r.id==='tech-writer')!,id:'navigator',name:'Navigator'});
  writeFileSync(join(root,'roles.json'),JSON.stringify(roles));
 }});
 t.diagnostic(JSON.stringify({phase:'fixture',root:f.root,daemonPid:f.c.proc.pid}));
 await f.c.command('prepare',{target:f.run.id,input:{mode:'new'}});
 await f.start();
 const idle=()=>until(async()=>{const r=await f.snapshot();return !(await f.c.command('busy',{target:f.run.id}))&&r.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;},45000);
 let runtime=await idle();
 const id=(name:string)=>runtime.assignments.find((a:any)=>a.name===name).currentAttemptId;
 const status=()=>f.tool(id('Coordinator'),'coding/choreography',{});
 const member=(name:string,roleId:string,responsibility:string,stage:number,writes=false)=>({name,roleId,responsibility,stage,writes,packages:['fixture'],rationale:'Exercise '+responsibility,input:'Fulfil '+responsibility+' responsibility'});
 const schedule={uiRequired:false,diagnosisReason:null,assignments:[member('Writer','implementer','writer',0,true),member('Navigator','navigator','navigator',0),member('Docs','tech-writer','documentation',1,true),member('Review','reviewer','reviewer',2)]};
 await f.tool(id('Coordinator'),'coding/choreography-propose',{revision:0,schedule},'schedule');
 const team=async(names:string[],retire:string[]=[])=>{
  const prior=id('Coordinator');
  await f.tool(prior,'team-proposal',{specialists:names.map(name=>{const m=schedule.assignments.find(m=>m.name===name)!;return {name,roleId:m.roleId,input:m.input,criteria:[{index:0,obligation:'Verify accepted criterion',checkIds:[]}],dependsOn:[]};}),retire:retire.map(name=>({name,reason:'Package contribution reconciled'}))});
  runtime=await until(async()=>{const r=await idle();return r.assignments.find((a:any)=>a.name==='Coordinator').currentAttemptId!==prior&&names.every(name=>r.assignments.some((a:any)=>a.name===name&&!a.retiredAt))?r:false;},45000);
 };
 await team(['Writer','Navigator']);
 const action=async(name:string,kind:string,extra:any={})=>{const state=await status();return f.tool(id(name),'coding/package',{revision:state.choreography.revision,packageId:'fixture',kind,summary:kind+' evidence',artifacts:[],...extra});};
 await action('Coordinator','start');
 await assert.rejects(f.tool(id('Coordinator'),'coding/claim',{assignee:'Navigator',instruction:'Write'}),/writing responsibility/);
 await f.tool(id('Coordinator'),'coding/claim',{assignee:'Writer',instruction:'Apply accepted behavior'},'claim-writer');
 const file=await f.tool(id('Writer'),'coding/read',{path:'source.txt'});
 await f.tool(id('Writer'),'coding/edit',{path:'source.txt',expectedHash:file.sha256,content:'after\n'},'edit');
 await assert.rejects(f.tool(id('Writer'),'coding/complete',{summary:'Premature freeze'}),/not complete|report|navigator/);

 const artifact=await f.tool(id('Writer'),'coding/read',{path:'source.txt'});
 const reportInput={revision:(await status()).choreography.revision,packageId:'fixture',kind:'report',summary:'Retain implementation snapshot',artifacts:[{path:'source.txt',sha256:artifact.sha256}]};
 const first=await f.tool(id('Writer'),'coding/package',reportInput,'review-report');
 await f.tool(id('Writer'),'coding/edit',{path:'source.txt',expectedHash:artifact.sha256,content:'second change\n'},'second-edit');
 const artifactsBeforeReplay=readdirSync(join(f.root,'data','artifacts')).sort();
 t.diagnostic('FOR-13 reached saved replay after successful report and authorized later edit');
 assert.deepEqual(await f.tool(id('Writer'),'coding/package',reportInput,'review-report'),first);
 await assert.rejects(f.tool(id('Writer'),'coding/package',{...reportInput,summary:'Changed request'},'review-report'),/different/);
 assert.equal((await f.tool(id('Writer'),'coding/read',{path:'source.txt'})).content,'second change\n');
 assert.deepEqual(readdirSync(join(f.root,'data','artifacts')).sort(),artifactsBeforeReplay);
 const saved=await status(); assert.equal(saved.choreography.reports.length,1);assert.equal(saved.choreography.reports[0].id,first.reports[0].id);
 t.diagnostic('FOR-13 passed: saved receipt replayed; changed payload refused; one report; later edit preserved');
});
