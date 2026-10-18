import assert from 'node:assert/strict';
import {mkdtempSync,realpathSync,mkdirSync,writeFileSync,readFileSync,rmSync,readdirSync,symlinkSync,linkSync,chmodSync,statSync,existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFileSync} from 'node:child_process';
import {DatabaseSync} from 'node:sqlite';
import {test,type TestContext} from 'node:test';
import {NativeRepository} from './repository-adapter';
import {seedMessages} from './messaging-fixture';
import {Workbench} from '../../server/workbench';
import {RepositoryWork} from '../../server/repository-work';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {setTimeout as delay} from 'node:timers/promises';
import {ContainerCommand} from './container-adapter';
import {randomUUID} from 'node:crypto';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
import {checkImage,dockerSocket} from './docker-engine';
function fixture(t:TestContext){
  const root=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-repository-')));const seed=seedMessages(join(root,'operator'));
  const git=(...args:string[])=>execFileSync('git',['-C',seed.root,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid',...args],{encoding:'utf8'}).trimEnd();
  writeFileSync(join(seed.root,'answer.txt'),'original\n');writeFileSync(join(seed.root,'AGENTS.md'),'Keep changes scoped.\n');writeFileSync(join(seed.root,'.gitignore'),'secret.txt\n');git('add','answer.txt','AGENTS.md','.gitignore');git('commit','-m','source');
  const config={database:seed.database,root:join(root,'owned'),authority:join(root,'data')};let c=new NativeRepository(config);
  t.after(async()=>{await c.close();const jobs=join(config.root,'.git-operations');if(existsSync(jobs))for(const run of readdirSync(jobs))for(const name of readdirSync(join(jobs,run)).filter(v=>v.endsWith('.json'))){const descriptor=JSON.parse(readFileSync(join(jobs,run,name),'utf8')).descriptor;const state=await inspectCheckedOwned(descriptor);if(state.state!=='stopped')await stopOwned(descriptor.root);assert.equal(state.state,'stopped','Repository fixture left unresolved Git ownership');}rmSync(root,{recursive:true,force:true});});
  return {root,seed,git,config,get c(){return c;},async reopen(){await c.close();c=new NativeRepository(config);await c.ready;},prepare:()=>c.command('prepare',{target:seed.run.id,input:{mode:'new'}}),status:()=>c.command('status',{target:seed.run.id})};
}
test('native admission preserves operator changes, pins exact ownership and matches retained revision manifests',{timeout:60000},async t=>{
  const f=fixture(t);writeFileSync(join(f.seed.root,'answer.txt'),'operator edit\n');const work=await f.prepare();assert.equal(work.base,f.git('rev-parse','main'));assert.equal(work.path,join(f.root,'owned',f.seed.run.id));assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'original\n');assert.equal(readFileSync(join(f.seed.root,'answer.txt'),'utf8'),'operator edit\n');assert.equal(work.instructions[0].body,'Keep changes scoped.\n');
  const status=await f.status();assert.deepEqual(status.drift,{base:false,head:false,worktree:false,requirements:false,instructions:false});assert.deepEqual(await f.prepare(),work);await f.reopen();assert.deepEqual(await f.prepare(),work);assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,2);
  const nativeFiles=await f.c.command('list',{target:f.seed.run.id});await f.c.close();const store=new Workbench(f.seed.database);try{const retained=new RepositoryWork(store,f.config.root);assert.deepEqual((await retained.status(f.seed.run.id)).revision,status.revision);assert.deepEqual(await retained.listFiles(f.seed.run.id),nativeFiles);}finally{store.close();}
});
test('native writer edits, exact patches and pinned reads preserve scope and durable replay',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();const pinned=(await f.status()).revision;await f.c.command('activate',{target:f.seed.run.id});await f.c.command('claim',{target:f.seed.recipient.attempt.id});
  const read=await f.c.command('read',{target:f.seed.run.id,path:'answer.txt'});const input={path:'answer.txt',expectedHash:read.sha256,content:'changed\n'};
  const result=await f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'edit',input});assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'changed\n');assert.deepEqual(await f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'edit',input}),result);
  await assert.rejects(f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'edit',input:{...input,content:'other'}}),/different content/);
  const patch={path:'answer.txt',expectedHash:result.sha256,replacements:[{oldText:'changed',newText:'patched'}]};const patched=await f.c.command('patch',{target:f.seed.recipient.attempt.id,key:'patch',input:patch});assert.deepEqual(await f.c.command('patch',{target:f.seed.recipient.attempt.id,key:'patch',input:patch}),patched);
  assert.equal((await f.c.command('read_revision',{target:f.seed.run.id,revision:pinned,path:'answer.txt'})).content,'original\n');assert.equal((await f.c.command('search',{target:f.seed.run.id,input:{query:'patched'}})).matches[0].path,'answer.txt');assert.equal((await f.c.command('search',{target:f.seed.run.id,revision:pinned,input:{query:'original'}})).matches[0].path,'answer.txt');
  await assert.rejects(f.c.command('edit',{target:f.seed.sender.attempt.id,key:'not-writer',input}),/current writer/);
  for(const path of ['../operator/answer.txt','.git/config','secret.txt'])await assert.rejects(f.c.command('edit',{target:f.seed.recipient.attempt.id,key:path,input:{path,expectedHash:null,content:'refused'}}));
  assert.equal(readFileSync(join(f.seed.root,'answer.txt'),'utf8'),'original\n');
});

