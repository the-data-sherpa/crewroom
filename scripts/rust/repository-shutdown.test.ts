import assert from 'node:assert/strict';
import {execFileSync,spawn} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {existsSync,linkSync,mkdirSync,readFileSync,rmSync,symlinkSync,unlinkSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {DatabaseSync} from 'node:sqlite';
import {fixture,waitFor} from './target-fixture';
import {ready} from './conflict-promotion-fixture';
import {helper,inspectCheckedOwned} from './process-adapter';
import {gitLatch} from '../../tests/fixtures/git-latch';

const worktreeProcess=(f:ReturnType<typeof fixture>)=>{const saved=f.intents().filter(i=>i.spec.args.includes('worktree')&&i.spec.args.includes('add'));assert.equal(saved.length,1);return saved[0].descriptor;};
const stopped=(result:any)=>{assert.equal(result.stopped,true,JSON.stringify(result));assert.ok(result.operations.every((op:any)=>op.state==='stopped'));};

test('Stop drains a held actual Git mutation after its caller is cancelled and permits a fresh operation',{timeout:45000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);
 try {
  await f.reopen();latch.arm();const requestId=randomUUID();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}},requestId).catch(e=>e);
  await latch.blocked();const d=worktreeProcess(f);assert.ok(d);assert.equal((await inspectCheckedOwned(d)).state,'live');assert.equal(await f.c.command('repository_busy',{target:f.seed.run.id}),true);
  stopped(await f.c.command('repository_stop',{target:randomUUID()}));assert.equal((await inspectCheckedOwned(d)).state,'live','A different run cannot stop this command');
  await f.c.command('cancel_waiter',{requestId});assert.match(String(await pending),/cancelled/);
  stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));assert.equal((await inspectCheckedOwned(d)).state,'stopped');assert.equal(await f.c.command('repository_busy',{target:f.seed.run.id}),false);
  stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));latch.release();const work=await f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}});assert.ok(existsSync(work.path));
 } finally {latch.release();latch.restore();}
});

test('close stops real Git, remains closed, and leaves read-only requests responsive',{timeout:45000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);
 try {
  await f.reopen();latch.arm();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}).catch(e=>e);await latch.blocked();const d=worktreeProcess(f);const admitted=f.descriptors().length;
  stopped(await f.c.command('repository_close'));assert.ok(await pending instanceof Error);assert.equal((await inspectCheckedOwned(d)).state,'stopped');await f.c.command('snapshot');
  await assert.rejects(f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}),/stopping/);assert.equal(f.descriptors().length,admitted);
  await f.c.close();assert.equal(f.c.proc.exitCode,0,f.c.stderr);
 } finally {latch.release();latch.restore();}
});

test('after controller SIGKILL passive reconciliation holds a live Git owner until explicit Stop',{timeout:60000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);
 try {
  await f.reopen();latch.arm();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}).catch(e=>e);await latch.blocked();const d=worktreeProcess(f);f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await f.reopen();
  await assert.rejects(f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}),/live|uncertain|ownership/);assert.equal((await inspectCheckedOwned(d)).state,'live');stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));assert.equal((await inspectCheckedOwned(d)).state,'stopped');latch.release();await f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}});
 } finally {latch.release();latch.restore();}
});

test('corrupt, linked, oversized and special saved records hold cleanup without skipping a known live command',{timeout:60000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);
 try {
  await f.reopen();latch.arm();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}).catch(e=>e);await latch.blocked();const d=worktreeProcess(f);const bad=join(f.config.root,'.git-operations',f.seed.run.id,'damaged.json');writeFileSync(bad,'{');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await f.reopen();
  const result=await f.c.command('repository_stop',{target:f.seed.run.id});assert.equal(result.stopped,false);assert.ok(result.ownershipErrors.length);assert.equal((await inspectCheckedOwned(d)).state,'stopped');await pending;
  const normal=join(f.config.root,'.git-operations',f.seed.run.id,`${d.token}.json`);
  for(const mode of ['symlink','hardlink','oversized','fifo']) {
   rmSync(bad);if(mode==='symlink')symlinkSync(normal,bad);else if(mode==='hardlink')linkSync(normal,bad);else if(mode==='fifo')execFileSync('/usr/bin/mkfifo',[bad]);else writeFileSync(bad,' '.repeat(256*1024+1));
   const held=await f.c.command('repository_stop',{target:f.seed.run.id});assert.equal(held.stopped,false,mode);assert.ok(held.ownershipErrors.length,mode);await assert.rejects(f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}));
  }
  rmSync(bad);stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));
 } finally {latch.release();latch.restore();}
});

