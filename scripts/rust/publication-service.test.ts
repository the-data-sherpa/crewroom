/** Actual Git, Docker checks, artifacts and native receipts; synthetic HTTP/attempt identities. */
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {test} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {DatabaseSync} from 'node:sqlite';
import {gitLatch} from '../../tests/fixtures/git-latch';
import {setup,summary} from './publication-support';

test('native publication acquires actual push and PR receipts, exact hosted readiness, and updates its summary once',{timeout:120000},async t=>{
 const r=await setup(t),p=await r.publish();assert.equal(p.ready,true,JSON.stringify(p));assert.equal(p.push.state,'confirmed');assert.equal(p.pr.state,'confirmed');assert.equal(r.reference(p.branch),r.proof.candidate.revision.head);assert.equal(r.state.posts,1);assert.match(p.description,new RegExp(r.proof.candidate.revision.head));assert.match(p.description,/output SHA-256/);
 await Promise.all(Array.from({length:8},()=>r.f.flush()));assert.equal(r.state.posts,1);assert.equal(r.state.puts,0);
 const revised=summary+' The required check and independent review both passed.';await r.prepare(revised);assert.equal((await r.f.c.command('publication_view',{target:p.runId})).record.ready,false);await r.f.flush();assert.equal(r.state.puts,1);assert.equal((await r.f.record()).ready,true);await r.prepare(revised);await r.f.flush();assert.equal(r.state.puts,1);assert.match(r.state.prs[0].description,/required check and independent review/);
 assert.equal((await r.f.c.command('notices',{target:p.runId})).some((n:any)=>n.payload.kind==='publication' && n.payload.ready===true),true);
});

test('native publication differentiates retryable, blocked and unknown PR outcomes without duplicate creation',{timeout:180000},async t=>{
 for(const [mode,expected] of [[429,'pending'],[403,'blocked'],['lost','uncertain'],['invalid','uncertain']] as const){await t.test(String(mode),async t=>{
  const r=await setup(t);if(typeof mode==='number')r.state.createStatus=mode;else r.state.createReply=mode;await r.prepare();await r.f.flush();let p=await r.f.record();assert.equal(p.push.state,'confirmed');assert.equal(p.pr.state,expected);assert.equal(r.state.posts,1);
  r.state.createStatus=0;r.state.createReply='normal';r.state.hidden=true;await r.f.flush();assert.equal(r.state.posts,mode===429?2:1);if(expected==='uncertain')assert.equal((await r.f.record()).pr.state,'uncertain');r.state.hidden=false;await r.f.flush();p=await r.f.record();assert.equal(p.pr.state,mode===403?'blocked':'confirmed');assert.equal(r.state.posts,mode===429?2:1);
 });}
});

test('native publication holds unknown hosted observations without revoking review and latches confirmed failures',{timeout:120000},async t=>{
 const r=await setup(t);await r.publish();r.state.outage=true;await r.f.flush();let p=await r.f.record();assert.equal(p.ready,false);assert.equal(p.reviewInvalidated,false);r.state.outage=false;r.state.status='FAILED';await r.f.flush();p=await r.f.record();assert.equal(p.reviewInvalidated,true);r.state.status='SUCCESSFUL';r.state.outage=true;await r.f.flush();assert.equal((await r.f.record()).reviewInvalidated,true);r.state.outage=false;await r.f.flush();assert.equal((await r.f.record()).ready,true);assert.equal((await r.f.record()).reviewInvalidated,false);
 r.state.build=2;await r.f.flush();const observed=(await r.f.record()).observation;r.state.build=1;await r.f.flush();p=await r.f.record();assert.deepEqual(p.observation,observed);assert.match(p.error,/older/);r.state.build=2;r.state.date='2026-10-02T00:00:00Z';await r.f.flush();assert.deepEqual((await r.f.record()).observation,observed);
});

test('native merge receipt completes delivery despite local artifact and hosted outages',{timeout:120000},async t=>{
 const r=await setup(t);const p=await r.publish();const path=join(r.f.config.authority,'artifacts',r.proof.review.artifact.id);writeFileSync(path,'corrupt local artifact');r.state.outage=true;r.state.prs[0].state='MERGED';r.state.prs[0].merge_commit={hash:p.revision.head};await r.f.flush();const result=await r.f.record();assert.equal(result.merged.commit,p.revision.head);assert.equal(result.pr.providerState,'MERGED');assert.equal(result.ready,false);assert.deepEqual(result.blockers,[]);assert.equal((await r.f.c.command('publication_view',{target:p.runId})).reason.includes('Delivery is complete'),true);await assert.rejects(r.prepare(),/merged/);const before=r.state.requests.length;await r.f.flush();assert.equal(r.state.requests.length,before);
});