test('native admission disables included content filters and hooks; redirected metadata and source parents are refused',{timeout:60000},async t=>{
  const f=fixture(t);const marker=join(f.root,'must-not-exist');mkdirSync(join(f.seed.root,'nested'));writeFileSync(join(f.seed.root,'nested','source.txt'),'inside\n');writeFileSync(join(f.seed.root,'.gitattributes'),'*.txt filter=trip\n');f.git('add','.gitattributes','nested/source.txt');f.git('commit','-m','scope');
  writeFileSync(join(f.seed.root,'.git','hooks','post-checkout'),`#!/bin/sh\ntouch '${marker}'\n`,{mode:0o755});const include=join(f.root,'included-config');writeFileSync(include,`[filter "trip"]\n smudge = touch ${marker}\n clean = touch ${marker}\n required = true\n`);f.git('config','include.path',include);
  const work=await f.prepare();assert.throws(()=>readFileSync(marker),/ENOENT/);const metadata=readFileSync(join(work.path,'.git'),'utf8');writeFileSync(join(work.path,'.git'),`gitdir: ${join(f.seed.root,'.git')}\n`);await assert.rejects(f.status(),/metadata changed/);writeFileSync(join(work.path,'.git'),metadata);
  rmSync(join(work.path,'nested'),{recursive:true});symlinkSync(f.seed.root,join(work.path,'nested'),'dir');await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'nested/source.txt'}));await assert.rejects(f.c.command('search',{target:f.seed.run.id,input:{query:'original'}}),/symbolic link/);assert.throws(()=>readFileSync(marker),/ENOENT/);
});

