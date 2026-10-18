import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {join} from 'node:path';
import {type TestContext,test} from 'node:test';
import {workflowFixture,until} from './workflow-support';
import {reviewedCandidate} from './workflow-coding-fixture';
import {source,issue,body} from './jira-workflow-fixture';
import {providerRepository} from './publication-workflow-fixture';

async function provider(t:TestContext,token:string,handler:(req:IncomingMessage,res:ServerResponse)=>Promise<void>){
 const server=createServer(async(req,res)=>{try{assert.equal(req.headers.authorization,'Bearer '+token);res.setHeader('Content-Type','application/json');await handler(req,res);}catch(error){res.statusCode=500;res.end(JSON.stringify({error:String(error)}));}});
 await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));t.after(async()=>{server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));});return `http://127.0.0.1:${(server.address() as any).port}`;
}

test('native research completion requires a saved Jira summary and confirms exactly one scripted provider comment after stop',{timeout:90000},async t=>{
 const comments:any[]=[];const apiUrl=await provider(t,'synthetic-jira-token',async(req,res)=>{assert.ok(req.url?.includes('/comment'));if(req.method==='POST'){comments.push({id:String(comments.length+1),body:(await body(req)).body});res.end(JSON.stringify({id:String(comments.length)}));}else res.end(JSON.stringify({comments,startAt:0,total:comments.length}));});
 const f=await workflowFixture(t,'research',1,true,{service:process.env.CREWROOM_TEST_DAEMON==='1',jira:{source,issue:issue(),apiUrl}});await f.start();const runtime=await f.idle(),coordinator=runtime.assignments[0].currentAttemptId;
 await f.tool(coordinator,'research/submit',{title:'Research result',kind:'research',body:'# Findings\nThe pinned source says before.',citations:[{path:'source.txt',commit:f.run.researchSource.commit,startLine:1,endLine:1,excerpt:'before'}],limitations:'Scripted provider fixture only.',coverage:[{criterionId:0,section:'Findings'}]},'research');
 await f.tool(coordinator,'research/complete',{},'complete');await f.idle();assert.equal((await f.c.command('run',{target:f.run.id})).status,'running');assert.equal(comments.length,0);
 const saved=await f.tool(coordinator,'jira/report',{kind:'research_outcome',eventKey:`research-outcome-${f.run.taskRevision}`},'report');assert.match(saved.body,/SHA-256/);assert.equal(saved.state,'pending');await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='completed');assert.equal(comments.length,0);
 await f.c.command('jira_flush',{target:f.run.id});await f.c.command('jira_flush',{target:f.run.id});assert.equal(comments.length,1);assert.equal((await f.c.command('jira_updates',{target:f.run.id}))[0].state,'confirmed');assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));
});

test('authenticated publication publishes the reviewed revision to disposable Git and a scripted Bitbucket PR exactly once',{timeout:120000},async t=>{
 const prs:any[]=[];let remote='';const reference=(branch:string)=>execFileSync('git',['-C',remote,'rev-parse',`refs/heads/${branch}`],{encoding:'utf8'}).trim();
 const apiUrl=await provider(t,'synthetic-bitbucket-token',async(req,res)=>{
  const path=new URL(req.url!,'http://fixture').pathname;const send=(v:any)=>{res.end(JSON.stringify(v));};
  if(path.endsWith('/repository'))return send({uuid:providerRepository.uuid,full_name:'fixture/repository'});
  if(req.method==='POST'){const input=await body(req);const pr={id:prs.length+1,title:input.title,description:input.description,state:'OPEN',updated_on:'2026-10-03T00:00:00Z',source:{branch:input.source.branch,commit:input.source.commit,repository:{uuid:providerRepository.uuid}},destination:{branch:input.destination.branch,commit:{hash:reference(input.destination.branch.name)},repository:{uuid:providerRepository.uuid}},links:{html:{href:`https://bitbucket.org/fixture/repository/pull-requests/${prs.length+1}`}}};prs.push(pr);return send(pr);}
  if(path.endsWith('/pullrequests'))return send({values:prs});if(/\/pullrequests\/\d+$/.test(path))return send(prs.at(-1));
  if(path.endsWith('/comments')||path.endsWith('/statuses')||path.endsWith('/pipelines/'))return send({values:[]});res.statusCode=404;send({});
 });
 const f=await workflowFixture(t,'coding',2,false,{service:process.env.CREWROOM_TEST_DAEMON==='1',configure({root,source,config}){remote=join(root,'remote.git');execFileSync('git',['clone','--bare',source,remote],{stdio:'ignore'});config.publicationRemote={connections:[{repositoryPath:source,repository:providerRepository,policy:{statusKeys:['build'],pipelineSteps:['Tests'],optionalSteps:[]},fixtureGitPath:remote,apiUrl}]};}});
 const {candidate,coordinator,reviewer}=await reviewedCandidate(f);await assert.rejects(f.tool(reviewer,'publication/prepare',{candidateId:candidate.id,summary:'Unauthorized'}),/not registered/);
 const input={candidateId:candidate.id,summary:'Updated the fixture source with passing checks and independent review.'};await f.tool(coordinator,'publication/prepare',input,'publish');await f.c.command('publication_flush',{target:f.run.id});await f.c.command('publication_flush',{target:f.run.id});
 const publication=await f.c.command('publication',{target:f.run.id});assert.equal(publication.push.state,'confirmed');assert.equal(publication.pr.state,'confirmed');assert.equal(reference(publication.branch),candidate.revision.head);assert.equal(prs.length,1);assert.equal(publication.ready,false,'Scripted provider has no hosted quality evidence');assert.match(prs[0].description,new RegExp(candidate.revision.head));assert.equal((await f.tool(coordinator,'publication/status',{})).record.pr.state,'confirmed');
});
