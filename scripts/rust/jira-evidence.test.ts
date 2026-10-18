import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {body,issue,jiraFixture} from './jira-workflow-fixture';
import {checkImage} from './docker-engine';
const image=checkImage();
async function candidate(f:Awaited<ReturnType<typeof jiraFixture>>){
 const run=f.seed.run.id;await f.c.command('prepare',{target:run,input:{mode:'new'}});
 const definition={id:'required',image,command:'/bin/sh',args:['-c','cat source.txt; test "$(cat source.txt)" = after'],timeoutMs:10000,memoryMiB:256,tmpExecutable:false,required:true};await f.c.command('configure',{target:run,input:[definition]});await f.c.command('activate',{target:run});
 await f.c.command('claim',{target:f.seed.writer.attempt.id});const read=await f.c.command('read',{target:run,path:'source.txt'});await f.c.command('edit',{target:f.seed.writer.attempt.id,key:'change',input:{path:'source.txt',expectedHash:read.sha256,content:'after\n'}});
 const completion=await f.c.command('request_completion',{target:f.seed.writer.attempt.id,key:'done',input:{summary:'Change fixture source'}});await f.c.command('pause',{target:run});for(const key of ['coordinator','writer','reviewer','reporter'])await f.c.command('ownership',{target:f.seed[key].attempt.id});await f.c.command('finish_pause',{target:run});await f.c.command('release',{target:f.seed.writer.attempt.id});const candidate=await f.c.command('complete',{target:run,key:completion.id});
 const agents:any={};for(const key of ['coordinator','writer','reviewer','reporter'])agents[key]=await f.c.command('replace',{target:f.seed[key].assignment.id,input:{automatic:false}});await f.c.command('activate',{target:run});
 const receipt=await f.c.command('execute_check',{target:agents.reviewer.id,key:'test',input:{checkId:'required'}});assert.equal(receipt.completed,true);assert.equal(receipt.exitCode,0);assert.equal(receipt.sourceIntact,true);
 await f.c.command('assign_review',{target:agents.coordinator.id,key:'review',input:{assignee:'Reviewer',candidateId:candidate.id,instruction:'Review exact source.'}});
 const review=await f.c.command('review',{target:agents.reviewer.id,input:{candidateId:candidate.id,verdict:'pass',summary:'The criterion is covered.',findings:[],coverage:[{criterion:0,assessment:'Checked exact source.',checkIds:['required']}]}});assert.equal((await f.c.command('assess',{target:run})).ready,true);
 return {candidate,agents,review,definition};
}

test('native Jira evidence uses actual checked revision and review, and refuses changed proof after credentials',{timeout:120000},async t=>{
 const comments:any[]=[];let posts=0;const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){comments.push({id:String(++posts),body:(await body(req)).body});res.end(JSON.stringify({id:String(posts)}));}else res.end(req.url!.includes('/comment')?JSON.stringify({comments,startAt:0,total:comments.length}):JSON.stringify(issue('100','Fix scrolling','10004')));},{activate:false});
 const {candidate:proof,agents,review,definition}=await candidate(f),run=f.seed.run.id;
 const report=(kind:string,key:string)=>f.report({kind,eventKey:key,candidateId:proof.id},agents.reporter.id);
 const evidence=await report('evidence','actual-proof');assert.match(evidence.body,new RegExp(proof.revision.head));assert.match(evidence.body,/required/);await assert.rejects(report('pr_link','link'),/confirmed PR receipt/);
 // This confirms policy consumption of a seeded provider receipt; acquisition is N5.3b.
 const p={id:randomUUID(),runId:run,attemptId:agents.coordinator.id,candidateId:proof.id,repository:{workspace:'fixture',slug:'repo',uuid:randomUUID()},revision:proof.revision,pr:{state:'confirmed',id:9,url:'https://bitbucket.org/fixture/repo/pull-requests/9',providerState:'OPEN'},observation:{pr:{state:'OPEN',source:{commit:{hash:proof.revision.head}}}},remoteSourceHead:proof.revision.head};await f.c.command('save_publication',{input:p});
 const linked=await report('pr_link','link');assert.match(linked.body,/pull-requests\/9/);await f.flush();assert.equal(posts,2);assert.equal((await f.updates()).every((u:any)=>u.state==='confirmed'),true);
 const artifact=join(f.config.authority,'artifacts',review.artifact.id),bytes=readFileSync(artifact);writeFileSync(artifact,'tampered');await assert.rejects(report('evidence','tampered'),/current verified candidate/);writeFileSync(artifact,bytes);
 await report('evidence','late-proof');const call=(await f.c.command('jira_credentials'))+2;f.rules({holdAt:call});const pending=f.flush();await f.entered(call);
 await f.c.command('configure_checks',{target:run,input:[{...definition,memoryMiB:512}]});f.release(call);await pending;assert.equal(posts,2);assert.equal((await f.updates()).find((u:any)=>u.eventKey==='late-proof').state,'blocked');
});