test('a lost edit receipt retains pending intent and stopped recovery adopts the exact file effect',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();await f.c.command('activate',{target:f.seed.run.id});await f.c.command('claim',{target:f.seed.recipient.attempt.id});const read=await f.c.command('read',{target:f.seed.run.id,path:'answer.txt'});
  const db=new DatabaseSync(f.seed.database);db.exec("CREATE TRIGGER lose_edit_receipt BEFORE UPDATE ON coding WHEN json_extract(NEW.data,'$.edits[0].state') = 'applied' BEGIN SELECT RAISE(ABORT, 'fixture lost edit receipt'); END;");db.close();
  await assert.rejects(f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'lost',input:{path:'answer.txt',expectedHash:read.sha256,content:'durable edit\n'}}),/fixture lost edit receipt/);assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'durable edit\n');assert.equal((await f.c.command('record',{target:f.seed.run.id})).edits[0].state,'pending');
  f.c.proc.kill('SIGKILL');await f.c.exit;await f.reopen();await assert.rejects(f.c.command('reconcile_edits',{target:f.seed.run.id}),/stopped writer/);const repair=new DatabaseSync(f.seed.database);repair.exec('DROP TRIGGER lose_edit_receipt');repair.close();
  await f.c.command('pause',{target:f.seed.run.id});for(const entry of [f.seed.sender,f.seed.recipient])await f.c.command('ownership',{target:entry.attempt.id});await f.c.command('finish_pause',{target:f.seed.run.id});await f.c.command('reconcile_edits',{target:f.seed.run.id});assert.equal((await f.c.command('record',{target:f.seed.run.id})).edits[0].state,'applied');await f.c.command('release',{target:f.seed.recipient.attempt.id});assert.equal((await f.status()).writer,undefined);assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'durable edit\n');
});

test('concurrent native admission creates one worktree and retains stopped operation identities',{timeout:60000},async t=>{
  const f=fixture(t);const results=await Promise.all(Array.from({length:8},()=>f.prepare()));assert.ok(results.every(v=>v.path===results[0].path));assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,2);
  const jobs=join(f.config.root,'.git-operations',f.seed.run.id);const records=readdirSync(jobs).filter(v=>v.endsWith('.json')).map(v=>JSON.parse(readFileSync(join(jobs,v),'utf8')));assert.equal(records.length,2);
  for(const record of records){const state=JSON.parse(execFileSync('target/debug/crewroom-process',['inspect-checked'],{input:JSON.stringify(record.descriptor),encoding:'utf8'}));assert.equal(state.state,'stopped');assert.ok(readFileSync(join(record.descriptor.root,'capture.json'),'utf8'));}
  await f.reopen();assert.equal((await f.prepare()).path,results[0].path);assert.equal(readdirSync(jobs).filter(v=>v.endsWith('.json')).length,2);
});

test('a killed controller cannot adopt repository work while its checked Git child is live',{timeout:60000},async t=>{
  const f=fixture(t);const latch=gitLatch(f.root,'worktree',2); // initial branch scan, then pre-dispatch branch scan, then add
  try {
    await f.reopen();latch.arm();const operation=f.prepare().then(value=>({value}),error=>({error}));await latch.blocked();
    const jobs=join(f.config.root,'.git-operations',f.seed.run.id);const records=readdirSync(jobs).filter(v=>v.endsWith('.json')).map(v=>JSON.parse(readFileSync(join(jobs,v),'utf8')));assert.equal(records.length,2);
    const active=records.find(r=>r.spec.args.includes('add'));assert.ok(active);assert.equal(JSON.parse(execFileSync('target/debug/crewroom-process',['inspect-checked'],{input:JSON.stringify(active.descriptor),encoding:'utf8'})).state,'live');
    f.c.proc.kill('SIGKILL');await f.c.exit;assert.ok('error' in await operation);await f.reopen();await assert.rejects(f.prepare(),/live or uncertain/);
    latch.release();await latch.finished();const deadline=Date.now()+10000;for(;;){const state=JSON.parse(execFileSync('target/debug/crewroom-process',['inspect-checked'],{input:JSON.stringify(active.descriptor),encoding:'utf8'}));if(state.state==='stopped')break;assert.ok(Date.now()<deadline,JSON.stringify(state));await delay(10);}
    const adopted=await f.prepare();assert.equal(adopted.state,'ready');assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,2);assert.equal(readdirSync(jobs).filter(v=>v.endsWith('.json')).length,2);
  } finally {latch.release();latch.restore();}
});

