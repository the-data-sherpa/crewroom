import assert from 'node:assert/strict';
import {readFileSync,writeFileSync,appendFileSync,rmSync} from 'node:fs';
import {join} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';

test('controller loss after durable peer dispatch recovers the host receipt without another Mail effect',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();const runtime=await f.idle();const coordinator=runtime.assignments[0],recipient=runtime.assignments[1];const root=join(f.runtime,recipient.currentAttemptId);
 writeFileSync(join(root,'prompt-wait'),'wait');await f.tool(coordinator.currentAttemptId,'agent-message',{to:recipient.name,body:'Recover this handoff once.'},'handoff');
 await until(async()=>(await f.snapshot()).messages[0]?.disposition==='dispatched');assert.equal(f.sent.size,1);await f.kill();writeFileSync(join(root,'prompt-release'),'release');rmSync(join(root,'prompt-wait'));
 await f.reopen();await until(async()=>(await f.snapshot()).messages[0]?.disposition==='received');await f.idle();const messages=(await f.snapshot()).messages;assert.equal(messages.length,1);assert.equal(f.sent.size,1);
 const prompts=readFileSync(join(root,'rpc-requests.jsonl'),'utf8').trim().split('\n').map(l=>JSON.parse(l)).filter(v=>v.type==='prompt');assert.equal(prompts.length,1);assert.equal(prompts[0].id,messages[0].id);
});

test('a corrupt durable event journal holds recovery after physical cleanup and never replaces agents',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();const runtime=await f.idle();const journal=join(f.runtime,runtime.assignments[0].currentAttemptId,'events.jsonl');await f.kill();appendFileSync(journal,'{"fixture-corrupt":\n');
 try{
  await f.reopen();const run=await f.c.command('run',{target:f.run.id});assert.equal(run.status,'attention_required');assert.match(run.attentionReason,/journal|event|JSON|JSONL|expected|pars|invalid/i);const after=await f.snapshot();assert.equal(after.attempts.length,2);assert.ok(after.attempts.every((a:any)=>a.ownership==='stopped'));
 }finally{
  const repaired=readFileSync(journal,'utf8').split('\n').filter(l=>{try{if(l)JSON.parse(l);return true;}catch{return false;}}).join('\n');writeFileSync(journal,repaired);await f.pause();
 }
});

test('a saved source conflict stops active hosts and is a stable hold without repeated repair or cleanup',{timeout:90000},async t=>{
 const f=await workflowFixture(t);await f.start();await f.idle();
 // Emulate an already accepted durable synchronization hold, not a live Jira response.
 const db=new DatabaseSync(f.database);db.prepare("UPDATE runs SET data=json_set(data,'$.sourceConflict',?) WHERE id=?").run('Imported requirements changed; explicit adoption is required.',f.run.id);db.close();await f.c.command('notify',{target:f.run.id});
 await until(async()=>(await f.c.command('run',{target:f.run.id})).status==='attention_required');await until(async()=>!(await f.c.command('busy',{target:f.run.id})));
 const runtime=await f.snapshot(),before=await f.c.command('run',{target:f.run.id});assert.ok(runtime.attempts.every((a:any)=>a.ownership==='stopped'));await assert.rejects(f.resume(),/requirements|source|adopt/i);
 for(let i=0;i<3;i++){await f.c.command('notify',{target:f.run.id});await until(async()=>!(await f.c.command('busy',{target:f.run.id})));}assert.deepEqual(await f.c.command('run',{target:f.run.id}),before);assert.deepEqual((await f.snapshot()).attempts,runtime.attempts);
});
