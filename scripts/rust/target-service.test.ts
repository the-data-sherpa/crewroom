/** Native wrapper proofs: actual checked Git/SQLite; HTTP PRs and attempts are fixtures. */
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {existsSync,readFileSync,readdirSync,renameSync,writeFileSync} from 'node:fs';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {join} from 'node:path';
import {test,type TestContext} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {DatabaseSync} from 'node:sqlite';
import {defaultRoles} from '../../server/roles';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {fixture,alter,waitFor} from './target-fixture';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
const repository={workspace:'fixture',slug:'repository',uuid:'{00000000-0000-4000-8000-000000000011}'};
const policy={statusKeys:['build'],pipelineSteps:['Tests']};
const roles=defaultRoles.map(r=>({...r,model:'fixture/model',effort:'low'}));
async function setup(t:TestContext) {
 const f=fixture(t);const {work,candidate}=await f.candidate();const remote=join(f.root,'remote.git');f.git('clone','--bare',f.seed.root,remote);
 let handler:(req:IncomingMessage,res:ServerResponse)=>void=(_req,res)=>{res.statusCode=503;res.end('{}');};let http=0;
 const server=createServer((req,res)=>{http++;assert.equal(req.headers.authorization,'Bearer synthetic-bitbucket-token');handler(req,res);});await new Promise<void>(resolve=>server.listen(0,'127.0.0.1',resolve));
 t.after(()=>new Promise<void>(resolve=>server.close(()=>resolve())));
 const connection={repositoryPath:f.seed.root,repository,policy,fixtureGitPath:remote,apiUrl:`http://127.0.0.1:${(server.address() as any).port}`};
 const options={publicationRemote:{connections:[connection]}};
 const jobs=join(f.config.authority,'publication');
 const intents=()=>existsSync(jobs)?readdirSync(jobs).flatMap(dir=>readdirSync(join(jobs,dir)).filter(n=>n.endsWith('.json')).map(n=>JSON.parse(readFileSync(join(jobs,dir,n),'utf8')))):[];
 f.beforeCleanup.push(async()=>{for(const item of intents()){const state=await inspectCheckedOwned(item.descriptor);if(state.state!=='stopped')await stopOwned(item.descriptor.root);assert.equal(state.state,'stopped','Remote wrapper left an unverified Git child');}});
 await f.reopen(options);
 return {f,work,candidate,remote,options,connection,intents,get http(){return http;},setHandler(h:typeof handler){handler=h;},
  reopen:(extra={})=>f.reopen({...options,...extra}),
  refresh:(requestId=randomUUID(),operatorDecision?:any)=>f.c.command('target_request',{target:f.seed.run.id,input:{requestId,...(operatorDecision?{operatorDecision}:{})}}),
  move(file='target.txt',content='remote target\n'){const head=f.move(file,content);f.git('push',remote,'main');return head;},
  ref(name:string){return f.gitAt(remote,'rev-parse',name);},
 };
}
function countFetch(f:Awaited<ReturnType<typeof setup>>){return f.intents().filter(i=>i.spec.args.includes('fetch')).length;}

test('native target wrapper fetches once, atomically associates evidence and notice, and replays before missing provider access',{timeout:60000},async t=>{
 const r=await setup(t),key=randomUUID();const target=r.move();const result=await r.refresh(key);assert.equal(result.changed,true);assert.equal(result.refresh.toBase,target);assert.notEqual(result.candidateId,r.candidate.id);assert.equal(result.refresh.targetRef,`refs/crewroom/${r.f.seed.run.id}/refresh/${key}/target`);assert.equal(countFetch(r),1);assert.equal((await r.f.c.command('notices',{target:r.f.seed.run.id})).filter((n:any)=>n.payload.kind==='target_refresh').length,1);
 await r.reopen({publicationRemote:{connections:[]}});assert.deepEqual(await r.refresh(key),result);assert.equal(countFetch(r),1);await assert.rejects(r.refresh(),/remote target is unavailable/);assert.equal(r.http,0);
 await r.reopen();await assert.rejects(r.f.c.command('remote_fetch',{target:r.f.seed.run.id,key}),/recorded target request/);
 const next=r.move('other.txt','later\n');const later=await r.refresh();assert.equal(later.refresh.toBase,next);assert.equal(later.refresh.sourceCandidateId,result.candidateId);assert.deepEqual(await r.refresh(key),result);assert.equal(countFetch(r),2);
});

