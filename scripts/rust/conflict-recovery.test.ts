import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {chmodSync,existsSync,readFileSync,realpathSync,rmSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {fixture,waitFor} from './target-fixture';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
import {gitLatch} from '../../tests/fixtures/git-latch';

test('missing live child outcome stays unknown even with complete retained captures',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();f.move('source.txt','Target conflict\n');
 const python=execFileSync('python3',['-c','import sys; print(sys.executable)'],{encoding:'utf8'}).trim();const helper=join(f.root,'lost-live-outcome');const real=realpathSync('target/debug/crewroom-process');
 // Only this disposable helper loses its ephemeral report pipe. Git capture
 // files are complete; they cannot substitute for a live child outcome.
 writeFileSync(helper,`#!${python}\nimport os,sys\nos.dup2(os.open(os.devnull,os.O_WRONLY),1)\nos.execv(${JSON.stringify(real)},[${JSON.stringify(real)},*sys.argv[1:]])\n`);chmodSync(helper,0o755);
 await f.reopen({helper});await assert.rejects(f.refresh(candidate),/Live Git outcome was not confirmed/);const record=await f.record();assert.equal(record.refreshes.at(-1).state,'rebasing');assert.equal(record.refreshes.at(-1).rebaseRecovery,undefined);
 const d=f.intents().find(r=>r.spec.args.includes('--onto')).descriptor;assert.equal((await inspectCheckedOwned(d)).state,'stopped');assert.ok(existsSync(join(d.root,'capture.json')));const capture=JSON.parse(readFileSync(join(d.root,'capture.json'),'utf8'));assert.equal(capture.stdout.truncated,false);assert.equal(capture.stderr.truncated,false);
 await f.reopen();await assert.rejects(f.c.command('reconcile_refresh',{target:f.seed.run.id}),/exit or rollback is unproved/);
});

test('conflict reads and search use the validated isolated source while the original stays clean',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const result=await f.refresh(candidate);const r=result.refresh;
 assert.equal((await f.c.command('active_conflict',{target:f.seed.run.id})).requestId,r.requestId);const validated=await f.c.command('validate_conflict',{target:f.seed.run.id});assert.equal(validated.state,'conflicted');assert.deepEqual(validated.conflicts,['source.txt']);
 const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});assert.match(read.content,/<<<<<<<|>>>>>>>/);assert.match(read.content,/Target conflict/);assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'implementation\n');
 assert.ok((await f.c.command('list',{target:f.seed.run.id})).some((e:any)=>e.path==='source.txt'));const search=await f.c.command('search',{target:f.seed.run.id,input:{query:'Target conflict'}});assert.equal(search.matches[0].path,'source.txt');assert.equal(f.gitAt(work.path,'status','--porcelain'),'');
 const pinned=await f.c.command('read_revision',{target:f.seed.run.id,revision:candidate.revision,path:'source.txt'});assert.equal(pinned.content,'implementation\n');
});

test('changed instructions or a detached non-rebase conflict workspace become operator holds',{timeout:90000},async t=>{
 for(const change of ['instructions','detached']) {
  const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const path=r.resolutionWorkspace.path;
  if(change==='instructions'){writeFileSync(join(path,'AGENTS.md'),'Unaccepted instructions\n');}else{f.gitAt(path,'rebase','--abort');f.gitAt(path,'checkout','--detach',candidate.revision.head);}
  await assert.rejects(f.c.command('validate_conflict',{target:f.seed.run.id}),/changed/);assert.equal((await f.record()).refreshes.at(-1).resolutionWorkspace.state,'operator_required');await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'source.txt'}),/ownership is unverified/);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
 }
});

test('an externally completed isolated rebase needs explicit Resume before ready recovery',{timeout:60000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const path=r.resolutionWorkspace.path;
 // Deliberate external operator Git effect; no native continuation receipt is
 // fabricated. Recovery still requires confirmed old children and explicit Resume.
 writeFileSync(join(path,'source.txt'),'Resolved source\n');f.gitAt(path,'add','source.txt');f.gitAt(path,'-c','core.editor=true','-c','commit.gpgSign=false','rebase','--continue');
 await assert.rejects(f.c.command('validate_conflict',{target:f.seed.run.id}),/Explicit Resume/);const runtime=await f.c.command('runtime',{target:f.seed.run.id});for(const a of runtime.assignments)await f.c.command('replace',{target:a.id,input:{automatic:false}});await f.c.command('continue',{target:f.seed.run.id});
 const ready=await f.c.command('validate_conflict',{target:f.seed.run.id});assert.equal(ready.state,'ready');assert.equal((await f.record()).refreshes.at(-1).resolutionWorkspace.toHead,f.gitAt(path,'rev-parse','HEAD'));assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal(f.gitAt(work.path,'status','--porcelain'),'');
});

