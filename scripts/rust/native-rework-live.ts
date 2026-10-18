/** Original E2E-03 sequence through the native daemon and actual OMP processes. */
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdirSync,readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {nativeScenarioBackend,nativeIdentities} from './native-scenario-backend';
import {createRuntimeObserver} from '../runtime-observer';
import {pythonCheckImage} from './docker-engine';
const config=JSON.parse(readFileSync(process.argv[2],'utf8'));
const checkout=join(config.root,'operator');mkdirSync(checkout);
const git=(...args:string[])=>execFileSync('git',['-C',checkout,...args],{encoding:'utf8',stdio:'pipe'}).trim();
git('init','-b','main');git('config','user.name','Fixture');git('config','user.email','fixture@example.invalid');
writeFileSync(join(checkout,'sum.py'),'def sum_values(xs): return 9\n');
writeFileSync(join(checkout,'test_sum.py'),'import unittest\nfrom sum import sum_values\nclass SumTest(unittest.TestCase):\n def test_empty(self): self.assertEqual(sum_values([]), 0)\n def test_nonempty(self): self.assertEqual(sum_values([2, 3]), 5)\n');
writeFileSync(join(checkout,'AGENTS.md'),'Change only the accepted sum contract and documentation. Run the configured unit check.\n');git('add','.');git('commit','-m','Native rework fixture');const base=git('rev-parse','HEAD');
const app=await nativeScenarioBackend(config);
async function request(path:string,body?:unknown):Promise<any>{const response=await fetch(app.origin+'/api'+path,{method:body===undefined?'GET':'POST',headers:{Authorization:`Bearer ${app.token}`,Origin:app.origin,'Content-Type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)})});const value=await response.json();assert.ok(response.ok,JSON.stringify(value));return value;}
const observe=createRuntimeObserver(request);
try{
 const criteria=['Empty input returns zero','Nonempty input returns its sum','Document the empty-list contract'];
 const repository=await request('/repositories',{name:'Native rework',path:checkout,targetBranch:'main',checks:[{id:'unit',image:pythonCheckImage(),command:'/usr/local/bin/python3',args:['-B','-m','unittest','-v'],timeoutMs:5000,required:true}]});
 const task=await request('/tasks',{title:'Repair real check and review failures',description:'Scripted model replies drive real native tools.',repositoryId:repository.id});
 await request(`/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:'Return sums including empty input and document the contract.',criteria});
 const run=await request(`/tasks/${task.id}/runs`,{revision:2});const owned=await request(`/runs/${run.id}/repository`,{mode:'new'});
 await request(`/runs/${run.id}/start`,{team:[
  {name:'Coordinator',roleId:'triage',coordinator:true,input:'Delegate implementation and independent validation, then repair recorded failures within the bound.'},
  {name:'SpecialistOne',roleId:'implementer',coordinator:false,input:'Implement only with writer ownership.',criteria:criteria.map((_,index)=>({index,obligation:'Implement accepted criterion.',checkIds:index<2?['unit']:[]}))},
  {name:'SpecialistTwo',roleId:'reviewer',coordinator:false,input:'Run actual checks and independently review every candidate.',criteria:criteria.map((_,index)=>({index,obligation:'Verify the exact candidate.',checkIds:index<2?['unit']:[]})),dependsOn:['SpecialistOne']}]});
 let state:any,coding:any;const deadline=Date.now()+150000;
 do{state=await observe(run.id);coding=await request(`/runs/${run.id}/coding`);assert.notEqual(state.run.status,'attention_required',state.run.attentionReason);const current=state.assignments.find((a:any)=>a.coordinator)?.currentAttemptId;
  if(state.attempts.find((a:any)=>a.id===current)?.state==='idle'&&state.events.some((e:any)=>e.attemptId===current&&e.event.type==='message_end'&&e.event.message?.role==='assistant'&&e.event.message.content?.some((p:any)=>p.type==='text'&&p.text==='CODING_READY')))break;
  await new Promise(resolve=>setTimeout(resolve,100));
 }while(Date.now()<deadline);
 assert.equal(coding.assessment.ready,true,JSON.stringify({coding,attempts:state.attempts,error:state.error}));
 assert.equal(coding.record.candidates.length,3);assert.equal(new Set(coding.record.candidates.map((c:any)=>c.revision.head)).size,3);assert.equal(coding.record.checks.length,3);assert.equal(coding.record.reviews.length,2);
 for(const [i,check] of coding.record.checks.entries()){assert.equal(check.candidateId,coding.record.candidates[i].id);assert.ok(check.started&&check.completed&&check.sourceIntact&&check.environment.platform.startsWith('linux/'));}
 assert.notEqual(coding.record.checks[0].exitCode,0);assert.equal(coding.record.checks[1].exitCode,0);assert.equal(coding.record.checks[2].exitCode,0);
 assert.equal(coding.record.reviews[0].verdict,'changes_requested');assert.equal(coding.record.reviews[0].candidateId,coding.record.candidates[1].id);assert.ok(coding.record.reviews[0].findings.some((f:any)=>f.severity==='major'));
 assert.equal(coding.record.reviews[1].verdict,'pass');assert.equal(coding.record.reviews[1].candidateId,coding.assessment.candidate.id);assert.equal(coding.record.reviews[1].coverage.length,3);
 for(const review of coding.record.reviews){const candidate=coding.record.candidates.find((c:any)=>c.id===review.candidateId);assert.ok(!candidate.implementationAttempts.includes(review.attemptId));}
 assert.equal(state.assignments.find((a:any)=>a.name==='SpecialistOne').automaticCycles,2);assert.ok(state.messages.some((m:any)=>m.body==='The actual unit check failed with 0 != 5. Repair the nonempty case.'&&m.disposition==='received'));
 assert.ok(state.events.some((e:any)=>e.event.type==='tool_execution_end'&&e.event.toolName==='check_output'&&e.event.isError===false&&JSON.stringify(e.event).includes('0 != 5')));
 const first=coding.record.checks[0];const output=await request(`/runs/${run.id}/result/artifacts/${first.output.id}`);assert.ok(JSON.stringify(output).includes('0 != 5'));
 assert.equal(git('rev-parse','HEAD'),base);assert.equal(git('status','--porcelain'),'');assert.equal(Number(git('rev-list','--count',owned.branch)),4);
 const identities=await nativeIdentities(config,state);await request(`/runs/${run.id}/pause`,{});const stopped=await observe(run.id);assert.ok(stopped.attempts.every((a:any)=>a.ownership==='stopped'));
 const report={passed:true,messages:state.messages,scenario:'E2E-03',criteria,runId:run.id,identities,candidates:coding.record.candidates,checks:coding.record.checks,reviews:coding.record.reviews,failedCheckOutputInspected:true,blockingReviewRepaired:true,automaticRepairCycles:2,operatorUnchanged:true,pausedAllStopped:true,attempts:stopped.attempts.map((a:any)=>({id:a.id,ownership:a.ownership,hostPid:a.hostPid,ompPid:a.ompPid})),boundary:'Actual native daemon, OMP, Rust Mail, Git and Docker; scripted model boundary. Third repair refusal is composed with existing native recovery-budget gate.'};
 writeFileSync(join(config.root,'scenario-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}finally{await app.close();}