test('concurrent native target requests share one pinned receipt and no-op keeps the exact current candidate',{timeout:60000},async t=>{
 const r=await setup(t),key=randomUUID();const results=await Promise.all(Array.from({length:8},()=>r.refresh(key)));for(const result of results){assert.deepEqual(result,results[0]);assert.equal(result.changed,false);assert.equal(result.candidateId,r.candidate.id);}assert.equal(countFetch(r),1);assert.equal((await r.f.c.command('record',{target:r.f.seed.run.id})).candidates.length,1);assert.deepEqual(await r.f.c.command('notices',{target:r.f.seed.run.id}),[]);
});

test('failed target-notice persistence rolls back candidate and association; startup finishes it once',{timeout:60000},async t=>{
 const r=await setup(t);r.move();const key=randomUUID();const db=new DatabaseSync(r.f.seed.database);db.exec("CREATE TRIGGER fail_target_notice BEFORE INSERT ON delivery_notices WHEN json_extract(NEW.data,'$.payload.kind')='target_refresh' BEGIN SELECT RAISE(ABORT,'fixture target notice failure'); END;");db.close();
 await assert.rejects(r.refresh(key),/fixture target notice failure/);assert.equal((await r.f.c.command('record',{target:r.f.seed.run.id})).candidates.length,1);assert.equal((await r.f.record()).refreshes.at(-1).candidateId,undefined);
 await r.f.c.command('target_recover');assert.equal((await r.f.c.command('run',{target:r.f.seed.run.id})).status,'attention_required');const repair=new DatabaseSync(r.f.seed.database);repair.exec('DROP TRIGGER fail_target_notice');repair.close();await r.reopen();await r.f.c.command('target_recover');const saved=await r.refresh(key);assert.equal(saved.changed,true);assert.equal(countFetch(r),1);await r.f.c.command('target_recover');assert.equal((await r.f.c.command('record',{target:r.f.seed.run.id})).candidates.length,2);assert.equal((await r.f.c.command('notices',{target:r.f.seed.run.id})).length,1);
});

test('a changed live remote after fetch preserves the source and refuses pinned-snapshot substitution',{timeout:60000},async t=>{
 const r=await setup(t);r.move();const latch=gitLatch(r.f.root,'ls-remote');t.after(()=>latch.restore());await r.reopen();latch.arm();const key=randomUUID();const pending=r.refresh(key);const rejected=assert.rejects(pending,/target changed|live target|original branch|unchanged/i);await latch.blocked();const later=r.move('late.txt','late target\n');latch.release();await rejected;
 assert.equal(r.f.gitAt(r.work.path,'rev-parse','HEAD'),r.candidate.revision.head);assert.equal(r.ref('main'),later);assert.equal((await r.f.record()).refreshes.at(-1).state,'conflicted');assert.equal((await r.refresh(key)).refresh.state,'conflicted');assert.equal(countFetch(r),1);
});

test('stopping a run drains active fetch and fences an already queued fetch; a cancelled waiter cannot orphan it',{timeout:60000},async t=>{
 const r=await setup(t);const latch=gitLatch(r.f.root,'fetch');t.after(()=>latch.restore());await r.reopen();latch.arm();const key=randomUUID(),rpc=randomUUID();const pending=r.f.c.command('target_request',{target:r.f.seed.run.id,input:{requestId:key}},rpc);const rejected=assert.rejects(pending,/cancelled/);await latch.blocked();const queued=r.refresh();const denied=assert.rejects(queued,/stopp/);await r.f.c.command('cancel_waiter',{requestId:rpc});await rejected;
 const live=r.intents().find(i=>i.spec.args.includes('fetch'));assert.equal((await inspectCheckedOwned(live.descriptor)).state,'live');await r.f.c.command('remote_stop',{target:r.f.seed.run.id});await denied;assert.equal((await inspectCheckedOwned(live.descriptor)).state,'stopped');assert.equal(countFetch(r),1);assert.throws(()=>r.f.git('rev-parse',`refs/crewroom/${r.f.seed.run.id}/refresh/${key}/target`));
 latch.release();assert.equal(await r.f.c.command('remote_observe',{target:r.f.seed.run.id}),r.candidate.revision.base);
 await r.f.c.command('remote_close');await r.f.c.command('remote_close');await assert.rejects(r.refresh(),/stopping/);await assert.rejects(r.f.c.command('remote_observe',{target:r.f.seed.run.id}),/stopping/);
});

test('actual controller kill during fetch reconciles the old child before refetch and never admits its abandoned request',{timeout:60000},async t=>{
 const r=await setup(t);const latch=gitLatch(r.f.root,'fetch');t.after(()=>latch.restore());await r.reopen();latch.arm();const abandoned=randomUUID();const pending=r.refresh(abandoned);const rejected=assert.rejects(pending,/exited/);await latch.blocked();const old=r.intents().find(i=>i.spec.args.includes('fetch'));r.f.c.proc.kill('SIGKILL');await r.f.c.exit;await rejected;assert.equal((await inspectCheckedOwned(old.descriptor)).state,'live');
 await r.reopen();const result=await r.refresh();assert.equal(result.changed,false);assert.equal((await inspectCheckedOwned(old.descriptor)).state,'stopped');assert.equal((await r.f.record()).refreshes.some((v:any)=>v.requestId===abandoned),false);assert.equal(countFetch(r),2);
});