async function resumeConflict(f:ReturnType<typeof fixture>) {
 const runtime=await f.c.command('runtime',{target:f.seed.run.id});
 for(const a of runtime.assignments)await f.c.command('replace',{target:a.id,input:{automatic:false}});
 await f.c.command('continue',{target:f.seed.run.id});await f.c.command('activate',{target:f.seed.run.id});
 const next=await f.c.command('runtime',{target:f.seed.run.id});
 return {coordinator:next.assignments.find((a:any)=>a.coordinator).currentAttemptId,writer:next.assignments.find((a:any)=>!a.coordinator).currentAttemptId};
}

test('scoped writer edits only recorded conflicts, preserves original source, and replays its claim',{timeout:150000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const a=await resumeConflict(f);
 await assert.rejects(f.c.command('claim',{target:a.writer}),/explicitly scoped writer/);
 const input={assignee:'Specialist',instruction:'Resolve the target conflict.'};const claim=await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input});assert.equal(claim.targetRefreshId,r.requestId);assert.equal(claim.automatic,false);
 assert.deepEqual(await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input}),claim);
 await f.c.command('configure_checks',{target:f.seed.run.id,input:[{id:'required',required:true,image:'fixture-image',memoryMiB:256}]});const shell=await f.c.command('shell',{target:a.writer,input:{command:'cat source.txt'}});assert.equal(shell.spec.workspace,r.resolutionWorkspace.path);assert.equal(shell.spec.readOnlyWorkspace,true);
 await assert.rejects(f.c.command('continue_conflict',{target:a.writer,key:'markers',input:{requestId:r.requestId}}),/Resolve every conflict marker/);
 const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});const edit={path:'source.txt',expectedHash:read.sha256,content:'Resolved source\n'};
 const applied=await f.c.command('edit',{target:a.writer,key:'resolution',input:edit});assert.deepEqual(await f.c.command('edit',{target:a.writer,key:'resolution',input:edit}),applied);
 assert.equal(readFileSync(join(r.resolutionWorkspace.path,'source.txt'),'utf8'),'Resolved source\n');assert.equal(readFileSync(join(work.path,'source.txt'),'utf8'),'implementation\n');assert.equal(f.gitAt(work.path,'status','--porcelain'),'');
 await assert.rejects(f.c.command('edit',{target:a.writer,key:'other',input:{path:'unrecorded.txt',expectedHash:null,content:'Outside scope'}}),/Only recorded conflict files/);
 const ready=await f.c.command('continue_conflict',{target:a.writer,key:'advance',input:{requestId:r.requestId}});assert.equal(ready.state,'ready');assert.deepEqual(await f.c.command('continue_conflict',{target:a.writer,key:'advance',input:{requestId:r.requestId}}),ready);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
 await assert.rejects(f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input}),/rebase is ready/);
 await assert.rejects(f.c.command('edit',{target:a.writer,key:'ready-edit',input:{...edit,expectedHash:applied.sha256,content:'Late edit'}}),/rebase is ready/);
 await f.c.command('pause',{target:f.seed.run.id});await assert.rejects(f.c.command('edit',{target:a.writer,key:'paused',input:{...edit,expectedHash:applied.sha256,content:'Late edit'}}),/Only the current writer/);
});

test('interrupted conflict grant restores only the same scoped stopped writer',{timeout:90000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;let a=await resumeConflict(f);
 await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});
 await f.c.command('pause',{target:f.seed.run.id});for(const id of [a.coordinator,a.writer])await f.c.command('ownership',{target:id});await f.c.command('finish_pause',{target:f.seed.run.id});await f.c.command('release',{target:a.writer});const prior=a.writer;a=await resumeConflict(f);
 const restored=await f.c.command('restore_writer',{target:f.seed.run.id});assert.equal(restored.restoredFrom,prior);assert.equal(restored.writerAttemptId,a.writer);assert.equal(restored.targetRefreshId,r.requestId);assert.equal(restored.automatic,false);
 const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});await f.c.command('edit',{target:a.writer,key:'restored',input:{path:'source.txt',expectedHash:read.sha256,content:'Restored resolution\n'}});
});


test('plain separator text is preserved through checked rebase continuation',{timeout:90000},async t=>{
 const f=fixture(t);const {candidate}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const a=await resumeConflict(f);
 await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});
 const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});const content='Documentation\n=======\nLiteral separator\n';await f.c.command('edit',{target:a.writer,key:'separator',input:{path:'source.txt',expectedHash:read.sha256,content}});
 const result=await f.c.command('continue_conflict',{target:a.writer,key:'continue',input:{requestId:r.requestId}});assert.equal(result.state,'ready');assert.equal(readFileSync(join(r.resolutionWorkspace.path,'source.txt'),'utf8'),content);const saved=(await f.record()).refreshes.at(-1).resolutionWorkspace;assert.equal(saved.continuation,undefined);assert.equal(saved.continuationReceipts.length,1);assert.equal(saved.continuationReceipts[0].result.head,result.head);
});