test('native publication preparation refuses stale authority after Git and validates strict summary boundaries',{timeout:120000},async t=>{
 const r=await setup(t);for(const input of [{candidateId:'bad',summary},{candidateId:r.proof.candidate.id,summary:'short'},{candidateId:r.proof.candidate.id,summary,extra:1},{candidateId:r.proof.candidate.id,summary:'😀'.repeat(20001)}])await assert.rejects(r.f.prepare(input,r.proof.agents.coordinator.id),/Invalid|fields|requires/);
 await assert.rejects(r.f.prepare({candidateId:r.proof.candidate.id,summary},r.proof.agents.reviewer.id),/coordinator/);
 const latch=gitLatch(r.f.root,'merge-base');try{await r.f.reopen();latch.arm();const pending=r.prepare(),rejected=assert.rejects(pending,/coordinator|control changed/);await latch.blocked();await r.f.c.command('desired',{target:r.f.seed.run.id,input:'paused'});latch.release();await rejected;assert.equal(await r.f.record(),null);assert.equal(r.state.posts,0);}finally{latch.release();latch.restore();}
});

test('native PR mutation revalidates after held credentials; cancellation leaves a pending honest receipt',{timeout:120000},async t=>{
 const r=await setup(t);await r.prepare();r.f.rules({holdAt:3});const pending=r.f.flush();await r.f.entered(3);await r.f.c.command('desired',{target:r.f.seed.run.id,input:'cancelled'});r.f.release(3);await pending;const p=await r.f.record();assert.equal(p.push.state,'confirmed');assert.equal(p.pr.state,'pending');assert.equal(r.state.posts,0);
});

test('native publication close drains a held response and retains the effect of an already sent PR request',{timeout:120000},async t=>{
 const r=await setup(t);await r.prepare();let entered!:()=>void;const waiting=new Promise<void>(resolve=>entered=resolve);let respond!:()=>void;
 r.state.intercept=async(req,res)=>{if(req.method!=='POST')return false;const chunks:Buffer[]=[];for await(const chunk of req)chunks.push(chunk);const b=JSON.parse(Buffer.concat(chunks).toString());const p={id:1,title:b.title,description:b.description,state:'OPEN',updated_on:r.state.date,source:{...b.source,repository:{uuid:r.f.connection.repository.uuid}},destination:{branch:b.destination.branch,commit:{hash:r.reference('main')},repository:{uuid:r.f.connection.repository.uuid}},links:{html:{href:'https://bitbucket.org/fixture/repository/pull-requests/1'}}};r.state.prs.push(p);r.state.posts++;respond=()=>res.end(JSON.stringify(p));entered();return true;};
 const id=randomUUID();const pending=r.f.flush(id);const cancelled=assert.rejects(pending,/cancelled/);await waiting;await r.f.c.command('cancel_waiter',{requestId:id});await cancelled;let done=false;const closing=r.f.c.command('publication_close').then(()=>done=true);await delay(50);assert.equal(done,false);assert.equal((await r.f.c.command('run',{target:r.f.seed.run.id})).status,'running');respond();await closing;assert.equal((await r.f.record()).pr.state,'confirmed');assert.equal(r.state.posts,1);await r.f.c.command('publication_close');await assert.rejects(r.prepare(),/stopping/);
});

test('native publication SQL failure after PR effect preserves sending for exact readback on restart',{timeout:120000},async t=>{
 const r=await setup(t);await r.prepare();const db=new DatabaseSync(r.f.database);db.exec("CREATE TRIGGER fail_publication_confirm BEFORE UPDATE ON publications WHEN json_extract(NEW.data,'$.pr.state')='confirmed' BEGIN SELECT RAISE(ABORT,'fixture publication receipt failure'); END;");db.close();await assert.rejects(r.f.flush(),/fixture publication receipt failure/);assert.equal(r.state.posts,1);assert.equal((await r.f.record()).pr.state,'sending');const fix=new DatabaseSync(r.f.database);fix.exec('DROP TRIGGER fail_publication_confirm');fix.close();await r.f.reopen();await r.f.flush();assert.equal(r.state.posts,1);assert.equal((await r.f.record()).ready,true);
});

