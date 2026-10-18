import assert from 'node:assert/strict';
import {existsSync,readFileSync,rmSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {fixture,alter,waitFor} from './target-fixture';
import {randomUUID} from 'node:crypto';
import {DatabaseSync} from 'node:sqlite';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
import {gitLatch} from '../../tests/fixtures/git-latch';

import {ready} from './conflict-promotion-fixture';

test('promotion atomically records lineage, completion and notice before cleanup and replays after reopen',{timeout:180000},async t=>{
 const {f,candidate,work,r,target,writer,completion,complete}=await ready(t);
 const promoted=await complete();assert.equal(promoted.refreshedFrom,candidate.id);assert.deepEqual(promoted.implementationAttempts,[...candidate.implementationAttempts,writer]);assert.equal(promoted.revision.base,target);assert.equal(promoted.revision.dirty,false);assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'Resolved source\n');
 const record=await f.record();const refresh=record.refreshes.at(-1);assert.equal(refresh.state,'completed');assert.equal(refresh.candidateId,promoted.id);assert.equal(refresh.resolutionWorkspace.operationId,completion.id);assert.equal(refresh.resolutionWorkspace.state,'promoted');
 const notices=await f.c.command('notices',{target:f.seed.run.id});assert.equal(notices.filter((n:any)=>n.payload.kind==='target_refresh').length,1);
 assert.equal(existsSync(r.resolutionWorkspace.path),false,f.c.stderr);assert.equal(f.git('for-each-ref','--format=%(objectname)',`refs/heads/${r.resolutionWorkspace.branch}`),'');
 await f.reopen();assert.deepEqual(await complete(),promoted);assert.equal((await f.c.command('status',{target:f.seed.run.id})).revision.head,promoted.revision.head);assert.equal((await f.c.command('notices',{target:f.seed.run.id})).length,notices.length);
});