function pub(r:Awaited<ReturnType<typeof setup>>){return {id:randomUUID(),runId:r.f.seed.run.id,repositoryPath:r.work.repository.path,repository,branch:r.work.branch,target:'main',candidateId:r.candidate.id,revision:r.candidate.revision,pr:{id:9,state:'confirmed',providerState:'OPEN',url:'https://bitbucket.org/fixture/repository/pull-requests/9'},push:{state:'confirmed'}};}
function pullRequest(p:ReturnType<typeof pub>){return {id:9,title:'Fixture PR',description:`Crewroom publication ${p.id}`,state:'OPEN',updated_on:'2026-10-03T00:00:00Z',source:{branch:{name:p.branch},commit:{hash:p.revision.head},repository:{uuid:repository.uuid}},destination:{branch:{name:'main'},commit:{hash:p.revision.base},repository:{uuid:repository.uuid}},links:{html:{href:p.pr.url}}};}
async function cancel(r:Awaited<ReturnType<typeof setup>>,id=r.f.seed.run.id){await r.f.c.command('cancel',{target:id});for(const a of (await r.f.c.command('runtime',{target:id})).attempts)await r.f.c.command('ownership',{target:a.id});await r.f.c.command('finish_pause',{target:id});await r.f.c.command('reconcile_writer',{target:id});await r.f.c.command('checkpoint',{target:id});}

test('cancelled work observes the actual configured PR at queue and admission; closed, moved, foreign and lost markers are refused',{timeout:90000},async t=>{
 const r=await setup(t);await cancel(r);const p=pub(r);await r.f.c.command('save_publication',{input:p});let pr=pullRequest(p);r.setHandler((req,res)=>res.end(JSON.stringify(req.url!.endsWith('/pullrequests/9')?pr:{uuid:repository.uuid,full_name:'fixture/repository'})));
 for(const mutate of [(v:any)=>v.state='MERGED',(v:any)=>v.description='Lost marker',(v:any)=>v.source.branch.name='other',(v:any)=>v.source.commit.hash='a'.repeat(40),(v:any)=>v.destination.repository.uuid='{11111111-1111-4111-8111-111111111111}']){pr=pullRequest(p);mutate(pr);await assert.rejects(r.f.c.command('remote_verify',{target:r.f.seed.run.id}),/held/);}
 pr=pullRequest(p);const input={requestId:randomUUID(),revision:(await r.f.c.command('task',{target:r.f.seed.task.id})).revision,roleId:'coordinator'};const next=await r.f.c.command('queue_later',{target:r.f.seed.run.id,input,roles});assert.equal(next.continuedFrom.publicationId,p.id);pr.state='DECLINED';await assert.rejects(r.f.c.command('prepare',{target:next.id,input:next.launch.repository}),/closed|merged/);assert.equal(await r.f.c.command('work_record',{target:next.id}),null);pr=pullRequest(p);const prepared=await r.f.c.command('prepare',{target:next.id,input:next.launch.repository});assert.equal(prepared.path,r.work.path);assert.equal(prepared.branch,p.branch);
});

test('a late PR read cannot verify changed retained receipts and close drains the registered HTTP observer',{timeout:60000},async t=>{
 const r=await setup(t);await cancel(r);const p=pub(r);await r.f.c.command('save_publication',{input:p});let arrived!:()=>void;let entered=new Promise<void>(resolve=>arrived=resolve);let respond!:()=>void;
 r.setHandler((req,res)=>{if(req.url!.endsWith('/pullrequests/9')){respond=()=>res.end(JSON.stringify(pullRequest(p)));arrived();}else res.end(JSON.stringify({uuid:repository.uuid,full_name:'fixture/repository'}));});
 const first=r.f.c.command('remote_verify',{target:r.f.seed.run.id});const stale=assert.rejects(first,/changed/);await entered;await r.f.c.command('save_publication',{input:{...p,revision:{...p.revision,head:'a'.repeat(40)}}});respond();await stale;
 await r.f.c.command('save_publication',{input:p});entered=new Promise<void>(resolve=>arrived=resolve);const second=r.f.c.command('remote_verify',{target:r.f.seed.run.id});const stopped=assert.rejects(second,/stopp/);await entered;let closed=false;const closing=r.f.c.command('remote_close').then(()=>closed=true);await delay(50);assert.equal(closed,false);assert.equal((await r.f.c.command('run',{target:r.f.seed.run.id})).status,'cancelled');respond();await stopped;await closing;assert.equal(closed,true);
});

