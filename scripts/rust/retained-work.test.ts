import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {existsSync,mkdirSync,readFileSync,realpathSync,renameSync,rmSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test,type TestContext} from 'node:test';
import {DatabaseSync} from 'node:sqlite';
import {defaultRoles} from '../../server/roles';
import {fixture,alter,waitFor} from './target-fixture';
import {describeCheckedOwned,prepareCheckedOwned,launchCheckedOwned,inspectCheckedOwned,stopOwned} from './process-adapter';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {once} from 'node:events';

const roles=defaultRoles.map(r=>({...r,model:'fixture/model',effort:'low'}));
async function cancelled(t:TestContext,dirty=false) {
 const f=fixture(t);const {work,candidate}=await f.candidate();
 if(dirty)writeFileSync(join(work.path,'source.txt'),'Saved unfinished edits\n');
 const cancel=async(id=f.seed.run.id)=>{
  await f.c.command('cancel',{target:id});const runtime=await f.c.command('runtime',{target:id});for(const a of runtime.attempts)await f.c.command('ownership',{target:a.id});await f.c.command('finish_pause',{target:id});await f.c.command('reconcile_writer',{target:id});await f.c.command('checkpoint',{target:id});
 };
 await cancel();
 // FOR-84: the native queue admits a coding successor only under an accepted implementation plan,
 // so the operator acceptance goes through the same public plan gate before any continuation is queued.
 // A moved target or changed brief needs a re-accepted plan; the continuation itself stays pinned to the retained base.
 let prior:any;
 const accept=async()=>{
  const task=await f.c.command('task',{target:f.seed.task.id});const source=await f.c.command('plan_source',{target:task.id,input:{revision:task.revision}});
  const plan=await f.c.command('plan_save',{target:task.id,input:{requestId:randomUUID(),revision:task.revision,sourceId:source.id,...(prior?{predecessorId:prior.id}:{}),content:{schemaVersion:1,goal:task.brief,scope:[task.brief],exclusions:[],assumptions:[],decisions:[],workPackages:[{id:'fixture',name:'Continue retained work',dependsOn:[],outputs:['A checked change'],criteria:task.criteria.map((_:unknown,i:number)=>i),validation:['Fresh checks and independent review']}]}}});
  await f.c.command('plan_decide',{target:plan.id,input:{requestId:randomUUID(),revision:task.revision,sourceId:source.id,decision:'accept'}});prior=plan;return plan;
 };
 await accept();
 const request=async()=>({requestId:randomUUID(),revision:(await f.c.command('task',{target:f.seed.task.id})).revision,roleId:'coordinator'});
 const queue=async(input?:any,id=f.seed.run.id)=>f.c.command('queue_later',{target:id,input:input??await request(),roles});
 return {f,work,candidate,cancel,request,queue,accept,prepare:(run:any)=>f.c.command('prepare',{target:run.id,input:run.launch.repository})};
}

test('cancelled dirty work transfers once to the same branch and survives reopen with fresh evidence',{timeout:90000},async t=>{
 const {f,work,cancel,request,queue,prepare}=await cancelled(t,true);const history=await f.c.command('record',{target:f.seed.run.id});const attempts=(await f.c.command('runtime',{target:f.seed.run.id})).attempts;
 const input=await request();const [next,again]=await Promise.all([queue(input),queue(input)]);assert.deepEqual(again,next);assert.equal((await f.c.command('work_record',{target:next.id})),null);await assert.rejects(queue(),/already has|open run/);
 await assert.rejects(f.c.command('prepare',{target:next.id,input:{mode:'new'}}),/retain the captured/);await f.reopen();const prepared=await prepare(next);assert.equal(prepared.path,work.path);assert.equal(prepared.branch,work.branch);assert.equal(readFileSync(join(prepared.path,'source.txt'),'utf8'),'Saved unfinished edits\n');assert.equal((await f.record()).transferredTo,next.id);assert.equal(await f.c.command('record',{target:next.id}),null);assert.deepEqual(await prepare(next),prepared);assert.deepEqual(await queue(input),next);
 assert.deepEqual(await f.c.command('record',{target:f.seed.run.id}),history);assert.deepEqual((await f.c.command('runtime',{target:f.seed.run.id})).attempts,attempts);assert.equal((await f.c.command('run',{target:f.seed.run.id})).status,'cancelled');await assert.rejects(f.c.command('claim',{target:f.seed.recipient.attempt.id}),/Only an active specialist may own repository writes/);
 await cancel(next.id);const later=await queue(undefined,next.id);assert.equal((await prepare(later)).path,work.path);assert.equal((await f.c.command('work_record',{target:next.id})).transferredTo,later.id);
});