test('explicit branch selection requires exact head and rejects checkout and unowned destinations',{timeout:60000},async t=>{
  const f=fixture(t);const head=f.git('rev-parse','HEAD');const prepare=(branch:string,expectedHead=head)=>f.c.command('prepare',{target:f.seed.run.id,input:{mode:'continue',branch,expectedHead}});
  await assert.rejects(prepare('main'),/checked out/);await assert.rejects(prepare('--detach'),/option/);f.git('branch','existing');await assert.rejects(prepare('existing','a'.repeat(40)),/head changed/);
  const path=join(f.config.root,f.seed.run.id);await f.c.ready;mkdirSync(path);writeFileSync(join(path,'foreign'),'preserve');await assert.rejects(prepare('existing'),/without owned state/);assert.equal(readFileSync(join(path,'foreign'),'utf8'),'preserve');rmSync(path,{recursive:true});
  const work=await prepare('existing');assert.equal(work.branch,'existing');assert.equal(work.startHead,head);await assert.rejects(f.prepare(),/different repository selection/);
});

test('requirements changed while Git is blocked remain responsive and refuse the final writer grant',{timeout:60000},async t=>{
  const f=fixture(t);await f.prepare();await f.c.command('activate',{target:f.seed.run.id});const latch=gitLatch(f.root,'merge-base');
  try {await f.reopen();latch.arm();const claim=f.c.command('claim',{target:f.seed.recipient.attempt.id}).then(value=>({value}),error=>({error}));await latch.blocked();
    const start=Date.now();const task=await f.c.command('task',{target:f.seed.task.id});await f.c.command('refine',{target:task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Changed while Git waits',criteria:task.criteria}});assert.ok(Date.now()-start<1000,'Store operations blocked behind Git');
    latch.release();await latch.finished();const result=await claim;assert.ok('error' in result);assert.match(String(result.error),/requirements changed|Reconcile drift/);assert.equal((await f.status()).writer,undefined);assert.equal((await f.status()).drift.requirements,true);
  }finally{latch.release();latch.restore();}
});

test('coordinator claims replay once and replacement restores the interrupted writer grant',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();await f.c.command('activate',{target:f.seed.run.id});const input={assignee:'Specialist',instruction:'\uFEFF Continue the scoped change. \n'};const claim=await f.c.command('claim_implementation',{target:f.seed.sender.attempt.id,key:'grant',input});assert.equal(claim.instruction,'Continue the scoped change.');assert.equal(claim.automatic,false);
  assert.deepEqual(await f.c.command('claim_implementation',{target:f.seed.sender.attempt.id,key:'grant',input}),claim);await assert.rejects(f.c.command('claim_implementation',{target:f.seed.recipient.attempt.id,key:'wrong',input}),/active coordinator/);await assert.rejects(f.c.command('claim_implementation',{target:f.seed.sender.attempt.id,key:'grant',input:{...input,instruction:'Different'}}),/different content/);
  const read=await f.c.command('read',{target:f.seed.run.id,path:'answer.txt'});await f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'unfinished',input:{path:'answer.txt',expectedHash:read.sha256,content:'retained unfinished change\n'}});await assert.rejects(f.c.command('release',{target:f.seed.recipient.attempt.id}),/stopped ownership/);
  await f.c.command('pause',{target:f.seed.run.id});for(const entry of [f.seed.sender,f.seed.recipient])await f.c.command('ownership',{target:entry.attempt.id});await f.c.command('finish_pause',{target:f.seed.run.id});await f.c.command('release',{target:f.seed.recipient.attempt.id});
  const replacement=await f.c.command('replace',{target:f.seed.recipient.assignment.id,input:{automatic:true}});await f.c.command('replace',{target:f.seed.sender.assignment.id,input:{automatic:false}});await f.c.command('continue',{target:f.seed.run.id});await f.c.command('activate',{target:f.seed.run.id});
  const restored=await f.c.command('restore_writer',{target:f.seed.run.id});assert.equal(restored.restoredFrom,f.seed.recipient.attempt.id);assert.equal(restored.writerAttemptId,replacement.id);assert.equal(restored.automatic,false);const runtime=await f.c.command('runtime',{target:f.seed.run.id});assert.equal(runtime.assignments.find((a:any)=>a.id===f.seed.recipient.assignment.id).automaticCycles,1);assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'retained unfinished change\n');
});

