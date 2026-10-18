import assert from 'node:assert/strict';
import {appendFileSync,existsSync,readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {supervisorFixture,until} from './supervisor-support';

test('native supervisor binds real tmux host/runner identities and observes authenticated tool delivery',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);const identity=await f.launch();assert.notEqual(identity.hostPid,identity.ompPid);
 const intent=JSON.parse(readFileSync(join(f.attemptRoot(),'host-intent.json'),'utf8'));const owner=JSON.parse(readFileSync(join(intent.descriptor.root,'owner.json'),'utf8'));assert.equal(owner.pid,identity.hostPid);assert.equal(owner.specSha256,intent.descriptor.specSha256);
 assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'live');await f.observed();await f.c.command('activate',{target:f.seeds[0].run.id});
 const config=JSON.parse(readFileSync(join(f.attemptRoot(),'host.json'),'utf8'));assert.deepEqual(await f.c.command('auth',{target:f.seeds[0].attempt.id,authorization:`Bearer ${config.token}`}),{authorized:true,observed:true});assert.equal((await f.c.command('auth',{target:f.seeds[0].attempt.id,authorization:`Bearer ${config.providerToken}`})).authorized,false);
 await f.input('message',{kind:'tool',name:'send_message',arguments:{to:'Engineer',body:'Observed lifecycle marker'}});
 await until(async()=>(await f.events()).find((v:any)=>v.event.type==='prompt_result'&&v.event.id==='message'));
 assert.equal(f.b.requests[0].headers.authorization,`Bearer ${config.token}`);assert.equal(f.b.requests[0].body.attemptId,f.seeds[0].attempt.id);
 assert.ok(f.tmux('capture-pane','-p','-t',identity.pane).trim().length>0,'Host output must reach the real tmux pane');
 assert.equal((await f.stop()).stopped,true);assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'stopped');
});

test('cancelled launch waiter stays registered and keyed retries share one process',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);const config=f.hostConfig(0,{setupGate:'setup'});const args={target:f.seeds[0].attempt.id,revision:f.seeds[0].run.controlRevision,config};
 const one=f.c.command('launch',args,'first');const oneFailure=assert.rejects(one,/cancelled/);await until(()=>existsSync(join(f.attemptRoot(),'setup-waiting')));
 const duplicate=f.c.command('launch',args);await f.c.command('cancel_waiter',{requestId:'first'});await oneFailure;assert.equal(await f.c.command('busy',{target:f.seeds[0].run.id}),true);
 const other=await f.launch(1);assert.ok(other.hostPid);assert.equal((await f.c.command('snapshot')).runs.length,2);
 writeFileSync(join(f.attemptRoot(),'setup-release'),'go');const identity=await duplicate;assert.ok(identity.hostPid);await f.observed();
 const requests=readFileSync(join(f.attemptRoot(),'rpc-requests.jsonl'),'utf8').trim().split('\n').map(v=>JSON.parse(v));assert.equal(requests.filter(v=>v.type==='negotiate_protocol').length,1);
 await assert.rejects(f.c.command('launch',args),/already registered|prior attempt|control/);assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'live');
});

test('Stop fences a held launch and its queued same-run successor before identity admission',{timeout:45000},async t=>{
 const f=await supervisorFixture(t,2,true);const first=f.launch(0,{setupGate:'setup'});const firstFailure=assert.rejects(first,/stopp|control|identity|ownership/i);
 await until(()=>existsSync(join(f.attemptRoot(),'setup-waiting')));const second=f.launch(1);const secondFailure=assert.rejects(second,/stopp|control/i);
 await f.c.command('desired',{target:f.seeds[0].run.id,state:'paused'});const result=await f.stop();await Promise.all([firstFailure,secondFailure]);
 assert.equal(result.stopped,true);assert.equal(existsSync(join(f.attemptRoot(1),'host-intent.json')),false);assert.equal(existsSync(join(f.attemptRoot(1),'omp-process/launch.json')),false);
 const attempts=(await f.c.command('runtime',{target:f.seeds[0].run.id})).attempts;assert.equal(attempts[0].ownership,'stopped');assert.equal(attempts[1].state,'admitted');assert.equal(attempts[1].ownership,undefined);assert.ok(attempts.every((a:any)=>!a.hostPid));
});