test('native evidence preparation rechecks reporter/run authorization after asynchronous Git',{timeout:120000},async t=>{
 const f=await jiraFixture(t,(_req,res)=>{res.end(JSON.stringify({comments:[],startAt:0,total:0}));},{activate:false});const {candidate:proof,agents}=await candidate(f);const latch=gitLatch(f.root,'merge-base');
 try{await f.reopen();latch.arm();const pending=f.report({kind:'evidence',eventKey:'paused-proof',candidateId:proof.id},agents.reporter.id).then(v=>({value:v}),e=>({error:String(e)}));await latch.blocked();await f.c.command('desired',{target:f.seed.run.id,input:'paused'});latch.release();await latch.finished();const result=await pending;assert.ok('error'in result);assert.match(result.error!,/reporter/);assert.deepEqual(await f.updates(),[]);}finally{latch.release();latch.restore();}
});

async function researchRun(f:Awaited<ReturnType<typeof jiraFixture>>,withReporter:boolean){
 const task=await f.c.command('task',{target:f.seed.task.id});const run=await f.c.command('queue_research',{target:task.id,input:{revision:task.revision,requestId:randomUUID()}});
 const coordinator=await f.c.command('admit',{target:run.id,input:{role:f.seed.coordinator.assignment.role,model:f.seed.coordinator.assignment.model,name:'Research coordinator',coordinator:true,input:'Research pinned source.'}});
 const reporter=withReporter?await f.c.command('admit',{target:run.id,input:{role:f.seed.reporter.assignment.role,model:f.seed.reporter.assignment.model,name:'Research reporter',coordinator:false,input:'Report verified outcome.'}}):undefined;
 await f.c.command('activate',{target:run.id});
 await f.c.command('research_submit',{target:coordinator.attempt.id,key:'research',input:{title:'Source assessment',kind:'research',body:'# Findings\nThe pinned source says before.',citations:[{path:'source.txt',commit:run.researchSource.commit,startLine:1,endLine:1,excerpt:'before'}],limitations:'Only disposable fixture source.',coverage:[{criterionId:0,section:'Findings'}]}});
 await f.c.command('research_request',{target:coordinator.attempt.id,key:'finish',input:{}});
 return {run,coordinator,reporter};
}
test('native research reporting waits for verified completion and uses coordinator fallback only without a reporter',{timeout:60000},async t=>{
 const comments:any[]=[];const paths:string[]=[];
 const f=await jiraFixture(t,async(req,res)=>{paths.push(req.url!);if(req.method==='POST'){comments.push({id:String(comments.length+1),body:(await body(req)).body});res.end(JSON.stringify({id:String(comments.length)}));}else res.end(JSON.stringify({comments,startAt:0,total:comments.length}));});
 await f.c.command('cancel',{target:f.seed.run.id});for(const key of ['coordinator','writer','reviewer','reporter'])await f.c.command('ownership',{target:f.seed[key].attempt.id});await f.c.command('finish_pause',{target:f.seed.run.id});
 for(const withReporter of [true,false]){
  const {run,coordinator,reporter}=await researchRun(f,withReporter);const input={kind:'research_outcome',eventKey:`research-outcome-${run.taskRevision}`};
  if(reporter)await assert.rejects(f.report(input,coordinator.attempt.id),/delivery reporter/);
  const active=reporter??coordinator;await assert.rejects(f.report({kind:'in_progress',eventKey:'execution-started'},active.attempt.id),withReporter?/Research does not/:/reporter/);
  assert.equal(await f.c.command('jira_needs_summary',{target:run.id,revision:run.taskRevision}),true);
  const saved=await f.report(input,active.attempt.id);assert.match(saved.body,/SHA-256/);assert.match(saved.body,/Only disposable fixture source/);assert.equal(await f.c.command('jira_needs_summary',{target:run.id,revision:run.taskRevision}),false);
  const before=comments.length;await f.flush();assert.equal(comments.length,before);
  await f.c.command('pause',{target:run.id});for(const a of [coordinator,reporter].filter(Boolean))await f.c.command('ownership',{target:a.attempt.id});await f.c.command('finish_pause',{target:run.id});await f.c.command('research_complete',{target:run.id});await f.flush();await f.flush();assert.equal(comments.length,before+1);assert.equal((await f.c.command('jira_updates',{target:run.id}))[0].state,'confirmed');
 }
 assert.equal(paths.some(p=>p.includes('transitions')),false);
});