test('scoped edits reject links, special files, stale hashes and ambiguous overlapping patches',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();await f.c.command('activate',{target:f.seed.run.id});await f.c.command('claim',{target:f.seed.recipient.attempt.id});
  const patch=(key:string,oldText:string)=>f.c.command('patch',{target:f.seed.recipient.attempt.id,key,input:{path:'answer.txt',expectedHash:read.sha256,replacements:[{oldText,newText:'new'}]}});
  let read=await f.c.command('read',{target:f.seed.run.id,path:'answer.txt'});await assert.rejects(patch('absent','not present'),/none/);
  const edited=await f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'overlap',input:{path:'answer.txt',expectedHash:read.sha256,content:'aaaa'}});read={...read,sha256:edited.sha256};await assert.rejects(patch('ambiguous','aaa'),/more than one/);
  chmodSync(join(work.path,'answer.txt'),0o755);read=await f.c.command('read',{target:f.seed.run.id,path:'answer.txt'});await f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'executable',input:{path:'answer.txt',expectedHash:read.sha256,content:'executable\n'}});assert.equal(statSync(join(work.path,'answer.txt')).mode&0o777,0o755);
  symlinkSync(join(f.seed.root,'answer.txt'),join(work.path,'linked'));await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'linked'}));rmSync(join(work.path,'linked'));
  linkSync(join(f.seed.root,'answer.txt'),join(work.path,'hardlinked'));await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'hardlinked'}));rmSync(join(work.path,'hardlinked'));
  execFileSync('mkfifo',[join(work.path,'fifo')]);await assert.rejects(f.c.command('read',{target:f.seed.run.id,path:'fifo'}));rmSync(join(work.path,'fifo'));
  assert.equal(readFileSync(join(f.seed.root,'answer.txt'),'utf8'),'original\n');
});

test('native ad-hoc preparation runs an actual read-only Docker command without creating check evidence',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();await f.c.command('activate',{target:f.seed.run.id});const image=checkImage();
  await assert.rejects(f.c.command('shell',{target:f.seed.recipient.attempt.id,input:{command:'true'}}),/configured, contained check environment/);
  await f.c.command('configure_checks',{target:f.seed.run.id,input:[{id:'required',required:true,image,memoryMiB:256,tmpExecutable:false}]});
  const input={command:'cat answer.txt; if echo forbidden > answer.txt; then exit 9; fi; test "$(cat answer.txt)" = original',timeoutMs:10000};
  await assert.rejects(f.c.command('shell',{target:f.seed.sender.attempt.id,input}),/delegates commands/);for(const timeoutMs of [0,999,600001,NaN])await assert.rejects(f.c.command('shell',{target:f.seed.recipient.attempt.id,input:{...input,timeoutMs}}));
  const prepared=await f.c.command('shell',{target:f.seed.recipient.attempt.id,input});assert.equal(prepared.spec.readOnlyWorkspace,true);assert.equal(prepared.spec.workspace,work.path);
  const job=new ContainerCommand(join(f.root,'adhoc'),{socket:dockerSocket()});try {await job.start(prepared.spec);await job.wait();const receipt=await job.receipt();assert.equal(receipt.started,true);assert.equal(receipt.completed,true);assert.equal(receipt.exitCode,0);assert.match(receipt.output,/original/);assert.match(receipt.output,/Read-only file system|read-only/i);}finally{await job.stop();await job.receipt();await job.remove();}assert.equal((await job.inspect()).state,'stopped');
  assert.deepEqual((await f.c.command('record',{target:f.seed.run.id})).checks,[]);assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'original\n');
});