test('close drains a cancelled waiter and prevents host launch during teardown',{timeout:45000},async t=>{
 const f=await supervisorFixture(t,2,true);const first=f.launch(0,{setupGate:'setup'}, {},'launch');const fail=assert.rejects(first,/cancelled/);
 await until(()=>existsSync(join(f.attemptRoot(),'setup-waiting')));await f.c.command('cancel_waiter',{requestId:'launch'});await fail;
 const close=f.c.command('close_runtime',{},'closing');const closed=assert.rejects(close,/cancelled/);await until(async()=>await f.c.command('busy',{target:'__close__'}));await f.c.command('cancel_waiter',{requestId:'closing'});await closed;
 await until(async()=>!(await f.c.command('busy',{target:f.seeds[0].run.id})));assert.equal((await f.c.command('owned',{target:f.seeds[0].attempt.id})).state,'stopped');
 await assert.rejects(f.launch(1),/stopping/);assert.equal(existsSync(join(f.attemptRoot(1),'host-intent.json')),false);
 assert.equal((await f.c.command('close_runtime')).stopped,true);
});

test('killed controller reattaches the same native host, runner and session without a second launch',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);const original=await f.launch();await f.c.command('activate',{target:f.seeds[0].run.id});await f.input('before',{});
 await until(async()=>(await f.events()).some((v:any)=>v.event.type==='prompt_result'&&v.event.id==='before'));
 await f.kill();await f.reopen();assert.equal((await f.c.command('reattach',{target:f.seeds[0].attempt.id})).reattached,true);
 const identity=await f.request({op:'identity'});for(const k of ['hostPid','ompPid','sessionId'])assert.equal(identity[k],original[k]);await f.observed();
 await f.input('after',{});await until(async()=>(await f.events()).some((v:any)=>v.event.type==='prompt_result'&&v.event.id==='after'));
 const requests=readFileSync(join(f.attemptRoot(),'rpc-requests.jsonl'),'utf8').trim().split('\n').map(v=>JSON.parse(v));assert.equal(requests.filter(v=>v.type==='negotiate_protocol').length,1);
});

test('restart stops an interrupted launch using its durable descriptor before saved PIDs exist',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);const launching=f.launch(0,{setupGate:'setup'});const failed=assert.rejects(launching,/exited/);await until(()=>existsSync(join(f.attemptRoot(),'setup-waiting')));
 await f.kill();await failed;await f.reopen();assert.equal((await f.c.command('attempt',{target:f.seeds[0].attempt.id})).hostPid,undefined);
 assert.equal((await f.stop()).stopped,true);assert.equal((await f.c.command('attempt',{target:f.seeds[0].attempt.id})).ownership,'stopped');
});

test('malformed complete journal records hold recovery while cleanup still stops every process',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);await f.launch();await f.kill();const path=join(f.attemptRoot(),'events.jsonl');appendFileSync(path,'{invalid complete record}\n');await f.reopen();
 const result=await f.stop();assert.equal(result.stopped,true);assert.match(result.attempts[0].journalError,/unreadable/);assert.equal((await f.c.command('attempt',{target:f.seeds[0].attempt.id})).ownership,'stopped');
});

test('wrong retained backend refuses reattachment and still permits explicit owned cleanup',{timeout:45000},async t=>{
 const f=await supervisorFixture(t);await f.launch();await f.kill();await f.reopen({backend:'http://127.0.0.1:59999'});
 await assert.rejects(f.c.command('reattach',{target:f.seeds[0].attempt.id}),/backend origin changed/);assert.equal((await f.stop()).stopped,true);
});
