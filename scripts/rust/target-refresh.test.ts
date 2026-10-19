import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {chmodSync,existsSync,mkdirSync,readFileSync,realpathSync,rmSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {DatabaseSync} from 'node:sqlite';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {fixture,alter,waitFor} from './target-fixture';

test('native refresh preserves scoped source, atomically associates its candidate, and replays before target lookup',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();const to=f.move();const key=randomUUID();const result=await f.refresh(candidate,key);
 assert.equal(result.refresh.state,'completed');assert.equal(result.refresh.toBase,to);assert.equal(result.refresh.sourceCandidateId,candidate.id);assert.equal(result.work.base,to);assert.equal(result.refresh.rebaseRecovery,'rebase_stopped');assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'implementation\n');
 const db=new DatabaseSync(f.seed.database);db.exec("CREATE TRIGGER fail_refresh_association BEFORE UPDATE ON repository_work WHEN json_extract(NEW.data,'$.refreshes[0].candidateId') IS NOT NULL BEGIN SELECT RAISE(ABORT,'fixture association failure'); END;");db.close();
 await assert.rejects(f.c.command('associate_refresh',{target:f.seed.run.id,key}),/fixture association failure/);assert.equal((await f.c.command('record',{target:f.seed.run.id})).candidates.length,1);
 const repair=new DatabaseSync(f.seed.database);repair.exec('DROP TRIGGER fail_refresh_association');repair.close();await f.reopen();
 const next=await f.c.command('associate_refresh',{target:f.seed.run.id,key});assert.equal(next.refreshedFrom,candidate.id);assert.deepEqual(next.implementationAttempts,candidate.implementationAttempts);assert.equal(next.revision.head,result.refresh.toHead);assert.deepEqual(await f.c.command('associate_refresh',{target:f.seed.run.id,key}),next);
 f.git('update-ref','-d','refs/heads/main');const replay=await f.refresh(candidate,key,'refs/heads/absent');assert.equal(replay.refresh.candidateId,next.id);assert.equal((await f.c.command('record',{target:f.seed.run.id})).candidates.length,2);
});

test('unchanged native target names the exact same-head source candidate and performs no Git mutation',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();const count=f.descriptors().length;
 const other={...candidate,id:randomUUID()};const record=await f.c.command('record',{target:f.seed.run.id});await f.c.command('policy_fixture',{target:f.seed.run.id,input:{candidates:[...record.candidates,other]}});
 await assert.rejects(f.refresh(candidate),/source/);const key=randomUUID();const result=await f.refresh(other,key);assert.equal(result.refresh.state,'completed');assert.equal(result.refresh.candidateId,other.id);assert.equal(f.descriptors().length,count);assert.deepEqual(await f.c.command('associate_refresh',{target:f.seed.run.id,key}),other);
 // FOR-84: native-only expectations replace the retained TypeScript RepositoryWork oracle (schema 26 cannot
 // open the native store). The oracle compared the whole refresh receipt except requestId/at; the same fields are
 // derived here from the seeded candidate and actual Git, and a second request key must yield the same receipt.
 const normalize=(r:any)=>{const {requestId,at,...value}=r;return value;};const target=f.git('rev-parse','refs/heads/main');assert.equal(target,candidate.revision.base);
 const expected={fromBase:candidate.revision.base,fromHead:candidate.revision.head,targetRef:'refs/heads/main',sourceCandidateId:other.id,state:'completed',toBase:target,toHead:candidate.revision.head,candidateId:other.id};
 assert.deepEqual(normalize(result.refresh),expected);assert.equal(result.refresh.requestId,key);assert.equal(result.work.base,target);assert.equal(f.gitAt(result.work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal(f.gitAt(result.work.path,'status','--porcelain'),'');
 const again=await f.refresh(other,randomUUID());assert.deepEqual(normalize(again.refresh),expected);assert.equal(f.descriptors().length,count);assert.equal(f.gitAt(result.work.path,'rev-parse','HEAD'),candidate.revision.head);
 assert.deepEqual((await f.record()).refreshes.map(normalize),[expected,expected]);
});

test('native refresh rejects rewritten targets and included custom merge drivers before admitting mutation',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();const count=f.descriptors().length;const base=f.git('rev-parse','HEAD');const ancestor=f.git('rev-parse','HEAD^');f.git('update-ref','refs/heads/main',ancestor);
 await assert.rejects(f.refresh(candidate),/rewritten/);assert.equal((await f.record()).refreshes?.length??0,0);
 f.git('update-ref','refs/heads/main',base);f.git('reset','--hard',base);f.move();const config=join(f.root,'included.gitconfig');writeFileSync(config,'[merge "fixture"]\n driver = touch should-not-execute\n');f.git('config','include.path',config);
 await assert.rejects(f.refresh(candidate),/custom merge drivers/);assert.equal(f.descriptors().length,count);assert.equal(existsSync(join(f.seed.root,'should-not-execute')),false);
});