test('actual controller SIGKILL during conflict staging or continuation keeps its durable ownership barrier',{timeout:180000},async t=>{
 for(const stage of ['add','--continue']) {
  const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const a=await resumeConflict(f);
  await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});
  const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});await f.c.command('edit',{target:a.writer,key:'resolve',input:{path:'source.txt',expectedHash:read.sha256,content:'Resolution\n'}});
  const latch=gitLatch(f.root,stage);try {
   await f.reopen();latch.arm();const pending=f.c.command('continue_conflict',{target:a.writer,key:'interrupted',input:{requestId:r.requestId}}).catch(e=>e);await waitFor(join(f.root,'git-latch','blocked'),30000);assert.equal((await f.record()).refreshes.at(-1).resolutionWorkspace.state,'continuing');
   const d=f.intents().filter(i=>i.spec.cwd===r.resolutionWorkspace.path && i.spec.args.includes(stage)).at(-1).descriptor;assert.equal((await inspectCheckedOwned(d)).state,'live');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await stopOwned(d.root);assert.equal((await inspectCheckedOwned(d)).state,'stopped');latch.release();latch.restore();await f.reopen();
   await assert.rejects(f.c.command('continue_conflict',{target:a.writer,key:'interrupted',input:{requestId:r.requestId}}),/ownership is unverified/);await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'source.txt'}),/ownership is unverified/);const w=(await f.record()).refreshes.at(-1).resolutionWorkspace;assert.equal(w.state,'continuing');assert.equal(w.continuation.toolCallId,'interrupted');assert.equal(w.continuationReceipts,undefined);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
  }finally{latch.release();latch.restore();}
 }
});


test('successive Git conflicts retain separate receipts and reject a historical continuation key',{timeout:150000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate('source.txt','implementation\n',work=>{f.gitAt(work.path,'add','source.txt');f.gitAt(work.path,'commit','-m','first implementation');writeFileSync(join(work.path,'source.txt'),'second implementation\n');});f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const a=await resumeConflict(f);
 await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});
 for(let step=1;step<=2;step++) {
  const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});await f.c.command('edit',{target:a.writer,key:`resolution-${step}`,input:{path:'source.txt',expectedHash:read.sha256,content:`Resolution ${step}\n`}});
  const result=await f.c.command('continue_conflict',{target:a.writer,key:`advance-${step}`,input:{requestId:r.requestId}});assert.equal(result.state,step===1?'conflicted':'ready');
 }
 await assert.rejects(f.c.command('continue_conflict',{target:a.writer,key:'advance-1',input:{requestId:r.requestId}}),/historical Git continuation/);const saved=(await f.record()).refreshes.at(-1).resolutionWorkspace;assert.equal(saved.continuationReceipts.length,2);assert.equal(saved.state,'ready');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);
});

test('pause while checked continuation runs keeps its effect unresolved and never issues a ready receipt',{timeout:100000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const a=await resumeConflict(f);
 await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});
 const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});await f.c.command('edit',{target:a.writer,key:'resolve',input:{path:'source.txt',expectedHash:read.sha256,content:'Resolution\n'}});
 const latch=gitLatch(f.root,'--continue');try {
  await f.reopen();latch.arm();const pending=f.c.command('continue_conflict',{target:a.writer,key:'pause-race',input:{requestId:r.requestId}}).then(value=>({value}),error=>({error}));await waitFor(join(f.root,'git-latch','blocked'),30000);await f.c.command('pause',{target:f.seed.run.id});latch.release();assert.ok('error' in await pending);const saved=(await f.record()).refreshes.at(-1).resolutionWorkspace;assert.equal(saved.state,'continuing');assert.equal(saved.continuationReceipts,undefined);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal(f.gitAt(work.path,'status','--porcelain'),'');
 }finally{latch.release();latch.restore();}
});


test('original edits and pinned target movement block isolated reads and writes without discarding either source',{timeout:100000},async t=>{
 const f=fixture(t);const {candidate,work}=await f.candidate();const target=f.move('source.txt','Target conflict\n');const r=(await f.refresh(candidate)).refresh;const a=await resumeConflict(f);
 await f.c.command('claim_implementation',{target:a.coordinator,key:'resolve',input:{assignee:'Specialist',instruction:'Resolve the target conflict.'}});const read=await f.c.command('read',{target:f.seed.run.id,path:'source.txt'});
 writeFileSync(join(work.path,'operator.txt'),'Keep operator work\n');await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'source.txt'}),/changed/);await assert.rejects(f.c.command('edit',{target:a.writer,key:'dirty-original',input:{path:'source.txt',expectedHash:read.sha256,content:'Resolution\n'}}),/changed/);assert.equal(readFileSync(join(work.path,'operator.txt'),'utf8'),'Keep operator work\n');rmSync(join(work.path,'operator.txt'));
 f.move('later.txt','Later target\n');await assert.rejects(f.c.command('edit',{target:a.writer,key:'moved-target',input:{path:'source.txt',expectedHash:read.sha256,content:'Resolution\n'}}),/changed/);assert.equal(readFileSync(join(r.resolutionWorkspace.path,'source.txt'),'utf8'),read.content);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);assert.equal((await f.record()).refreshes.at(-1).toBase,target);
});