test('operator retry decisions are strict, normalized and replayed before another held target or fetch',{timeout:90000},async t=>{
 const r=await setup(t);r.move('source.txt','conflicting target\n');r.move('AGENTS.md','changed project instructions\n');const first=await r.refresh();assert.equal(first.refresh.resolutionWorkspace.state,'operator_required');const count=countFetch(r);
 await assert.rejects(r.refresh(),/operator reason/);await assert.rejects(r.refresh(randomUUID(),{conflictRequestId:randomUUID(),reason:'Retry'}),/operator reason/);await assert.rejects(r.refresh(randomUUID(),{conflictRequestId:first.refresh.requestId,reason:' '}),/Invalid/);assert.equal(countFetch(r),count);
 const key=randomUUID(),decision={conflictRequestId:first.refresh.requestId,reason:'  Reviewed the retained conflict. Retry this target.  '};const retry=await r.refresh(key,decision);assert.equal(retry.refresh.supersedes.reason,decision.reason.trim());assert.equal((await r.f.record()).refreshes[0].supersededBy.requestId,key);assert.deepEqual(await r.refresh(key,{...decision,reason:decision.reason.trim()}),retry);assert.deepEqual(await r.refresh(first.refresh.requestId),{changed:false,refresh:(await r.f.record()).refreshes[0]});await assert.rejects(r.refresh(key,{...decision,reason:'Different decision'}),/different operator decision/);assert.equal(countFetch(r),count+1);
});

test('queued fetch rechecks cancellation at actual dispatch and never creates its target ref',{timeout:60000},async t=>{
 const r=await setup(t);const latch=gitLatch(r.f.root,'fetch');t.after(()=>latch.restore());await r.reopen();latch.arm();const first=r.f.c.command('remote_fetch',{target:r.f.seed.run.id,key:randomUUID()});await latch.blocked();const key=randomUUID();const queued=r.f.c.command('remote_fetch',{target:r.f.seed.run.id,key});const refused=assert.rejects(queued,/cancelled|admission changed/);await delay(30);await r.f.c.command('desired',{target:r.f.seed.run.id,input:'cancelled'});latch.release();await first;await refused;assert.throws(()=>r.f.git('rev-parse',`refs/crewroom/${r.f.seed.run.id}/refresh/${key}/target`));
});

test('configuration and target request boundaries reject ambiguous policies and malformed replay keys before Git',{timeout:60000},async t=>{
 const r=await setup(t);for(const bad of [{statusKeys:[],pipelineSteps:['Tests']},{statusKeys:['a','a'],pipelineSteps:['Tests']},{statusKeys:['😀'.repeat(201)],pipelineSteps:['Tests']},{...policy,optionalSteps:null},{...policy,extra:true}]) {
  await assert.rejects(r.reopen({publicationRemote:{connections:[{...r.connection,policy:bad}]}}),/policy|request fields/);
 }
 await r.reopen({publicationRemote:{connections:[{...r.connection,policy:{statusKeys:['😀'.repeat(200)],pipelineSteps:['Tests']}}]}});
 for(const input of [{requestId:'bad'},{requestId:randomUUID(),extra:true},{requestId:randomUUID(),operatorDecision:null},{requestId:randomUUID(),operatorDecision:{conflictRequestId:randomUUID(),reason:'😀'.repeat(4001)}}])await assert.rejects(r.f.c.command('target_request',{target:r.f.seed.run.id,input}),/Invalid|Unexpected|UUID|fields/);
 assert.equal(countFetch(r),0);assert.equal(r.http,0);
});

test('unverified remote journal cleanup remains an error on repeated close and is not hidden by observer unavailability',{timeout:60000},async t=>{
 const r=await setup(t);await r.f.c.command('remote_observe',{target:r.f.seed.run.id});const item=r.intents()[0];const dir=join(r.f.config.authority,'publication',`${r.f.seed.run.id}.observe`);const file=join(dir,item.descriptor.token+'.json'),original=readFileSync(file);const corrupt=JSON.parse(original.toString());corrupt.spec.args.push('--changed');writeFileSync(file,JSON.stringify(corrupt));
 await assert.rejects(r.f.c.command('remote_observe',{target:r.f.seed.run.id}),/unavailable/);for(let i=0;i<2;i++)await assert.rejects(r.f.c.command('remote_close'),/identity changed/);writeFileSync(file,original);await r.reopen();assert.equal(await r.f.c.command('remote_observe',{target:r.f.seed.run.id}),r.candidate.revision.base);
});