test('native conflict preparation restores the original before retaining a detached scoped workspace',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','target conflict\n');const result=await f.refresh(candidate);const r=result.refresh;assert.equal(r.state,'conflicted');assert.equal(r.rebaseRecovery,'restored');assert.deepEqual(r.conflicts,['source.txt']);assert.equal(r.resolutionWorkspace.state,'conflicted');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal(f.gitAt(work.path,'status','--porcelain'),'');assert.throws(()=>f.gitAt(r.resolutionWorkspace.path,'symbolic-ref','-q','--short','HEAD'));await assert.rejects(f.c.command('claim',{target:f.seed.recipient.attempt.id}),/conflict|writer|attempt|running/i);await f.reopen();await f.c.command('reconcile_refresh',{target:f.seed.run.id});assert.deepEqual((await f.record()).refreshes.at(-1),r);
});

test('instruction conflicts and dropped source scope restore the original and remain operator-owned',{timeout:90000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');f.move('AGENTS.md','Target instructions\n');const result=await f.refresh(candidate);assert.equal(result.refresh.resolutionWorkspace.state,'operator_required');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
 const g=fixture(t);const c=await g.candidate();g.move('source.txt','implementation\n');const dropped=await g.refresh(c.candidate);assert.equal(dropped.refresh.state,'conflicted');assert.deepEqual(dropped.refresh.conflicts,[]);assert.equal(dropped.refresh.resolutionWorkspace,undefined);assert.equal(g.gitAt(c.work.path,'rev-parse','HEAD'),c.candidate.revision.head);
});

test('fetched native target needs an actual observer; unavailable or changed observations restore exact source',{timeout:120000},async t=>{
 for(const outcome of ['missing','unavailable','moved','matched']) {
  const f=fixture(t);const {candidate,work}=await f.candidate();const target=f.move();const pin=`refs/crewroom/${f.seed.run.id}/${randomUUID()}`;f.git('update-ref',pin,target);
  if(outcome!=='missing'){const observer=join(f.root,'scripted-observer');mkdirSync(observer);writeFileSync(join(observer,'result.json'),JSON.stringify(outcome==='unavailable'?{error:true}:{head:outcome==='moved'?candidate.revision.base:target}));await f.reopen({targetObserver:observer});}
  if(outcome==='matched'){const result=await f.refresh(candidate,randomUUID(),pin);assert.equal(result.refresh.state,'completed');assert.equal(result.refresh.toBase,target);}else{await assert.rejects(f.refresh(candidate,randomUUID(),pin),/unavailable|changed/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal((await f.record()).refreshes.at(-1).rebaseRecovery,'restored');}
 }
});

test('an observer await never authorizes resetting external source or new ownership',{timeout:150000},async t=>{
 for(const mode of ['source','ownership','attention']) {
  const f=fixture(t);const {candidate,work}=await f.candidate();const target=f.move();const pin=`refs/crewroom/${f.seed.run.id}/${randomUUID()}`;f.git('update-ref',pin,target);
  const observer=join(f.root,'scripted-observer');mkdirSync(observer);writeFileSync(join(observer,'hold'),'');writeFileSync(join(observer,'result.json'),JSON.stringify({error:true}));await f.reopen({targetObserver:observer});
  const pending=f.refresh(candidate,randomUUID(),pin).then(value=>({value}),error=>({error}));await waitFor(join(observer,'called'));
  if(mode==='source'){writeFileSync(join(work.path,'external.txt'),'External work must survive\n');f.gitAt(work.path,'add','external.txt');f.gitAt(work.path,'commit','-m','external work');}else{alter(f,'runs',f.seed.run.id,r=>r.status=mode==='attention'?'attention_required':'running');}
  const retained=f.gitAt(work.path,'rev-parse','HEAD');writeFileSync(join(observer,'release'),'');assert.ok('error'in await pending);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),retained);assert.equal((await f.record()).refreshes.at(-1).state,'rebasing');assert.equal((await f.record()).refreshes.at(-1).rebaseRecovery,'rebase_stopped');
 }
});

test('actual controller SIGKILL during native rebase retains unknown outcome after exact child stop',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move();const latch=gitLatch(f.root,'rebase');try{await f.reopen();latch.arm();const pending=f.refresh(candidate).catch(e=>e);await latch.blocked();f.c.proc.kill('SIGKILL');await f.c.exit;await pending;
  const d=f.intents().find(r=>r.spec.args.includes('--onto')).descriptor;assert.equal((await inspectCheckedOwned(d)).state,'live');await stopOwned(d.root);assert.equal((await inspectCheckedOwned(d)).state,'stopped');latch.release();latch.restore();await f.reopen();await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/exit or rollback is unproved/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal((await f.record()).refreshes.at(-1).rebaseRecovery,undefined);
 }finally{latch.release();latch.restore();}
});