test('keyed continuation replay precedes missing source and provider access',{timeout:60000},async t=>{
 const {f,work,request,queue}=await cancelled(t);const input=await request();const next=await queue(input);const moved=work.path+'-temporarily-moved';renameSync(work.path,moved);try {
  assert.deepEqual(await queue(input),next);await assert.rejects(queue({...input,roleId:'reviewer'}),/different content/);
 }finally{renameSync(moved,work.path);}
});

test('source changes and unresolved attempts block queueing and queued admission without losing edits',{timeout:90000},async t=>{
 const {f,work,candidate,request,queue,prepare}=await cancelled(t);
 const source=readFileSync(join(work.path,'source.txt'),'utf8');writeFileSync(join(work.path,'source.txt'),'External edits\n');await assert.rejects(queue(),/head or saved edits changed/);writeFileSync(join(work.path,'source.txt'),source);
 f.gitAt(work.path,'commit','--allow-empty','-m','External commit');await assert.rejects(queue(),/head or saved edits changed/);f.gitAt(work.path,'reset','--hard',candidate.revision.head);
 const instructions=readFileSync(join(work.path,'AGENTS.md'),'utf8');writeFileSync(join(work.path,'AGENTS.md'),'Changed instructions\n');await assert.rejects(queue(),/saved edits changed|instructions changed/);writeFileSync(join(work.path,'AGENTS.md'),instructions);
 await f.c.command('ownership',{target:f.seed.recipient.attempt.id,stopped:false});await assert.rejects(queue(),/ownership is unresolved/);await f.c.command('ownership',{target:f.seed.recipient.attempt.id});
 const input=await request();const next=await queue(input);writeFileSync(join(work.path,'source.txt'),'Changed while queued\n');await assert.rejects(prepare(next),/saved edits changed/);assert.equal(await f.c.command('work_record',{target:next.id}),null);assert.equal((await f.record()).transferredTo,undefined);writeFileSync(join(work.path,'source.txt'),source);
 await f.c.command('cancel_queued',{target:next.id,input:{requestId:randomUUID()}});await assert.rejects(prepare(next),/newer control|linked successor|retained ownership/);assert.deepEqual(await queue(input),next);const replacement=await queue();assert.notEqual(replacement.id,next.id);assert.equal((await prepare(replacement)).path,work.path);
});

test('failed transfer is atomic and pre-launch replay revalidates the original source after reopening',{timeout:90000},async t=>{
 const {f,work,queue,prepare}=await cancelled(t,true);const next=await queue();const db=new DatabaseSync(f.seed.database);db.exec(`CREATE TRIGGER fail_transfer BEFORE INSERT ON repository_work WHEN NEW.run_id='${next.id}' BEGIN SELECT RAISE(ABORT,'fixture transfer failure'); END;`);db.close();
 await assert.rejects(prepare(next),/fixture transfer failure/);assert.equal((await f.record()).transferredTo,undefined);assert.equal(await f.c.command('work_record',{target:next.id}),null);assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'Saved unfinished edits\n');const repair=new DatabaseSync(f.seed.database);repair.exec('DROP TRIGGER fail_transfer');repair.close();await f.reopen();const prepared=await prepare(next);
 writeFileSync(join(work.path,'source.txt'),'Changed after transfer\n');await f.reopen();await assert.rejects(prepare(next),/saved edits changed/);assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'Changed after transfer\n');writeFileSync(join(work.path,'source.txt'),'Saved unfinished edits\n');assert.deepEqual(await prepare(next),prepared);
});