test('pause recorded during an admitted Git mutation prevents the following worktree dispatch',{timeout:60000},async t=>{
  const f=fixture(t);const latch=gitLatch(f.root,'update-ref');try{await f.reopen();latch.arm();const prepare=f.prepare().then(value=>({value}),error=>({error}));await latch.blocked();await f.c.command('pause',{target:f.seed.run.id});latch.release();await latch.finished();const result=await prepare;assert.ok('error' in result);assert.match(String(result.error),/preparation changed/);assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,1);assert.equal(readdirSync(join(f.config.root,'.git-operations',f.seed.run.id)).filter(v=>v.endsWith('.json')).length,1);}finally{latch.release();latch.restore();}
});

test('post-effect admission receipt loss reopens the same worktree with exact reserved ownership',{timeout:60000},async t=>{
  const f=fixture(t);await f.c.ready;const db=new DatabaseSync(f.seed.database);db.exec("CREATE TRIGGER lose_admission BEFORE UPDATE ON repository_work WHEN json_extract(NEW.data,'$.state') = 'ready' BEGIN SELECT RAISE(ABORT, 'fixture lost admission'); END;");db.close();await assert.rejects(f.prepare(),/fixture lost admission/);assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,2);
  f.c.proc.kill('SIGKILL');await f.c.exit;const repair=new DatabaseSync(f.seed.database);repair.exec('DROP TRIGGER lose_admission');repair.close();await f.reopen();const work=await f.prepare();assert.equal(work.state,'ready');assert.equal(readFileSync(join(work.path,'answer.txt'),'utf8'),'original\n');assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,2);
});

test('dirty manifests and bounded literal, regex and pinned searches match retained source behavior',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();const pinned=(await f.status()).revision;
  for(const name of ['😀.txt','\uE000.txt','space name.txt'])writeFileSync(join(work.path,name),'Match[0]\n');rmSync(join(work.path,'answer.txt'));symlinkSync('missing-relative-target',join(work.path,'symbolic'));writeFileSync(join(work.path,'many.txt'),'many '+ '😀'.repeat(200)+'\n'+Array.from({length:110},()=> 'many\n').join(''));
  const files=await f.c.command('list',{target:f.seed.run.id});const status=await f.status();assert.equal(status.drift.worktree,true);assert.ok(files.some((file:any)=>file.path==='answer.txt'&&file.hash==='deleted'));
  const queries=[{query:'Match[0]'},{query:'match',ignoreCase:true},{query:'Match.0.',regex:true},{query:'Match',path:'space name.txt'},{query:'many'}];const results=[];for(const input of queries)results.push(await f.c.command('search',{target:f.seed.run.id,input}));assert.equal(results[4].matches.length,100);assert.equal(results[4].truncated,true);assert.ok(results[4].matches[0].text.length<=300);assert.ok(!/[\uD800-\uDBFF]$/.test(results[4].matches[0].text));
  await assert.rejects(f.c.command('search',{target:f.seed.run.id,input:{query:'x',path:':(glob)**'}}),/plain repository paths/);await assert.rejects(f.c.command('read_revision',{target:f.seed.run.id,revision:{...pinned,tree:'a'.repeat(40)},path:'answer.txt'}),/tree identity/);
  await f.c.close();const store=new Workbench(f.seed.database);try{const retained=new RepositoryWork(store,f.config.root);assert.deepEqual(await retained.listFiles(f.seed.run.id),files);assert.deepEqual((await retained.status(f.seed.run.id)).revision,status.revision);for(let i=0;i<queries.length-1;i++)assert.deepEqual(JSON.parse(JSON.stringify(await retained.search(f.seed.run.id,queries[i]))),results[i]);}finally{store.close();}
});

