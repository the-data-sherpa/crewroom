import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createServer} from 'node:http';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until,acceptFixturePlan} from './workflow-support';
import {NativeDaemon} from './daemon-client';
import {reviewedCandidate} from './workflow-coding-fixture';
import {providerRepository} from './publication-workflow-fixture';
import {body} from './jira-workflow-fixture';
import {DatabaseSync} from 'node:sqlite';

test('native scheduler reconciles hosted review, re-hold, queued Resume priority and restart through actual checks and scripted Bitbucket',{timeout:180000},async t=>{
 let remote='',status='SUCCESSFUL';const prs:any[]=[];
 const reference=(branch:string)=>execFileSync('git',['-C',remote,'rev-parse',`refs/heads/${branch}`],{encoding:'utf8'}).trim();
 const provider=createServer(async(req,res)=>{try{
  assert.equal(req.headers.authorization,'Bearer synthetic-bitbucket-token');res.setHeader('Content-Type','application/json');
  const path=new URL(req.url!,'http://fixture').pathname;const send=(v:any)=>res.end(JSON.stringify(v));const current=()=>prs.at(-1);
  if(path.endsWith('/repository'))return void send({uuid:providerRepository.uuid,full_name:'fixture/repository'});
  if(req.method==='POST'){const v=await body(req);const pr={id:prs.length+1,title:v.title,description:v.description,state:'OPEN',updated_on:'2026-10-03T00:00:00Z',source:{branch:v.source.branch,commit:v.source.commit,repository:{uuid:providerRepository.uuid}},destination:{branch:v.destination.branch,commit:{hash:reference(v.destination.branch.name)},repository:{uuid:providerRepository.uuid}},links:{html:{href:`https://bitbucket.org/fixture/repository/pull-requests/${prs.length+1}`}}};prs.push(pr);return void send(pr);}
  if(req.method==='PUT'){const v=await body(req);Object.assign(current(),v);return void send(current());}
  if(path.endsWith('/pullrequests'))return void send({values:prs});if(/\/pullrequests\/\d+$/.test(path))return void send(current());
  if(path.endsWith('/comments'))return void send({values:[]});
  if(path.endsWith('/statuses'))return void send({values:[{key:'build',state:status,name:'Build',url:'https://example.invalid/build',updated_on:'2026-10-03T00:00:00Z'}]});
  if(path.endsWith('/pipelines/')){const pr=current();return void send({values:[{uuid:'pipeline-1',build_number:1,run_number:1,created_on:'2026-10-03T00:00:00Z',state:{name:'COMPLETED',result:{name:'SUCCESSFUL'}},target:{type:'pipeline_pullrequest_target',commit:pr.source.commit,destination_commit:pr.destination.commit,source:pr.source.branch.name,destination:pr.destination.branch.name,pullrequest:{id:pr.id},selector:{type:'pull-requests',pattern:'**'}}}]});}
  if(path.endsWith('/steps/'))return void send({values:[{uuid:'step-1',name:'Tests',state:{name:'COMPLETED',result:{name:'SUCCESSFUL'}}}]});
  res.statusCode=404;send({});
 }catch(error){res.statusCode=500;res.end(JSON.stringify({error:String(error)}));}});
 await new Promise<void>(r=>provider.listen(0,'127.0.0.1',r));t.after(async()=>{provider.closeAllConnections();await new Promise<void>(r=>provider.close(()=>r()));});
 const apiUrl=`http://127.0.0.1:${(provider.address() as any).port}`;
 const f=await workflowFixture(t,'coding',2,false,{service:true,configure({root,source,config}){remote=join(root,'remote.git');execFileSync('git',['clone','--bare',source,remote],{stdio:'ignore'});config.publicationRemote={connections:[{repositoryPath:source,repository:providerRepository,policy:{statusKeys:['build'],pipelineSteps:['Tests'],optionalSteps:[]},fixtureGitPath:remote,apiUrl}]};}});
 const client=()=>{assert.ok(f.c instanceof NativeDaemon);return f.c;};
 const queue=async(title:string)=>{const c=client();const task=await c.request('/api/tasks',{title,description:'',repositoryId:f.repository.id});await c.request(`/api/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:title,criteria:['Checked change']});await acceptFixturePlan(c,task.id);return c.request(`/api/tasks/${task.id}/runs`,{revision:2,launch:{repository:{mode:'new'},team:[f.team[0]]}});};
 const state=(id:string)=>client().request(`/api/runs/${id}/runtime`);
 const running=(id:string)=>until(async()=>{const v=await state(id);return v.run.status==='running'&&!v.busy?v:false;},30000);
 const {candidate,coordinator}=await reviewedCandidate(f);const second=await queue('Second slot');const third=await queue('Third slot');
 await f.tool(coordinator,'publication/prepare',{candidateId:candidate.id,summary:'Checked native daemon publication with actual Docker evidence and independent review.'},'publish');
 await client().request(`/api/runs/${f.run.id}/publication/reconcile`,{});await running(second.id);
 let first=await state(f.run.id);assert.equal(first.run.status,'paused');assert.equal(first.run.pauseReason,'review');assert.ok(first.attempts.every((a:any)=>a.ownership==='stopped'));assert.equal(prs.length,1);
 // Reconciliation can join an observation already in flight. Wait for saved invalidation before releasing the holder.
 status='FAILED';await until(async()=>{const observed=await client().request(`/api/runs/${f.run.id}/publication/reconcile`,{});return observed.record?.reviewInvalidated===true;});assert.equal((await client().request('/api/workspace')).queue.slot.runId,second.id);
 await client().request(`/api/runs/${second.id}/cancel`,{});
 await until(async()=>{const w=await client().request('/api/workspace');return w.queue.slot?.runId===f.run.id;});first=await state(f.run.id);assert.equal(first.run.status,'paused');assert.equal(first.run.pauseReason,'validation');assert.equal((await state(third.id)).attempts.length,0);
 status='SUCCESSFUL';await client().request(`/api/runs/${f.run.id}/publication/reconcile`,{});await running(third.id);assert.equal((await state(f.run.id)).run.pauseReason,'review');
 await client().request(`/api/runs/${f.run.id}/resume`,{});assert.equal((await state(f.run.id)).run.resumeRequest.state,'queued');const fourth=await queue('Fourth slot');
 await f.kill();await f.reopen();await running(third.id);assert.equal((await state(f.run.id)).run.resumeRequest.state,'queued');
 await client().request(`/api/runs/${third.id}/cancel`,{});first=await running(f.run.id);assert.ok(first.attempts.length>4);assert.equal((await state(fourth.id)).attempts.length,0);assert.equal((await client().request('/api/workspace')).queue.slot.runId,f.run.id);
 await client().request(`/api/runs/${f.run.id}/cancel`,{});await running(fourth.id);assert.equal(prs.length,1);
});

test('daemon history and provider usage equal the retained projection after a real restart',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'research',1,false,{service:true});await f.start();await f.idle();await f.pause();const runtime=await f.snapshot();const attempt=runtime.attempts[0];await f.c.close();
 const store=new DatabaseSync(f.database);
 try {
  const events:any[]=[{type:'provider_request'},{type:'provider_usage',state:'actual',usage:{input:10,output:2,cacheRead:3,cacheWrite:4,reasoning:1,total:20}},{type:'provider_request'},{type:'provider_usage',state:'unavailable',reason:'Receipt unavailable'},{type:'provider_request'},{type:'message_end',message:{role:'assistant',provider:'fixture',model:'model',timestamp:123,content:'Recorded answer',usage:{cost:{total:0.2}}}},{type:'message_end',message:{role:'assistant',provider:'fixture',model:'model',timestamp:123,content:'Recorded answer',usage:{cost:{total:0.2}}}},{type:'process_exit',code:17,signal:null},{type:'prompt_result',status:'error',error:{message:'Retained failure'}}];
  let seq=Number(store.prepare('SELECT COALESCE(MAX(sequence),0)+1 AS next FROM agent_events WHERE attempt_id=?').get(attempt.id)!.next);for(const e of events)store.prepare('INSERT INTO agent_events(attempt_id,sequence,data) VALUES(?,?,?)').run(attempt.id,seq++,JSON.stringify(e));
 }finally{store.close();}
 await f.reopen();const got=(await f.snapshot()).history;assert.ok(got.attempts.some((a:any)=>a.causes.some((c:string)=>c.includes('Retained failure'))));assert.ok(JSON.stringify(got.usage).includes('0.2'));await f.kill();await f.reopen();assert.deepEqual((await f.snapshot()).history,got);
});

test('dependency merge records hold queue admission until the selected checkout contains the verified merge',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true});
 const git=(...args:string[])=>execFileSync('git',['-C',f.source,...args],{encoding:'utf8'}).trim();
 const base=git('rev-parse','HEAD');git('checkout','-b','fixture-merged');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--allow-empty','-m','Observed merge');const merged=git('rev-parse','HEAD');git('checkout','main');
 assert.ok(f.c instanceof NativeDaemon);await f.c.request(`/api/queue/${f.run.id}/remove`,{requestId:randomUUID()});
 const task=await f.c.request('/api/tasks',{title:'Dependent',description:'',repositoryId:f.repository.id});await f.c.request(`/api/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:'Wait for verified ancestry',criteria:['Merged base']});await f.c.request(`/api/tasks/${task.id}/dependencies`,{requestId:randomUUID(),dependencyRevision:0,dependsOn:[f.task.id]});await acceptFixturePlan(f.c,task.id);
 let dependent=await f.c.request(`/api/tasks/${task.id}/runs`,{revision:2,launch:{repository:{mode:'new'},team:f.team}});
 await f.c.close();
 const store=new DatabaseSync(f.database);
 try {
  const publication:any={id:randomUUID(),runId:f.run.id,attemptId:randomUUID(),candidateId:randomUUID(),repositoryPath:f.source,repository:providerRepository,branch:'fixture/merged',target:'main',revision:{head:merged,base,tree:git('rev-parse',merged+'^{tree}'),worktreeHash:'a'.repeat(64),requirementsHash:'b'.repeat(64),instructionsHash:'c'.repeat(64),dirty:false},title:'Provider-observed fixture',description:'Seeded provider merge record, not a live delivery',policy:{statusKeys:[],pipelineSteps:[],optionalSteps:[]},createdAt:new Date().toISOString(),push:{state:'confirmed',expectedHead:null},pr:{state:'confirmed',id:1,providerState:'MERGED'},ready:false,blockers:[],merged:{commit:merged,at:new Date().toISOString()}};
  store.prepare('INSERT INTO publications VALUES(?,?)').run(f.run.id,JSON.stringify(publication));
 }finally{store.close();}
 await f.reopen();assert.ok(f.c instanceof NativeDaemon);let c=f.c;await acceptFixturePlan(c,dependent.taskId);
 const held=await until(async()=>{const w=await c.request('/api/workspace');return w.queue.plans.find((p:any)=>p.id===dependent.id)?.dependencyBaseHold;});assert.match(held.reason,/target|merge|branch/i);assert.equal((await c.request(`/api/runs/${dependent.id}/runtime`)).attempts.length,0);
 git('merge','--ff-only',merged);
 // MH5 pins queued plans to their inspected source. Reaccept and requeue after the explicit checkout update.
 await c.request(`/api/queue/${dependent.id}/remove`,{requestId:randomUUID()});
 await acceptFixturePlan(c,dependent.taskId);
 dependent=await c.request(`/api/tasks/${task.id}/runs`,{revision:2,launch:{repository:{mode:'new'},team:f.team}});
 await until(async()=>{const v=await c.request(`/api/runs/${dependent.id}/runtime`);return v.run.status==='running';},30000);assert.equal((await c.request('/api/workspace')).queue.slot.runId,dependent.id);
});
