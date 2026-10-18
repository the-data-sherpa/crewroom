import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {existsSync,readFileSync,readdirSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {DatabaseSync} from 'node:sqlite';
import {Workbench} from '../../server/workbench';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {inspectCheckedOwned} from './process-adapter';
import {setup} from './publication-support';
import {body} from './publication-workflow-fixture';
function intents(r:Awaited<ReturnType<typeof setup>>){const root=join(r.f.config.authority,'publication',r.f.seed.run.id);return existsSync(root)?readdirSync(root).filter(n=>n.endsWith('.json')).map(n=>JSON.parse(readFileSync(join(root,n),'utf8'))):[];}

test('native publication recovers actual controller death before and after the Git effect',{timeout:120000},async t=>{
 for(const after of [false,true])await t.test(after?'after push':'before push',async t=>{
  const r=await setup(t);const p=await r.prepare();const latch=gitLatch(r.f.root,after?'ls-remote':'push',after?2:0);try{await r.f.reopen();latch.arm();const pending=r.f.flush(),rejected=assert.rejects(pending,/exited/);await latch.blocked();assert.equal((await r.f.record()).push.state,'sending');if(after)assert.equal(r.reference(p.branch),p.revision.head);else assert.throws(()=>r.reference(p.branch));r.f.c.proc.kill('SIGKILL');await r.f.c.exit;await rejected;
  const live=[];for(const i of intents(r))if((await inspectCheckedOwned(i.descriptor)).state==='live')live.push(i);assert.equal(live.length,1);await r.f.reopen();await r.f.flush();assert.equal((await inspectCheckedOwned(live[0].descriptor)).state,'stopped');assert.equal((await r.f.record()).ready,true);assert.equal(r.state.posts,1);assert.equal(intents(r).filter(i=>i.spec.args.includes('push')).length,after?1:2);
  }finally{latch.release();latch.restore();}
 });
});

test('native PR crash windows reconcile provider identity and never replay an unconfirmed POST',{timeout:120000},async t=>{
 for(const effect of [false,true])await t.test(effect?'after PR effect':'before PR effect',async t=>{
  const r=await setup(t);await r.prepare();let entered!:()=>void;const waiting=new Promise<void>(resolve=>entered=resolve);let socket:any;
  r.state.intercept=async(req,res)=>{if(req.method!=='POST')return false;const b=await body(req);socket=res;if(effect){r.state.posts++;r.state.prs.push({id:1,title:b.title,description:b.description,state:'OPEN',updated_on:r.state.date,source:{...b.source,repository:{uuid:r.f.connection.repository.uuid}},destination:{branch:b.destination.branch,commit:{hash:r.reference('main')},repository:{uuid:r.f.connection.repository.uuid}},links:{html:{href:'https://bitbucket.org/fixture/repository/pull-requests/1'}}});}entered();return true;};
  const pending=r.f.flush(),rejected=assert.rejects(pending,/exited/);await waiting;assert.equal((await r.f.record()).pr.state,'sending');r.f.c.proc.kill('SIGKILL');await r.f.c.exit;await rejected;socket.destroy();r.state.intercept=undefined;await r.f.reopen();await r.f.flush();await r.f.flush();assert.equal((await r.f.record()).pr.state,effect?'confirmed':'uncertain');assert.equal(r.state.posts,effect?1:0);
 });
});

test('native SQL failure before a sending intent causes no push or PR effect',{timeout:120000},async t=>{
 for(const kind of ['push','pr'])await t.test(kind,async t=>{
  const r=await setup(t);await r.prepare();const db=new DatabaseSync(r.f.database);db.exec(`CREATE TRIGGER fail_intent BEFORE UPDATE ON publications WHEN json_extract(NEW.data,'$.${kind}.state')='sending' BEGIN SELECT RAISE(ABORT,'fixture sending intent failure'); END;`);db.close();await assert.rejects(r.f.flush(),/fixture sending intent failure/);assert.equal(r.state.posts,0);if(kind==='push')assert.equal(intents(r).some(i=>i.spec.args.includes('push')),false);const fix=new DatabaseSync(r.f.database);fix.exec('DROP TRIGGER fail_intent');fix.close();await r.f.flush();assert.equal((await r.f.record()).ready,true);assert.equal(r.state.posts,1);
 });
});

test('Stop fences a pending second run admitted by a shared flush before the first provider replies',{timeout:120000},async t=>{
 const r=await setup(t),p=await r.prepare();
 // This second intent is deliberately seeded to test admission/epoch fencing.
 // No readiness or provider success is inferred for this queued run.
 const store=new Workbench(r.f.database);let second:any;try{const task=store.createTask({title:'Queued publication fixture',description:'Explicit queued intent fixture',repositoryId:r.f.seed.repository.id});store.refine(task.id,{revision:1,sourceRevision:1,brief:'Pending publication scope',criteria:['Stay fenced']});second=store.queue(task.id,{revision:2});}finally{store.close();}
 await r.f.c.command('save_publication',{input:{...p,id:randomUUID(),runId:second.id}});
 let entered!:()=>void;const waiting=new Promise<void>(resolve=>entered=resolve);let respond!:()=>void;let reads=0;
 r.state.intercept=(_req,res)=>{reads++;if(reads!==1)return false;respond=()=>res.end(JSON.stringify({uuid:r.f.connection.repository.uuid,full_name:'fixture/repository'}));entered();return true;};
 const flush=r.f.flush(),refused=assert.rejects(flush,/stopped/);await waiting;let stopped=false;const stop=r.f.c.command('remote_stop',{target:second.id}).then(()=>stopped=true);await delay(50);assert.equal(stopped,false);respond();await refused;await stop;assert.equal(r.state.posts,1);assert.equal((await r.f.record(second.id)).lastAttemptAt,undefined);assert.equal((await r.f.record()).ready,true);
});

test('a changed required check or corrupted artifact while PR credentials wait refuses mutation',{timeout:120000},async t=>{
 for(const changed of ['policy','artifact'])await t.test(changed,async t=>{
  const r=await setup(t);await r.prepare();r.f.rules({holdAt:3});const pending=r.f.flush();await r.f.entered(3);if(changed==='policy')await r.f.c.command('configure_checks',{target:r.f.seed.run.id,input:[{...r.proof.definition,memoryMiB:512}]});else writeFileSync(join(r.f.config.authority,'artifacts',r.proof.review.artifact.id),'tampered');r.f.release(3);await pending;assert.equal(r.state.posts,0);assert.equal((await r.f.record()).pr.state,'pending');
 });
});