function publication(f:ReturnType<typeof fixture>,work:any,candidate:any){return {id:randomUUID(),runId:f.seed.run.id,repositoryPath:work.repository.path,repository:{workspace:'fixture',slug:'repository',uuid:'{00000000-0000-4000-8000-000000000011}'},branch:work.branch,target:'main',candidateId:candidate.id,revision:candidate.revision,pr:{id:9,state:'confirmed',providerState:'OPEN',url:'https://bitbucket.org/fixture/repository/pull-requests/9'},push:{state:'confirmed'}};}
function observer(f:ReturnType<typeof fixture>){const path=join(f.root,'continuation-observer');mkdirSync(path);writeFileSync(join(path,'result.json'),JSON.stringify({verified:true}));return path;}

test('retained PRs need a fresh observer at queue and admission, including repeated inherited publication',{timeout:90000},async t=>{
 const {f,work,candidate,cancel,request,queue,prepare}=await cancelled(t);const pub=publication(f,work,candidate);await f.c.command('save_publication',{input:pub});await assert.rejects(queue(),/PR verification is unavailable/);const path=observer(f);await f.reopen({continuationObserver:path});writeFileSync(join(path,'result.json'),JSON.stringify({verified:false}));await assert.rejects(queue(),/Scripted PR verification/);writeFileSync(join(path,'result.json'),JSON.stringify({verified:true}));
 const input=await request();const next=await queue(input);assert.equal(next.continuedFrom.publicationId,pub.id);writeFileSync(join(path,'result.json'),JSON.stringify({verified:false}));assert.deepEqual(await queue(input),next);await assert.rejects(prepare(next),/Scripted PR verification/);writeFileSync(join(path,'result.json'),JSON.stringify({verified:true}));await prepare(next);await cancel(next.id);assert.equal(await f.c.command('publication',{target:next.id}),null);
 const later=await queue(undefined,next.id);assert.equal(later.continuedFrom.publicationId,pub.id);assert.equal(later.continuedFrom.prId,9);assert.equal(readFileSync(join(path,'called'),'utf8'),next.id);assert.equal((await prepare(later)).path,work.path);assert.equal(await f.c.command('record',{target:later.id}),null);assert.deepEqual(await f.c.command('publication',{target:f.seed.run.id}),pub);
});