test('native research reads remain pinned after operator target movement and create no coding evidence',{timeout:60000},async t=>{
  const f=fixture(t);const work=await f.prepare();const source=await f.c.command('pin_research',{input:work.repository});const seeded=await f.c.command('seed_research',{input:{title:'Inspect repository',description:'',repositoryId:work.repository.id}});assert.equal(seeded.run.researchSource.commit,source.commit);
  writeFileSync(join(f.seed.root,'answer.txt'),'later source\n');f.git('add','answer.txt');f.git('commit','-m','later');
  const read=await f.c.command('read_research',{target:seeded.run.id,path:'answer.txt'});assert.equal(read.content,'original\n');assert.equal(read.commit,source.commit);const search=await f.c.command('search_research',{target:seeded.run.id,input:{query:'original'}});assert.equal(search.revision,source.commit);assert.equal(search.matches[0].path,'answer.txt');assert.equal(await f.c.command('record',{target:seeded.run.id}),null);
  await assert.rejects(f.c.command('prepare',{target:seeded.run.id,input:{mode:'new'}}),/Refinement runs/);assert.equal(readFileSync(join(f.seed.root,'answer.txt'),'utf8'),'later source\n');
});

test('native repair policy requires failed current validation and charges at most two automatic repairs',{timeout:60000},async t=>{
  const f=fixture(t);await f.prepare();const revision=(await f.status()).revision;await f.c.command('activate',{target:f.seed.run.id});
  // Candidate/check/review records below are policy fixtures, not executed evidence.
  const candidate={id:'policy-candidate',revision};await f.c.command('policy_fixture',{target:f.seed.run.id,input:{candidates:[candidate],requiredChecks:[{id:'required',required:true}]}});
  const input={assignee:'Specialist',instruction:'Repair failed validation.'};let coordinator=f.seed.sender.attempt.id;let writer=f.seed.recipient.attempt.id;const claim=(key:string)=>f.c.command('claim_implementation',{target:coordinator,key,input});
  await assert.rejects(claim('no-failure'),/Complete validation/);
  await f.c.command('policy_fixture',{target:f.seed.run.id,input:{reviews:[{candidateId:candidate.id,assignmentId:'reviewer',verdict:'changes_requested'},{candidateId:candidate.id,assignmentId:'reviewer',verdict:'approved'}]}});await assert.rejects(claim('latest-review'),/Complete validation/);
  await f.c.command('policy_fixture',{target:f.seed.run.id,input:{checks:[{candidateId:candidate.id,checkId:'required',started:true,completed:true,sourceIntact:true,exitCode:1,timedOut:false}]}});
  const completions:any[]=[];for(let cycle=1;cycle<=2;cycle++){
    const result=await claim('repair-'+cycle);assert.equal(result.automatic,true);assert.deepEqual(await claim('repair-'+cycle),result);const current=await f.c.command('runtime',{target:f.seed.run.id});assert.equal(current.assignments.find((a:any)=>!a.coordinator).automaticCycles,cycle);
    completions.push({attemptId:writer,state:'completed'});await f.c.command('policy_fixture',{target:f.seed.run.id,input:{completions}});await f.c.command('pause',{target:f.seed.run.id});for(const id of [coordinator,writer])await f.c.command('ownership',{target:id});await f.c.command('finish_pause',{target:f.seed.run.id});await f.c.command('release',{target:writer});
    writer=(await f.c.command('replace',{target:f.seed.recipient.assignment.id,input:{automatic:false}})).id;coordinator=(await f.c.command('replace',{target:f.seed.sender.assignment.id,input:{automatic:false}})).id;await f.c.command('continue',{target:f.seed.run.id});await f.c.command('activate',{target:f.seed.run.id});
  }
  await assert.rejects(claim('third'),/two automatic repair/);assert.equal((await f.status()).writer,undefined);
});