test('native stale research revision cannot hold the verified current outcome lane',{timeout:60000},async t=>{
 const comments:any[]=[];const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){comments.push({id:String(comments.length+1),body:(await body(req)).body});res.end(JSON.stringify({id:String(comments.length)}));}else res.end(JSON.stringify({comments,startAt:0,total:comments.length}));});
 await f.c.command('cancel',{target:f.seed.run.id});for(const key of ['coordinator','writer','reviewer','reporter'])await f.c.command('ownership',{target:f.seed[key].attempt.id});await f.c.command('finish_pause',{target:f.seed.run.id});
 const {run,coordinator,reporter}=await researchRun(f,true);await f.report({kind:'research_outcome',eventKey:`research-outcome-${run.taskRevision}`},reporter.attempt.id);
 const task=await f.c.command('task',{target:f.seed.task.id});const revised=await f.c.command('refine',{target:task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Compare the source and its limitations.',criteria:['Explain the source','Identify risks']}});
 await f.c.command('adopt',{target:run.id,input:{revision:revised.revision,requestId:randomUUID(),reason:'Include the risks.'}});for(const a of [coordinator,reporter])await f.c.command('ownership',{target:a.attempt.id});await f.c.command('finish_pause',{target:run.id});await f.c.command('desired',{target:run.id,input:'running'});
 const next=[];for(const a of [coordinator,reporter])next.push(await f.c.command('replace',{target:a.assignment.id,input:{automatic:false}}));await f.c.command('continue',{target:run.id});await f.c.command('activate',{target:run.id});
 await f.c.command('research_submit',{target:next[0].id,key:'revised',input:{title:'Revised source assessment',kind:'research',body:'# Findings\nThe source says before.\n# Risk\nOnly a fixture was observed.',citations:[{path:'source.txt',commit:run.researchSource.commit,startLine:1,endLine:1,excerpt:'before'}],limitations:'Synthetic fixture source only.',coverage:[{criterionId:0,section:'Findings'},{criterionId:1,section:'Risk'}]}});
 await f.c.command('research_request',{target:next[0].id,key:'finish-current',input:{}});await f.report({kind:'research_outcome',eventKey:`research-outcome-${revised.revision}`},next[1].id);await f.c.command('pause',{target:run.id});for(const a of next)await f.c.command('ownership',{target:a.id});await f.c.command('finish_pause',{target:run.id});await f.c.command('research_complete',{target:run.id});await f.flush();await f.flush();
 assert.deepEqual((await f.c.command('jira_updates',{target:run.id})).map((u:any)=>[u.eventKey,u.state]),[[`research-outcome-${run.taskRevision}`,'blocked'],[`research-outcome-${revised.revision}`,'confirmed']]);assert.equal(comments.length,1);assert.match(JSON.stringify(comments),/Revised source assessment/);
});