test('a damaged SQLite ownership record still permits cleanup of the independently saved live Git command',{timeout:45000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);
 try {
  await f.reopen();latch.arm();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}).catch(e=>e);await latch.blocked();const d=worktreeProcess(f);const db=new DatabaseSync(f.seed.database);const raw=db.prepare('SELECT data FROM repository_work WHERE run_id=?').get(f.seed.run.id)!.data;
  try {db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify({...JSON.parse(String(raw)),refreshes:'corrupt ownership'}),f.seed.run.id);const held=await f.c.command('repository_stop',{target:f.seed.run.id});assert.equal(held.stopped,false);assert.equal((await inspectCheckedOwned(d)).state,'stopped');await pending;}finally{db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(raw!,f.seed.run.id);db.close();}
  stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));
 } finally {latch.release();latch.restore();}
});

test('replaced operation directories retain uncertainty even when empty',{timeout:30000},async t=>{
 const f=fixture(t);await f.c.ready;const path=join(f.config.root,'.git-operations');const elsewhere=join(f.root,'elsewhere');mkdirSync(elsewhere);rmSync(path,{recursive:true});symlinkSync(elsewhere,path);
 try {assert.equal((await f.c.command('repository_close')).stopped,false);}finally{unlinkSync(path);mkdirSync(path);stopped(await f.c.command('repository_close'));}
});

test('an unavailable ownership table cannot skip cleanup of an independently recorded live process',{timeout:45000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);
 try {
  await f.reopen();latch.arm();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}).catch(e=>e);await latch.blocked();const d=worktreeProcess(f);const db=new DatabaseSync(f.seed.database);
  db.exec('ALTER TABLE repository_work RENAME TO hidden_repository_work');
  try {const result=await f.c.command('repository_stop',{target:f.seed.run.id});assert.equal(result.stopped,false);assert.ok(result.ownershipErrors.some((s:string)=>s.includes('repository_work')));assert.equal((await inspectCheckedOwned(d)).state,'stopped');await pending;}finally{db.exec('ALTER TABLE hidden_repository_work RENAME TO repository_work');db.close();}
  stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));
 }finally{latch.release();latch.restore();}
});

test('Stop seals a SQLite-only promotion after controller death and prevents delayed helper execution',{timeout:180000},async t=>{
 const {f,work,candidate,observer,complete}=await ready(t,true);writeFileSync(join(observer,'hold-2'),'');await f.reopen({targetObserver:observer});const pending=complete().catch(e=>e);await waitFor(join(observer,'called-2'),40000);const saved=(await f.record()).refreshes.at(-1).resolutionWorkspace.promotionProcess;assert.equal(existsSync(saved.root),false);
 f.c.proc.kill('SIGKILL');await f.c.exit;await pending;rmSync(join(observer,'hold-2'));await f.reopen({targetObserver:observer});stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));assert.equal((await inspectCheckedOwned(saved)).state,'stopped');
 const delayed=spawn(helper,['checked-exec-report',saved.root,saved.token,saved.specSha256,saved.epoch,saved.controllerRoot],{stdio:'ignore'});const code=await new Promise(resolve=>delayed.once('exit',resolve));assert.notEqual(code,0);assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);await assert.rejects(complete(),/stopped without applying/);
});