function sql(f:ReturnType<typeof fixture>,query:string){const db=new DatabaseSync(f.seed.database);try{db.exec(query);}finally{db.close();}}
test('candidate association and notice failures roll back all evidence and retain the workspace',{timeout:180000},async t=>{
 const {f,candidate,work,r,completion,complete}=await ready(t);
 sql(f,"CREATE TRIGGER fail_association BEFORE UPDATE ON repository_work WHEN json_extract(NEW.data,'$.refreshes[0].candidateId') IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture association failure'); END;");
 await assert.rejects(complete(),/fixture association failure/);assert.equal((await f.record()).refreshes.at(-1).state,'completed');assert.notEqual(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
 for(const stage of ['association','notice']) {
  const record=await f.c.command('record',{target:f.seed.run.id});assert.equal(record.candidates.length,1);assert.equal(record.completions.find((c:any)=>c.id===completion.id).state,'requested');assert.equal((await f.record()).refreshes.at(-1).candidateId,undefined);assert.equal((await f.c.command('notices',{target:f.seed.run.id})).length,0);assert.ok(existsSync(r.resolutionWorkspace.path));
  if(stage==='association'){sql(f,"DROP TRIGGER fail_association; CREATE TRIGGER fail_notice BEFORE INSERT ON delivery_notices BEGIN SELECT RAISE(ABORT,'fixture notice failure'); END;");await f.reopen();await assert.rejects(complete(),/fixture notice failure/);}
 }
 sql(f,'DROP TRIGGER fail_notice');alter(f,'runs',f.seed.run.id,r=>r.status='attention_required');await f.reopen();const result=await complete();assert.equal(result.refreshedFrom,candidate.id);assert.equal(existsSync(r.resolutionWorkspace.path),false);assert.equal((await f.c.command('notices',{target:f.seed.run.id})).length,1);
});

test('cleanup retains external and ignored files, then reconciles absent worktree and exact branch',{timeout:180000},async t=>{
 const {f,r,completion,complete}=await ready(t);await f.c.command('commit',{target:f.seed.run.id,input:completion});
 const path=r.resolutionWorkspace.path;writeFileSync(join(path,'operator.txt'),'Keep external data\n');const promoted=await complete();assert.ok(existsSync(path));assert.equal(readFileSync(join(path,'operator.txt'),'utf8'),'Keep external data\n');
 await assert.rejects(f.c.command('cleanup_conflict',{target:f.seed.run.id,key:r.requestId}),/changed|untracked/);rmSync(join(path,'operator.txt'));
 const exclude=f.gitAt(path,'rev-parse','--git-path','info/exclude');writeFileSync(exclude,'ignored.txt\n',{flag:'a'});writeFileSync(join(path,'ignored.txt'),'Keep ignored data\n');await assert.rejects(f.c.command('cleanup_conflict',{target:f.seed.run.id,key:r.requestId}),/changed|untracked/);rmSync(join(path,'ignored.txt'));
 // Actual operator removal models the stopped, lost-reply worktree removal window.
 f.git('worktree','remove',path);await f.reopen();await f.c.command('cleanup_conflict',{target:f.seed.run.id,key:r.requestId});assert.equal(f.git('for-each-ref','--format=%(objectname)',`refs/heads/${r.resolutionWorkspace.branch}`),'');assert.deepEqual(await complete(),promoted);
});

test('a stopped unlaunched promotion is sealed and rejected without retrying its reset',{timeout:180000},async t=>{
 const {f,work,r,candidate,observer,complete}=await ready(t,true);writeFileSync(join(observer,'hold-2'),'');await f.reopen({targetObserver:observer});const pending=complete().catch(e=>e);await waitFor(join(observer,'called-2'),30000);const saved=(await f.record()).refreshes.at(-1).resolutionWorkspace;assert.equal(saved.state,'promoting');assert.equal(existsSync(saved.promotionProcess.root),false);
 f.c.proc.kill('SIGKILL');await f.c.exit;await pending;rmSync(join(observer,'hold-2'));await f.reopen({targetObserver:observer});await assert.rejects(complete(),/stopped without applying/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal((await inspectCheckedOwned(saved.promotionProcess)).state,'stopped');assert.ok(existsSync(r.resolutionWorkspace.path));const coding=await f.c.command('record',{target:f.seed.run.id});assert.equal(coding.completions.at(-1).state,'rejected');
});

test('actual controller SIGKILL during reset retains ownership and rejects only after verified stop',{timeout:180000},async t=>{
 const {f,work,r,candidate,complete}=await ready(t);const latch=gitLatch(f.root,'reset');try {
  await f.reopen();latch.arm();const pending=complete().catch(e=>e);await waitFor(join(f.root,'git-latch','blocked'),40000);const p=(await f.record()).refreshes.at(-1).resolutionWorkspace.promotionProcess;assert.equal((await inspectCheckedOwned(p)).state,'live');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await f.reopen();await assert.rejects(complete(),/live|uncertain|ownership/i);assert.equal((await f.record()).refreshes.at(-1).resolutionWorkspace.state,'promoting');await stopOwned(p.root);latch.release();latch.restore();await f.reopen();await assert.rejects(complete(),/stopped without applying/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.ok(existsSync(r.resolutionWorkspace.path));
 }finally{latch.release();latch.restore();}
});

test('killed post-effect reset recovers using its saved executable and rejects corrupted process proof',{timeout:180000},async t=>{
 const {f,work,r,candidate,complete}=await ready(t);const latch=gitLatch(f.root,'reset');const shim=join(f.root,'git-latch','bin','git');writeFileSync(shim,readFileSync(shim,'utf8').replace('      result=$?','      result=$?\n      : > "$state/effected"\n      while [ ! -e "$state/after" ]; do sleep 0.01; done'));
 try {
  await f.reopen();latch.arm();const pending=complete().catch(e=>e);await waitFor(join(f.root,'git-latch','blocked'),40000);const p=(await f.record()).refreshes.at(-1).resolutionWorkspace.promotionProcess;latch.release();await waitFor(join(f.root,'git-latch','effected'));assert.notEqual(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await stopOwned(p.root);latch.restore();rmSync(shim);await f.reopen();
  const manifest=join(p.root,'launch.json');const original=readFileSync(manifest);writeFileSync(manifest,Buffer.concat([original,Buffer.from(' ')]));await assert.rejects(complete(),/manifest|ownership|uncertain|identity/i);assert.equal((await f.record()).refreshes.at(-1).resolutionWorkspace.state,'promoting');writeFileSync(manifest,original);
  alter(f,'repository_work',f.seed.run.id,w=>w.refreshes.at(-1).resolutionWorkspace.promotionProcess.args.push('--hard'));await assert.rejects(complete(),/process|command|identity/i);alter(f,'repository_work',f.seed.run.id,w=>w.refreshes.at(-1).resolutionWorkspace.promotionProcess=p);
  alter(f,'runs',f.seed.run.id,r=>r.status='attention_required');const promoted=await complete();assert.equal(promoted.refreshedFrom,candidate.id);assert.equal(existsSync(r.resolutionWorkspace.path),false);
 }finally{writeFileSync(join(f.root,'git-latch','after'),'');latch.release();latch.restore();}
});

test('target movement before dispatch rejects intact work while post-reset observation failure retains local evidence',{timeout:300000},async t=>{
 for(const phase of ['before','unavailable','after']) {
  const {f,work,r,candidate,observer,complete}=await ready(t,true);writeFileSync(join(observer,phase==='after'?'result-3.json':'result-1.json'),JSON.stringify(phase==='before'?{head:candidate.revision.base}:{error:true}));await f.reopen({targetObserver:observer});
  if(phase==='before'){await assert.rejects(complete(),/live target changed/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal((await f.c.command('record',{target:f.seed.run.id})).completions.at(-1).state,'rejected');assert.ok(existsSync(r.resolutionWorkspace.path));}
  else if(phase==='unavailable'){await assert.rejects(complete(),/observation unavailable/);const saved=(await f.record()).refreshes.at(-1);assert.equal(saved.resolutionWorkspace.state,'ready');assert.equal(saved.resolutionWorkspace.promotionProcess,undefined);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);}
  else{const result=await complete();assert.equal(result.refreshedFrom,candidate.id);assert.equal((await f.c.command('run',{target:f.seed.run.id})).status,'attention_required');assert.deepEqual(await complete(),result);}
 }
});

test('source or operator control changes during target observation preserve the saved unlaunched intent',{timeout:360000},async t=>{
 for(const mode of ['original','isolated','requirements','attention']) {
  const {f,work,r,candidate,observer,complete}=await ready(t,true);writeFileSync(join(observer,'hold-2'),'');await f.reopen({targetObserver:observer});const pending=complete().then(value=>({value}),error=>({error}));await waitFor(join(observer,'called-2'),40000);const p=(await f.record()).refreshes.at(-1).resolutionWorkspace.promotionProcess;
  if(mode==='original')writeFileSync(join(work.path,'external.txt'),'External source\n');
  if(mode==='isolated')writeFileSync(join(r.resolutionWorkspace.path,'source.txt'),'External resolution\n');
  if(mode==='attention')alter(f,'runs',f.seed.run.id,r=>r.status='attention_required');
  if(mode==='requirements'){const task=await f.c.command('task',{target:f.seed.task.id});await f.c.command('refine',{target:f.seed.task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Changed accepted scope',criteria:['Changed requirement']}});}
  writeFileSync(join(observer,'release-2'),'');assert.ok('error' in await pending);assert.equal(existsSync(p.root),false);assert.equal((await f.record()).refreshes.at(-1).resolutionWorkspace.state,'promoting');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
 }
});

test('changed source identities and revisions cannot bypass conflict completion evidence',{timeout:180000},async t=>{
 const {f,work,r,candidate,completion,complete}=await ready(t);
 const coding=await f.c.command('record',{target:f.seed.run.id});await f.c.command('policy_fixture',{target:f.seed.run.id,input:{candidates:[...coding.candidates,{...candidate,id:randomUUID()}]}});await assert.rejects(complete(),/source candidate|ownership/);await f.c.command('policy_fixture',{target:f.seed.run.id,input:{candidates:coding.candidates}});
 await f.c.command('commit',{target:f.seed.run.id,input:completion});await assert.rejects(f.c.command('associate_refresh',{target:f.seed.run.id,key:r.requestId}),/completion transaction/);
 const head=f.gitAt(work.path,'rev-parse','HEAD');writeFileSync(join(work.path,'extra.txt'),'External committed source\n');f.gitAt(work.path,'add','extra.txt');f.gitAt(work.path,'commit','-m','external source');await assert.rejects(complete(),/Promoted source changed/);assert.equal((await f.c.command('record',{target:f.seed.run.id})).candidates.length,1);assert.ok(existsSync(r.resolutionWorkspace.path));
 // Restore only the disposable test commit to validate the original exact receipt.
 f.gitAt(work.path,'reset','--hard',head);const promoted=await complete();assert.equal(promoted.refreshedFrom,candidate.id);
});

test('actual controller SIGKILL after workspace removal preserves branch-only cleanup recovery',{timeout:180000},async t=>{
 const {f,r,complete}=await ready(t);const latch=gitLatch(f.root,'update-ref');try {
  await f.reopen();latch.arm();const pending=complete().catch(e=>e);await waitFor(join(f.root,'git-latch','blocked'),60000);assert.equal(existsSync(r.resolutionWorkspace.path),false);const deletions=f.intents().filter(i=>i.spec.args.at(-4)==='update-ref' && i.spec.args.at(-3)==='-d' && i.spec.args.at(-2)===`refs/heads/${r.resolutionWorkspace.branch}`);assert.equal(deletions.length,1);const d=deletions[0].descriptor;assert.equal((await inspectCheckedOwned(d)).state,'live');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await stopOwned(d.root);latch.release();latch.restore();await f.reopen();
  const branch=`refs/heads/${r.resolutionWorkspace.branch}`;const head=f.git('rev-parse',branch);f.git('update-ref',branch,f.git('rev-parse','main'));await assert.rejects(f.c.command('cleanup_conflict',{target:f.seed.run.id,key:r.requestId}),/branch moved/);f.git('update-ref',branch,head);
  const old=f.intents().find(i=>i.spec.cwd===r.resolutionWorkspace.path);const manifest=join(old.descriptor.root,'launch.json');const raw=readFileSync(manifest);writeFileSync(manifest,Buffer.concat([raw,Buffer.from(' ')]));await assert.rejects(f.c.command('cleanup_conflict',{target:f.seed.run.id,key:r.requestId}),/live|uncertain|ownership/i);writeFileSync(manifest,raw);
  await f.c.command('cleanup_conflict',{target:f.seed.run.id,key:r.requestId});assert.equal(f.git('for-each-ref','--format=%(objectname)',branch),'');const replay=await complete();assert.equal(replay.id,(await f.record()).refreshes.at(-1).candidateId);
 }finally{latch.release();latch.restore();}
});