test('concurrent native prepares share one intent; a summary waiting for flush cannot be overwritten by its old record',{timeout:120000},async t=>{
 const r=await setup(t);const prepared=await Promise.all(Array.from({length:8},()=>r.prepare()));assert.equal(new Set(prepared.map(p=>p.id)).size,1);r.f.rules({holdAt:3});const pending=r.f.flush();await r.f.entered(3);const revised=summary+' Revised while the prior publication was waiting for credentials.';let saved=false;const prepare=r.prepare(revised).then(()=>saved=true);await delay(50);assert.equal(saved,false);r.f.release(3);await pending;await prepare;assert.equal((await r.f.record()).summary,revised);assert.equal((await r.f.record()).ready,false);await r.f.flush();assert.equal((await r.f.record()).ready,true);assert.equal(r.state.posts,1);assert.equal(r.state.puts,1);
});

test('native publication refuses an unexpected remote branch and never overwrites it',{timeout:120000},async t=>{
 const r=await setup(t),p=await r.prepare();r.f.gitAt(r.f.operator,'push',r.f.remote,`main:refs/heads/${p.branch}`);await r.f.flush();const result=await r.f.record();assert.equal(result.push.state,'blocked');assert.match(result.error,/changed outside/);assert.equal(r.reference(p.branch),p.revision.base);assert.equal(r.state.posts,0);await r.f.flush();assert.equal(r.reference(p.branch),p.revision.base);
});

test('native publication rejects foreign open PRs and ambiguous durable markers',{timeout:120000},async t=>{
 for(const ambiguous of [false,true])await t.test(ambiguous?'ambiguous marker':'foreign PR',async t=>{
  const r=await setup(t),p=await r.prepare();const work=await r.f.c.command('work_record',{target:p.runId});r.f.gitAt(work.path,'push',r.f.remote,`HEAD:refs/heads/${p.branch}`);
  const pr={id:1,title:p.title,description:ambiguous?p.description:'Unrelated open PR',state:'OPEN',updated_on:r.state.date,source:{branch:{name:p.branch},commit:{hash:p.revision.head},repository:{uuid:p.repository.uuid}},destination:{branch:{name:p.target},commit:{hash:p.revision.base},repository:{uuid:p.repository.uuid}},links:{html:{href:'https://bitbucket.org/fixture/repository/pull-requests/1'}}};r.state.prs.push(pr);if(ambiguous)r.state.prs.push({...pr,id:2,links:{html:{href:'https://bitbucket.org/fixture/repository/pull-requests/2'}}});await r.f.flush();assert.equal(r.state.posts,0);const result=await r.f.record();assert.equal(result.ready,false);assert.match(result.error,ambiguous?/Multiple PRs/:/without the recorded/);assert.equal(result.pr.state,ambiguous?'pending':'blocked');
 });
});

test('native close during credential acquisition prevents a late PR mutation and drains its original job',{timeout:120000},async t=>{
 const r=await setup(t);await r.prepare();r.f.rules({holdAt:3});const pending=r.f.flush();await r.f.entered(3);let closed=false;const closing=r.f.c.command('publication_close').then(()=>closed=true);await delay(50);assert.equal(closed,false);r.f.release(3);await pending;await closing;assert.equal(r.state.posts,0);assert.equal((await r.f.record()).pr.state,'pending');await r.f.c.command('publication_close');
});

test('native stop verifies a live publication child and prevents its queued remote effect',{timeout:120000},async t=>{
 const r=await setup(t),p=await r.prepare();const latch=gitLatch(r.f.root,'push');try{await r.f.reopen();latch.arm();const pending=r.f.flush();await latch.blocked();await r.f.c.command('remote_stop',{target:p.runId});await pending;assert.equal((await r.f.record()).push.state,'uncertain');assert.throws(()=>r.reference(p.branch));assert.equal(r.state.posts,0);latch.release();await r.f.flush();assert.equal((await r.f.record()).ready,true);assert.equal(r.state.posts,1);}finally{latch.release();latch.restore();}
});

test('a PR changed during hosted reads cannot retain a stale description or identity confirmation',{timeout:120000},async t=>{
 const r=await setup(t);await r.publish();let gets=0;r.state.intercept=(req,_res)=>{if(/\/pullrequests\/1$/.test(req.url!) && ++gets===2)r.state.prs[0].description+='\nExternally revised description';return false;};await r.f.flush();let p=await r.f.record();assert.equal(p.ready,false);assert.ok(p.blockers.some((b:string)=>b.includes('description')));r.state.intercept=undefined;await r.f.flush();assert.equal((await r.f.record()).ready,true);assert.equal(r.state.puts,1);
 gets=0;r.state.intercept=(req,_res)=>{if(/\/pullrequests\/1$/.test(req.url!) && ++gets===2)r.state.prs[0].description='Lost durable marker during hosted observation';return false;};await r.f.flush();p=await r.f.record();assert.equal(p.ready,false);assert.match(p.error,/identity/);assert.equal(r.state.posts,1);assert.equal(r.state.puts,1);
});