test('cancelled Stop waiter continues to fence an unlaunched promotion until its preparation settles',{timeout:180000},async t=>{
 const {f,work,candidate,observer,complete}=await ready(t,true);writeFileSync(join(observer,'hold-2'),'');await f.reopen({targetObserver:observer});const pending=complete().catch(e=>e);await waitFor(join(observer,'called-2'),40000);const p=(await f.record()).refreshes.at(-1).resolutionWorkspace.promotionProcess;
 try {
 const requestId=randomUUID();const closing=f.c.command('repository_stop',{target:f.seed.run.id},requestId).catch(e=>e);await waitFor(join(p.root,'seal.json'));await f.c.command('cancel_waiter',{requestId});assert.match(String(await closing),/cancelled/);assert.equal(await f.c.command('repository_busy',{target:f.seed.run.id}),true);await f.c.command('snapshot');writeFileSync(join(observer,'release-2'),'');assert.ok(await pending instanceof Error);
 const deadline=Date.now()+10000;while(await f.c.command('repository_busy',{target:f.seed.run.id})){assert.ok(Date.now()<deadline);await delay(10);}assert.equal((await inspectCheckedOwned(p)).state,'stopped');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));
 }finally{writeFileSync(join(observer,'release-2'),'');await pending;}
});

test('native Stop recovers an actual checked promotion after controller SIGKILL',{timeout:180000},async t=>{
 const {f,work,candidate,complete}=await ready(t);const latch=gitLatch(f.root,'reset');try {
  await f.reopen();latch.arm();const pending=complete().catch(e=>e);await waitFor(join(f.root,'git-latch','blocked'),40000);const p=(await f.record()).refreshes.at(-1).resolutionWorkspace.promotionProcess;assert.equal((await inspectCheckedOwned(p)).state,'live');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await f.reopen();stopped(await f.c.command('repository_stop',{target:randomUUID()}));assert.equal((await inspectCheckedOwned(p)).state,'live','A different run cannot stop this promotion');stopped(await f.c.command('repository_stop',{target:f.seed.run.id}));assert.equal((await inspectCheckedOwned(p)).state,'stopped');assert.equal(f.gitAt(work.path,'rev-parse','HEAD'),candidate.revision.head);latch.release();await assert.rejects(complete(),/stopped without applying/);
 }finally{latch.release();latch.restore();}
});

test('missing retained command record must not prove shutdown with a surviving process',{timeout:45000},async t=>{
 const f=fixture(t);const latch=gitLatch(f.root,'worktree',2);let path:string|undefined;let saved:Buffer|undefined;
 try {await f.reopen();latch.arm();const pending=f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}).catch(e=>e);await latch.blocked();const d=f.intents().find(i=>i.spec.args.includes('worktree')&&i.spec.args.includes('add')).descriptor;path=join(f.config.root,'.git-operations',f.seed.run.id,`${d.token}.json`);saved=readFileSync(path);f.c.proc.kill('SIGKILL');await f.c.exit;await pending;unlinkSync(path);await f.reopen();const before=f.intents().length;await assert.rejects(f.c.command('prepare',{target:f.seed.run.id,input:{mode:'new'}}),/ownership|record|prior/i);assert.equal(f.intents().length,before);const result=await f.c.command('repository_stop',{target:f.seed.run.id});assert.equal((await inspectCheckedOwned(d)).state,'live');assert.equal(result.stopped,false,'A surviving process without its command record must remain uncertain');}
 finally {if(path&&saved)writeFileSync(path,saved);await f.c.command('repository_stop',{target:f.seed.run.id});latch.release();latch.restore();}
});

test('unknown promotion directories hold run Stop and close until ownership is reconciled',{timeout:20000},async t=>{
 const f=fixture(t);await f.c.ready;const parent=join(f.config.root,'promotion-processes');mkdirSync(parent);const path=join(parent,`${randomUUID()}-${randomUUID()}`);mkdirSync(path);
 try {for(const op of ['repository_stop','repository_close']){const result=await f.c.command(op,{target:f.seed.run.id});assert.equal(result.stopped,false);assert.ok(result.ownershipErrors.some((s:string)=>s.includes('no verified ownership record')));assert.ok(existsSync(path));}}
 finally {rmSync(path,{recursive:true});stopped(await f.c.command('repository_close'));}
});