test('reasoned retries preserve the earlier operator workspace and cannot bypass unknown preparation',{timeout:90000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Conflict\n');f.move('AGENTS.md','Changed target instructions\n');const held=await f.refresh(candidate);const workspace=held.refresh.resolutionWorkspace;
 await assert.rejects(f.refresh(candidate),/operator reason/);const id=randomUUID();f.git('reset','--hard',candidate.revision.base);
 const decision={requestId:held.refresh.requestId,reason:'Inspected the target rollback and retained workspace.'};const retry=await f.refresh(candidate,id,'refs/heads/main',decision);assert.equal(retry.refresh.state,'completed');assert.equal(retry.refresh.candidateId,candidate.id);assert.equal(retry.work.refreshes[0].supersededBy.requestId,id);assert.equal(retry.work.refreshes[0].resolutionWorkspace.path,workspace.path);assert.ok(existsSync(workspace.path));assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
 alter(f,'repository_work',f.seed.run.id,w=>w.refreshes[0].resolutionWorkspace.state='preparing');await assert.rejects(f.refresh(candidate),/Unverified Git mutation/);await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/unverified Git ownership/);
});

test('stopped rebase recovery adopts only the actual clean effect after controller SIGKILL',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();const target=f.move();const pin=`refs/crewroom/${f.seed.run.id}/${randomUUID()}`;f.git('update-ref',pin,target);
 const observer=join(f.root,'scripted-observer');mkdirSync(observer);writeFileSync(join(observer,'hold'),'');writeFileSync(join(observer,'result.json'),JSON.stringify({head:target}));await f.reopen({targetObserver:observer});
 const key=randomUUID();const pending=f.refresh(candidate,key,pin).catch(e=>e);await waitFor(join(observer,'called'));assert.equal((await f.record()).refreshes.at(-1).rebaseRecovery,'rebase_stopped');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;
 writeFileSync(join(observer,'release'),'');await f.reopen({targetObserver:observer});await f.c.command('reconcile_refresh',{target:f.seed.run.id});const r=(await f.record()).refreshes.at(-1);assert.equal(r.state,'completed');assert.equal(r.toBase,target);const result=await f.c.command('associate_refresh',{target:f.seed.run.id,key});assert.equal(result.refreshedFrom,candidate.id);await f.c.command('reconcile_refresh',{target:f.seed.run.id});assert.deepEqual(await f.c.command('associate_refresh',{target:f.seed.run.id,key}),result);
});

test('actual SIGKILL during conflict add or isolated rebase preserves the restored original and preparation barrier',{timeout:120000},async t=>{
 for(const stage of ['add','isolated-rebase']) {
  const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const latch=gitLatch(f.root,stage==='add'?'add':'--onto',stage==='add'?0:1);
  try{await f.reopen();latch.arm();const pending=f.refresh(candidate).catch(e=>e);await latch.blocked();const held=await f.record();assert.equal(held.refreshes.at(-1).resolutionWorkspace.state,'preparing');assert.equal(held.refreshes.at(-1).rebaseRecovery,'restored');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
   const candidates=f.intents().filter(r=>stage==='add'?r.spec.args.includes('worktree')&&r.spec.args.includes('add')&&r.spec.args.includes(held.refreshes.at(-1).resolutionWorkspace.path):r.spec.args.includes('--onto')&&r.spec.cwd!==work.path);assert.equal(candidates.length,1);const d=candidates[0].descriptor;assert.equal((await inspectCheckedOwned(d)).state,'live');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;
   await stopOwned(d.root);assert.equal((await inspectCheckedOwned(d)).state,'stopped');latch.release();latch.restore();await f.reopen();await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/unverified Git ownership/);await assert.rejects(f.refresh(candidate),/Unverified Git mutation/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
  }finally{latch.release();latch.restore();}
 }
});

test('a lost rollback receipt remains held despite the branch appearing restored',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const db=new DatabaseSync(f.seed.database);db.exec("CREATE TRIGGER fail_restored BEFORE UPDATE ON repository_work WHEN json_extract(NEW.data,'$.refreshes[0].rebaseRecovery')='restored' BEGIN SELECT RAISE(ABORT,'fixture lost restoration'); END;");db.close();await assert.rejects(f.refresh(candidate),/fixture lost restoration/);
 assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal((await f.record()).refreshes.at(-1).rebaseRecovery,'undo_started');const repair=new DatabaseSync(f.seed.database);repair.exec('DROP TRIGGER fail_restored');repair.close();await f.reopen();const before=f.descriptors().length;await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/exit or rollback is unproved/);assert.equal(f.descriptors().length,before);
});

