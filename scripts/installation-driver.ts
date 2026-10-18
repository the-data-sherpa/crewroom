import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { backendFixture } from './backend-fixture';
import { createRuntimeObserver } from './runtime-observer';
import { pythonCheckImage } from './rust/docker-engine';
const config=JSON.parse(readFileSync(process.argv[2],'utf8'));
let app=await backendFixture(config);
const backendPids=[app.pid];
writeFileSync(join(config.control,'backend-pids.json'),JSON.stringify(backendPids));
const request=async(path:string,body?:unknown):Promise<any>=>{
 const response=await fetch(app.origin+'/api'+path,body===undefined?{}:{method:'POST',headers:{Origin:app.origin,'Content-Type':'application/json'},body:JSON.stringify(body)});
 const value=await response.json();assert.ok(response.ok,JSON.stringify(value));return value;
};
const observe=createRuntimeObserver(request);
const wait=async(check:()=>Promise<boolean>,message:string)=>{const by=Date.now()+60000;do{if(await check())return;await new Promise(resolve=>setTimeout(resolve,50));}while(Date.now()<by);assert.fail(message);};
const hash=(path:string)=>createHash('sha256').update(readFileSync(path)).digest('hex');
const git=(...args:string[])=>execFileSync('git',['-C',config.repository,...args],{encoding:'utf8',stdio:'pipe'}).trim();
const initialHead=git('rev-parse','HEAD');
try {
 const empty=await request('/workspace');assert.equal(empty.tasks.length,0);assert.equal(empty.runs.length,0);assert.equal(empty.repositories.length,0);
 for(const [id,selector,effort] of config.roles){const role=empty.roles.find((r:any)=>r.id===id);assert.ok(role);await request(`/roles/${id}`,{...role,revision:role.revision??1,model:selector,effort});}
 const repo=await request('/repositories',{name:'Fresh installation fixture',path:config.repository,targetBranch:'main',checks:[{id:'unit',image:pythonCheckImage(),command:'/usr/local/bin/python3',args:['-B','-m','unittest','-v'],timeoutMs:5000,required:true}]});
 const task=await request('/tasks',{title:'Fresh local coding run',description:'No live Jira connection is configured.',repositoryId:repo.id});
 await request(`/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:'Return the sum including empty lists and document the contract.',criteria:['Empty input returns zero','Nonempty input returns its sum','Document the empty-list contract']});
 const run=await request(`/tasks/${task.id}/runs`,{revision:2});const owned=await request(`/runs/${run.id}/repository`,{mode:'new'});
 await request(`/runs/${run.id}/start`,{team:[
  {name:'Coordinator',roleId:'triage',coordinator:true,input:'Delegate implementation, obtain actual check and independent review.'},
  {name:'SpecialistOne',roleId:'implementer',coordinator:false,input:'Implement only with writer ownership.',criteria:[0,1,2].map(index=>({index,obligation:'Implement accepted criterion.',checkIds:index<2?['unit']:[]}))},
  {name:'SpecialistTwo',roleId:'reviewer',coordinator:false,input:'Check and independently review the current candidate.',criteria:[0,1,2].map(index=>({index,obligation:'Independently verify accepted criterion.',checkIds:index<2?['unit']:[]})),dependsOn:['SpecialistOne']}]});
 await wait(async()=>{const state=await observe(run.id);const current=state.assignments.find((a:any)=>a.coordinator)?.currentAttemptId;return state.attempts.find((a:any)=>a.id===current)?.state==='idle' && state.events.some((e:any)=>e.attemptId===current && e.event.type==='message_end' && e.event.message?.role==='assistant' && e.event.message.content?.some((part:any)=>part.type==='text' && part.text==='CODING_READY'));},'First coding coordinator did not complete.');
 const result=await request(`/runs/${run.id}/result`);assert.equal(result.assessment.ready,true);assert.equal(result.checks.length,1);assert.equal(result.checks[0].completed,true);assert.equal(result.checks[0].exitCode,0);assert.equal(result.reviews.length,1);assert.equal(result.publication.record,undefined);
 await request(`/runs/${run.id}/pause`,{});const steering=await request(`/runs/${run.id}/steering`,{requestId:randomUUID(),body:'Retain this operator note across complete backup and restore.'});
 const state=await observe(run.id);assert.ok(state.attempts.every((a:any)=>a.ownership==='stopped'));assert.equal(state.messages.length,3);assert.ok(state.messages.every((m:any)=>m.disposition==='received'));
 const savedTask=(await request('/workspace')).tasks.find((t:any)=>t.id===task.id);const roleHash=hash(config.rolePath);const runtimeHash=hash(join(config.root,'runtime-config.json'));
 const artifactIds=[result.checks[0].output.id,result.reviews[0].artifact.id];const artifacts=await Promise.all(artifactIds.map(id=>request(`/runs/${run.id}/result/artifacts/${id}`)));
 const files=new Map<string,string>();for(const attempt of readdirSync(config.runtime.root)){const session=join(config.runtime.root,attempt,'sessions');if(existsSync(session))for(const path of readdirSync(session,{recursive:true}))if(String(path).endsWith('.jsonl'))files.set(join(session,String(path)),hash(join(session,String(path))));}
 assert.ok(files.size>=3);const beforeWork=await request(`/runs/${run.id}/repository`);
 await app.close();writeFileSync(join(config.control,'backup-ready'),'stopped');
 await wait(async()=>existsSync(join(config.control,'restored')),'Whole-state restore did not finish.');
 assert.equal(hash(join(config.root,'runtime-config.json')),runtimeHash);
 app=await backendFixture(config,{existingRuntime:true});
 backendPids.push(app.pid);writeFileSync(join(config.control,'backend-pids.json'),JSON.stringify(backendPids));
 assert.equal(hash(join(config.root,'runtime-config.json')),runtimeHash);
 const reopened=await request('/workspace');assert.deepEqual(reopened.tasks.find((t:any)=>t.id===task.id),savedTask);assert.equal(reopened.runs.find((r:any)=>r.id===run.id).status,'paused');
 assert.equal(hash(config.rolePath),roleHash);for(const [path,digest] of files)assert.equal(hash(path),digest);
 const after=await request(`/runs/${run.id}/result`);assert.equal(after.assessment.ready,true);assert.deepEqual(after.candidates,result.candidates);assert.deepEqual(after.checks,result.checks);assert.deepEqual(after.reviews,result.reviews);
 assert.deepEqual(await Promise.all(artifactIds.map(id=>request(`/runs/${run.id}/result/artifacts/${id}`))),artifacts);
 const restored=await observe(run.id);assert.deepEqual(restored.messages,state.messages);assert.equal(restored.attempts.length,state.attempts.length);assert.ok(restored.attempts.every((a:any)=>a.ownership==='stopped'));
 assert.equal(reopened.messages.find((m:any)=>m.id===steering.id).body,steering.body);assert.equal(reopened.messages.find((m:any)=>m.id===steering.id).delivery,'pending');
 const restoredWork=await request(`/runs/${run.id}/repository`);assert.equal(restoredWork.path,beforeWork.path);assert.equal(restoredWork.branch,beforeWork.branch);assert.equal(git('status','--porcelain'),'');assert.equal(git('rev-parse','HEAD'),initialHead);
 const nextTask=await request('/tasks',{title:'Subsequent research run',description:'After same-path whole-state restore.',repositoryId:repo.id});
 await request(`/tasks/${nextTask.id}/refinement`,{revision:1,sourceRevision:1,brief:'Describe the pinned fixture contract with a source citation.',criteria:['Explain the documented sum contract']});
 const next=await request(`/tasks/${nextTask.id}/research-runs`,{revision:2,requestId:randomUUID()});
 await request(`/runs/${next.id}/start`,{team:[{name:'ResearchCoordinator',roleId:'triage',coordinator:true,input:'Ask Researcher to submit a verified artifact, then request completion.'},{name:'Researcher',roleId:'tech-writer',coordinator:false,input:'Read README.md and document its contract with a verified source citation.',criteria:[{index:0,obligation:'Explain pinned contract with limitations.'}]}]});
 await wait(async()=>{const value=await request(`/runs/${next.id}/research`);return value.assessment.completed;},'Subsequent research run did not complete.');
 const research=await request(`/runs/${next.id}/research`);assert.equal(research.artifacts.length,1);assert.equal(research.artifacts[0].state,'verified');assert.equal(research.artifacts[0].citations[0].verified,true);
 const completed=await observe(next.id);assert.ok(completed.attempts.every((a:any)=>a.ownership==='stopped'));assert.equal(completed.messages.length,2);assert.ok(completed.messages.every((m:any)=>m.disposition==='received'));
 assert.equal((await request(`/runs/${run.id}/result`)).assessment.ready,true);assert.equal(git('rev-parse','HEAD'),initialHead);assert.equal(git('status','--porcelain'),'');
 console.log(JSON.stringify({passed:true,freshConfiguration:true,cliLaunches:2,runId:run.id,subsequentRun:next.id,firstLocalEvidenceReady:true,subsequentResearchCompleted:true,restored:{requirements:true,steering:true,messages:state.messages.length,sessions:files.size,artifacts:artifactIds,candidate:result.assessment.candidate.revision,ownedWorktree:owned.path,roleHash,runtimeHash},providerBoundary:'Scripted model responses; no Jira or Bitbucket delivery configured.'}));
} finally {await app.close();}