test('a configuration revision change during Git preflight cannot reserve stale ownership',{timeout:60000},async t=>{
  const f=fixture(t);const latch=gitLatch(f.root,'worktree');try{await f.reopen();latch.arm();const preparing=f.prepare().then(value=>({value}),error=>({error}));await latch.blocked();const db=new DatabaseSync(f.seed.database);db.exec("UPDATE repositories SET data=json_set(data,'$.revision',2)");db.close();latch.release();await latch.finished();const result=await preparing;assert.ok('error' in result);assert.match(String(result.error),/configuration changed/);assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,1);await assert.rejects(f.status(),/not been prepared/);}finally{latch.release();latch.restore();}
});

test('replacement-decoded text hashes support edits while raw byte manifests remain exact',{timeout:60000},async t=>{
  const f=fixture(t);writeFileSync(join(f.seed.root,'malformed.txt'),Buffer.from([0x61,0xff,0x62,0x0a]));f.git('add','malformed.txt');f.git('commit','-m','bytes');const work=await f.prepare();await f.c.command('activate',{target:f.seed.run.id});await f.c.command('claim',{target:f.seed.recipient.attempt.id});const read=await f.c.command('read',{target:f.seed.run.id,path:'malformed.txt'});assert.equal(read.content,'a\uFFFDb\n');const files=await f.c.command('list',{target:f.seed.run.id});assert.notEqual(files.find((v:any)=>v.path==='malformed.txt').hash,read.sha256);
  await f.c.command('edit',{target:f.seed.recipient.attempt.id,key:'replace-text',input:{path:'malformed.txt',expectedHash:read.sha256,content:'replacement\n'}});assert.equal(readFileSync(join(work.path,'malformed.txt'),'utf8'),'replacement\n');assert.deepEqual(readFileSync(join(f.seed.root,'malformed.txt')),Buffer.from([0x61,0xff,0x62,0x0a]));
});

test('provider merge records require actual ancestry in the configured target before admission',{timeout:60000},async t=>{
  const f=fixture(t);f.git('checkout','-b','future');f.git('commit','--allow-empty','-m','future merge');const merge=f.git('rev-parse','HEAD');await f.c.close();const store=new Workbench(f.seed.database);
  try {const repositoryId=store.getTask(f.seed.task.id).repositoryId;const task=store.createTask({title:'Upstream fixture',description:'',repositoryId});store.refine(task.id,{revision:1,sourceRevision:1,brief:'Upstream',criteria:['Delivered']});const run=store.queue(task.id,{revision:2});store.setDependencies(f.seed.task.id,{requestId:randomUUID(),dependencyRevision:0,dependsOn:[task.id]});
    // Saved provider observation is a fixture; Git ancestry below is checked by Rust.
    store.savePublication({id:randomUUID(),runId:run.id,attemptId:randomUUID(),candidateId:randomUUID(),repositoryPath:f.seed.root,repository:{uuid:randomUUID(),workspace:'fixture',slug:'repo'},branch:'future',target:'main',revision:{base:merge,head:merge,tree:f.git('rev-parse','HEAD^{tree}'),worktreeHash:'d'.repeat(64),dirty:false,requirementsHash:'e'.repeat(64),instructionsHash:'f'.repeat(64)},title:'Fixture',description:'Fixture',policy:{statusKeys:[],pipelineSteps:[],optionalSteps:[]},createdAt:new Date().toISOString(),push:{state:'confirmed',expectedHead:null},pr:{state:'confirmed',id:1,providerState:'MERGED'},ready:false,blockers:[],merged:{commit:merge,observedAt:new Date().toISOString()}});
  }finally{store.close();}await f.reopen();assert.match(await f.c.command('dependency_hold',{target:f.seed.run.id}),/verified merge.*update the target branch/);await assert.rejects(f.prepare(),/verified merge/);assert.equal(f.git('worktree','list','--porcelain').split('worktree ').length-1,1);
  f.git('checkout','main');f.git('merge','--ff-only','future');assert.equal(await f.c.command('dependency_hold',{target:f.seed.run.id}),null);assert.equal((await f.prepare()).base,merge);
});