test('recorded stopped conflict recovery verifies exact rebase metadata before aborting',{timeout:90000},async t=>{
 for(const tamper of [false,true]) {
  const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const db=new DatabaseSync(f.seed.database);db.exec("CREATE TRIGGER fail_undo BEFORE UPDATE ON repository_work WHEN json_extract(NEW.data,'$.refreshes[0].rebaseRecovery')='undo_started' BEGIN SELECT RAISE(ABORT,'fixture before undo'); END;");db.close();await assert.rejects(f.refresh(candidate),/fixture before undo/);
  assert.equal((await f.record()).refreshes.at(-1).rebaseRecovery,'rebase_stopped');const repair=new DatabaseSync(f.seed.database);repair.exec('DROP TRIGGER fail_undo');repair.close();await f.reopen();
  if(tamper){const root=f.gitAt(work.path,'rev-parse','--path-format=absolute','--git-path','rebase-merge');writeFileSync(join(root,'orig-head'),'0'.repeat(40)+'\n');const count=f.descriptors().length;await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/admitted source and target/);assert.equal(f.descriptors().length,count);assert.ok(existsSync(root));}
  else{await f.c.command('reconcile_refresh',{target:f.seed.run.id});const result=(await f.record()).refreshes.at(-1);assert.equal(result.state,'interrupted');assert.equal(result.rebaseRecovery,'restored');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal(f.gitAt(work.path,'status','--porcelain'),'');await f.c.command('reconcile_refresh',{target:f.seed.run.id});}
 }
});

test('dirty source and accepted requirements changed during final observation never become a refresh receipt',{timeout:90000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();writeFileSync(join(work.path,'external.txt'),'keep\n');await assert.rejects(f.refresh(candidate),/clean/);assert.equal((await f.record()).refreshes?.length??0,0);rmSync(join(work.path,'external.txt'));
 const target=f.move();const pin=`refs/crewroom/${f.seed.run.id}/${randomUUID()}`;f.git('update-ref',pin,target);const observer=join(f.root,'scripted-observer');mkdirSync(observer);writeFileSync(join(observer,'hold'),'');writeFileSync(join(observer,'result.json'),JSON.stringify({head:target}));await f.reopen({targetObserver:observer});
 const pending=f.refresh(candidate,randomUUID(),pin).then(value=>({value}),error=>({error}));await waitFor(join(observer,'called'));const task=await f.c.command('task',{target:f.seed.task.id});await f.c.command('refine',{target:f.seed.task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Accepted scope changed',criteria:['New accepted criterion']}});const head=f.gitAt(work.path,'rev-parse','HEAD');writeFileSync(join(observer,'release'),'');assert.ok('error'in await pending);assert.equal((await f.record()).refreshes.at(-1).state,'rebasing');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),head);
});

test('actual EFBIG during checked Git capture cannot become a numeric rebase outcome',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();f.move('source.txt','Target conflict\n');
 const python=execFileSync('python3',['-c','import sys; print(sys.executable)'],{encoding:'utf8'}).trim();const helper=join(f.root,'bounded-helper');const realHelper=realpathSync('target/debug/crewroom-process');
 writeFileSync(helper,`#!${python}\nimport os,resource,signal,sys\nresource.setrlimit(resource.RLIMIT_FSIZE,(1024,1024))\nsignal.signal(signal.SIGXFSZ,signal.SIG_IGN)\nos.execv(${JSON.stringify(realHelper)},[${JSON.stringify(realHelper)},*sys.argv[1:]])\n`);chmodSync(helper,0o755);
 const realGit=execFileSync('/bin/sh',['-c','command -v git'],{encoding:'utf8'}).trim();const bin=join(f.root,'noisy-git');mkdirSync(bin);const quote=(s:string)=>`'${s.replaceAll("'",`'\\''`)}'`;
 writeFileSync(join(bin,'git'),`#!/bin/sh\nfor argument in "$@"; do\n if [ "$argument" = --onto ]; then printf '%s' '${'x'.repeat(4096)}'; break; fi\ndone\nexec ${quote(realGit)} "$@"\n`);chmodSync(join(bin,'git'),0o755);
 const previous=process.env.PATH;try{process.env.PATH=`${bin}:${previous}`;await f.reopen({helper});await assert.rejects(f.refresh(candidate));const record=await f.record();assert.equal(record.refreshes.at(-1).state,'rebasing');assert.equal(record.refreshes.at(-1).rebaseRecovery,undefined);const d=f.intents().find(r=>r.spec.args.includes('--onto')).descriptor;assert.equal((await inspectCheckedOwned(d)).state,'stopped');assert.equal(readFileSync(join(d.root,'stdout.log')).length,1024);assert.equal(existsSync(join(d.root,'capture.json')),false);await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/exit or rollback is unproved/);}finally{process.env.PATH=previous;}
});