test('observer awaits cannot admit changed files, configuration, publication or newer successor control',{timeout:120000},async t=>{
 for(const change of ['files','configuration','publication','requirements','control']) {
  const {f,work,candidate,queue,prepare}=await cancelled(t);const pub=publication(f,work,candidate);await f.c.command('save_publication',{input:pub});const path=observer(f);await f.reopen({continuationObserver:path});
  const next=change==='control'?await queue():undefined;rmSync(join(path,'called'),{force:true});writeFileSync(join(path,'hold'),'');const pending=(next?prepare(next):queue()).then(value=>({value}),error=>({error}));await waitFor(join(path,'called'));
  if(change==='files')writeFileSync(join(work.path,'source.txt'),'External work during observation\n');
  if(change==='configuration')alter(f,'repositories',work.repository.id,r=>r.targetBranch='different-target');
  if(change==='publication')await f.c.command('save_publication',{input:{...pub,pr:{...pub.pr,providerState:'DECLINED'}}});
  if(change==='requirements'){const task=await f.c.command('task',{target:f.seed.task.id});await f.c.command('refine',{target:f.seed.task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Changed while verifying',criteria:['Reaccept this request']}});}
  if(change==='control')await f.c.command('desired',{target:next.id,input:'cancelled'});
  writeFileSync(join(path,'release'),'');assert.ok('error' in await pending);assert.equal((await f.record()).transferredTo,undefined);if(next)assert.equal(await f.c.command('work_record',{target:next.id}),null);if(change==='files')assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'External work during observation\n');
 }
});

test('a successor accepts the new brief while preserving source history and rejecting a mismatched PR',{timeout:60000},async t=>{
 const {f,work,candidate,queue,accept,prepare}=await cancelled(t);const prior=await f.c.command('run',{target:f.seed.run.id});const task=await f.c.command('task',{target:f.seed.task.id});await f.c.command('refine',{target:f.seed.task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Accept new successor scope',criteria:['New scope needs new evidence']}});await accept();
 const pub=publication(f,work,candidate);await f.c.command('save_publication',{input:{...pub,branch:'unrelated-branch'}});const path=observer(f);await f.reopen({continuationObserver:path});await assert.rejects(queue(),/PR does not match/);await f.c.command('save_publication',{input:pub});const next=await queue();const prepared=await prepare(next);assert.notEqual(prepared.requirementsHash,work.requirementsHash);assert.equal(prepared.checkpoints[0].revision.requirementsHash,prepared.requirementsHash);assert.equal((await f.c.command('run',{target:f.seed.run.id})).taskRevision,prior.taskRevision);assert.equal(next.taskSnapshot.brief,'Accept new successor scope');assert.equal(await f.c.command('record',{target:next.id}),null);
});

test('moved targets remain pinned across repeated cancellation until fresh evidence and explicit refresh',{timeout:120000},async t=>{
 for(const mode of ['moved','returned','repeated']) {
  const {f,work,candidate,cancel,queue,accept,prepare}=await cancelled(t);const target=f.move();await accept();let next=await queue();let prepared=await prepare(next);assert.equal(prepared.base,work.base);assert.equal(prepared.targetRef,work.base);assert.equal(prepared.continuationTargetRef,work.targetRef);
  if(mode==='repeated'){await cancel(next.id);next=await queue(undefined,next.id);prepared=await prepare(next);assert.equal(prepared.continuationTargetRef,work.targetRef);}
  const team=[];for(const [name,coordinator] of [['Coordinator',true],['Writer',false]] as const)team.push(await f.c.command('admit',{target:next.id,input:{name,coordinator,role:roles.find(r=>r.id===(coordinator?'coordinator':'implementer')),model:f.seed.sender.assignment.model,input:'Inspect retained implementation against the accepted brief.'}}));
  await f.c.command('activate',{target:next.id});await f.c.command('ownership',{target:team[1].attempt.id});const fresh=await f.c.command('candidate',{target:next.id,input:[team[1].attempt.id]});assert.notEqual(fresh.id,candidate.id);assert.match((await f.c.command('assess',{target:next.id})).blockers.join(' '),/Refresh from the target/);
  await f.c.command('pause',{target:next.id});for(const member of team)await f.c.command('ownership',{target:member.attempt.id});await f.c.command('finish_pause',{target:next.id});if(mode==='returned')f.git('reset','--hard',work.base);
  const key=randomUUID();const refreshed=await f.c.command('refresh',{target:next.id,key,targetRef:work.targetRef,sourceCandidateId:fresh.id});assert.equal(refreshed.work.base,mode==='returned'?work.base:target);assert.equal(refreshed.work.continuationTargetRef,undefined);const associated=await f.c.command('associate_refresh',{target:next.id,key});assert.equal(associated.id===fresh.id,mode==='returned');assert.equal((await f.c.command('assess',{target:next.id})).ready,false);
 }
});

test('old-owner checked Git barriers survive controller death and seal unlaunched work before transfer',{timeout:120000},async t=>{
 for(const mode of ['live','unlaunched']) {
  const {f,work,queue,prepare}=await cancelled(t);const prior=f.intents()[0].descriptor;const authority={root:prior.controllerRoot,epoch:prior.epoch,lost:new Promise<void>(()=>{}),assertCurrent(){assert.equal(JSON.parse(readFileSync(join(prior.controllerRoot,'epoch.json'),'utf8')).epoch,prior.epoch);},async close(){}};
  const latch=gitLatch(f.root,'status');const token=randomUUID();const root=join(f.config.root,'.git-operations',f.seed.run.id,token);const command=realpathSync(join(f.root,'git-latch','bin','git'));const args=['-c','core.hooksPath=/dev/null','-c','core.fsmonitor=false','-c','submodule.recurse=false','status','--porcelain'];const options={cwd:work.path,env:{PATH:process.env.PATH??'',HOME:process.env.HOME??'',LANG:'C'}};const d=describeCheckedOwned(root,token,command,args,options,authority);
  // Persist a real checked Git operation under the cancelled SOURCE run. Agent
  // identities remain synthetic; the process, manifest and kernel lease are real.
  writeFileSync(root+'.json',JSON.stringify({descriptor:d,spec:{root,token,command,args,...options,epoch:authority.epoch,controllerRoot:authority.root}}));prepareCheckedOwned(root,token,command,args,options,authority);
  let exit:Promise<any>|undefined;const manifest=join(root,'launch.json');const raw=readFileSync(manifest);
  try {
   if(mode==='live'){
    latch.arm();const child=launchCheckedOwned(d,authority);exit=once(child,'exit');await waitFor(join(f.root,'git-latch','blocked'));assert.equal((await inspectCheckedOwned(d)).state,'live');f.c.proc.kill('SIGKILL');await f.c.exit;latch.restore();await f.reopen();await assert.rejects(queue(),/live or uncertain/);assert.equal((await f.record()).transferredTo,undefined);await stopOwned(root);await exit;latch.release();
   }else{
    await f.c.command('verify_cancelled',{target:f.seed.run.id});assert.equal((await inspectCheckedOwned(d)).state,'stopped');assert.ok(existsSync(join(root,'seal.json')));assert.throws(()=>launchCheckedOwned(d,authority),/already launched or sealed/);
   }
   writeFileSync(manifest,Buffer.concat([raw,Buffer.from(' ')]));await assert.rejects(queue(),/live or uncertain/);writeFileSync(manifest,raw);const next=await queue();assert.equal((await prepare(next)).path,work.path);
  }finally{writeFileSync(manifest,raw);latch.release();latch.restore();await stopOwned(root);if(exit)await exit;}
 }
});

test('dependency merges must be in the retained accepted base even when the target checkout has advanced',{timeout:60000},async t=>{
 const {f,work,queue,accept,prepare}=await cancelled(t);f.git('checkout','-b','dependency-future');f.git('commit','--allow-empty','-m','Future dependency merge');const merge=f.git('rev-parse','HEAD');f.git('checkout','main');f.git('merge','--ff-only','dependency-future');
 // FOR-84: seeded through the native store, which the retained TypeScript Workbench (schema 26) can no longer open.
 {const task=await f.c.command('create_task',{input:{title:'Upstream fixture',description:'',repositoryId:work.repository.id}});await f.c.command('refine',{target:task.id,input:{revision:1,sourceRevision:1,brief:'Upstream',criteria:['Delivered']}});const refined=await f.c.command('task',{target:task.id});const source=await f.c.command('plan_source',{target:task.id,input:{revision:refined.revision}});
  // An accepted implementation plan is the native gate that makes the queued upstream run a coding run.
  const plan=await f.c.command('plan_save',{target:task.id,input:{requestId:randomUUID(),revision:refined.revision,sourceId:source.id,content:{schemaVersion:1,goal:refined.brief,scope:[refined.brief],exclusions:[],assumptions:[],decisions:[],workPackages:[{id:'fixture',name:'Upstream delivery',dependsOn:[],outputs:['A merged change'],criteria:[0],validation:['Provider merge fixture']}]}}});
  await f.c.command('plan_decide',{target:plan.id,input:{requestId:randomUUID(),revision:refined.revision,sourceId:source.id,decision:'accept'}});const run=await f.c.command('queue',{target:task.id,input:{revision:refined.revision}});assert.equal(run.purpose,'coding');await f.c.command('set_dependencies',{target:f.seed.task.id,input:{requestId:randomUUID(),dependencyRevision:0,dependsOn:[task.id]}});
  // Saved provider observation is a fixture; Git ancestry against the retained base is checked by Rust.
  await f.c.command('save_publication',{input:{id:randomUUID(),runId:run.id,attemptId:randomUUID(),candidateId:randomUUID(),repositoryPath:f.seed.root,repository:{uuid:randomUUID(),workspace:'fixture',slug:'repo'},branch:'dependency-future',target:'main',revision:{base:work.base,head:merge,tree:f.git('rev-parse','HEAD^{tree}'),worktreeHash:'d'.repeat(64),dirty:false,requirementsHash:'e'.repeat(64),instructionsHash:'f'.repeat(64)},title:'Fixture',description:'Fixture',policy:{statusKeys:[],pipelineSteps:[],optionalSteps:[]},createdAt:new Date().toISOString(),push:{state:'confirmed',expectedHead:null},pr:{state:'confirmed',id:1,providerState:'MERGED'},ready:false,blockers:[],merged:{commit:merge,observedAt:new Date().toISOString()}}});
 }await accept();await f.reopen();const next=await queue();assert.equal(await f.c.command('dependency_hold',{target:next.id}),null);await assert.rejects(prepare(next),/verified merge.*prepared base/);assert.equal((await f.record()).transferredTo,undefined);assert.equal(await f.c.command('work_record',{target:next.id}),null);
});
