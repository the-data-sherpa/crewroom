import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {existsSync,readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {once} from 'node:events';
import WebSocket from 'ws';
import {workflowFixture,until,acceptFixturePlan} from './workflow-support';
import {NativeDaemon} from './daemon-client';
const daemon=(f:Awaited<ReturnType<typeof workflowFixture>>)=>{assert.ok(f.c instanceof NativeDaemon);return f.c;};

test('production daemon authenticates operator and host APIs, delivers Mail, pushes histories and resumes independent processes',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'research',1,false,{service:true});const c=daemon(f);
 assert.equal((await c.request('/api/health')).executionAvailable,true);
 assert.equal((await c.request('/api/workspace')).connections.runner,true);
 const noOrigin=await fetch(c.info.origin+'/api/tasks',{method:'POST',headers:{Authorization:`Bearer ${c.info.token}`,'Content-Type':'application/json'},body:JSON.stringify({title:'Wrong boundary',description:''})});assert.equal(noOrigin.status,403);
 await f.start();let v=await f.idle();assert.equal(v.run.status,'running');assert.equal(v.attempts.length,2);assert.equal(new Set(v.attempts.map((a:any)=>a.hostPid)).size,2);
 const coordinator=v.assignments.find((a:any)=>a.coordinator),specialist=v.assignments.find((a:any)=>!a.coordinator);
 await assert.rejects(c.request('/internal/agent-message',{attemptId:coordinator.currentAttemptId,toolCallId:randomUUID(),arguments:{to:specialist.name,body:'Operator token cannot impersonate a host'}}),/authorized/);
 await assert.rejects(c.request('/api/workspace',undefined,f.host(coordinator.currentAttemptId).token),/403/);
 const ws=new WebSocket(c.info.origin.replace('http:','ws:')+'/api/live',{headers:{Authorization:`Bearer ${c.info.token}`,Origin:c.info.origin}});const received:any[]=[];ws.on('message',raw=>received.push(JSON.parse(String(raw))));await once(ws,'open');t.after(()=>ws.close());
 for(const topic of [`run:${f.run.id}`,`result:${f.run.id}`,`events:${f.run.id}`])ws.send(JSON.stringify({op:'subscribe',topic}));
 await f.input(coordinator.currentAttemptId,{kind:'tool',name:'send_message',arguments:{to:specialist.name,body:'Native daemon handoff'}});
 await until(async()=>(await f.snapshot()).messages.some((m:any)=>m.disposition==='received'));
 const steering=await c.request(`/api/runs/${f.run.id}/steering`,{requestId:randomUUID(),body:'Keep the research outcome scoped.'});
 await until(async()=>(await c.request('/api/workspace')).messages.some((s:any)=>s.id===steering.id&&s.delivery==='received'));
 await until(()=>received.some(v=>v.topic===`run:${f.run.id}`&&v.kind==='state')&&received.some(v=>v.kind==='events')&&received.some(v=>v.topic===`result:${f.run.id}`&&v.data?.research));
 assert.equal((await c.request(`/api/runs/${f.run.id}/result`)).publication.available,false);
 assert.equal((await f.pause()).status,'paused');v=await f.snapshot();assert.ok(v.attempts.every((a:any)=>a.ownership==='stopped'));
 await assert.rejects(f.tool(coordinator.currentAttemptId,'agent-message',{to:specialist.name,body:'Stale host'}),/authorized|no longer/);
 await f.resume();v=await f.idle();assert.equal(v.attempts.length,4);assert.equal(v.history.attempts.length,4);assert.equal(v.history.usage.billedUsd,null);
 await c.request(`/api/runs/${f.run.id}/cancel`,{});await assert.rejects(f.resume(),/cancel|Resume|Pause/);
});

test('production daemon crash reattaches the same hosts on its stable origin; a changed origin stops untrusted retained endpoints',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'research',1,false,{service:true});await f.start();const before=await f.idle();const old=daemon(f).info;
 await f.kill();await f.reopen();let v=await until(async()=>{const v=await f.snapshot();return v.observedAttemptIds.length===2?v:false;});
 assert.equal(daemon(f).info.origin,old.origin);assert.notEqual(daemon(f).info.token,old.token);assert.deepEqual(v.attempts.map((a:any)=>a.ompPid),before.attempts.map((a:any)=>a.ompPid));
 await f.kill();f.config.nativePort=0;await f.reopen();v=await f.snapshot();assert.notEqual(daemon(f).info.origin,old.origin);
 assert.ok(v.attempts.filter((a:any)=>before.attempts.some((b:any)=>b.id===a.id)).every((a:any)=>a.ownership==='stopped'));
 assert.ok(v.attempts.every((a:any)=>a.ownership==='stopped'||v.observedAttemptIds.includes(a.id)));assert.ok(v.run.status==='attention_required'||v.run.status==='paused'||v.run.status==='running');
});

test('native scheduler claims one coding slot, retains a paused holder and dispatches the next plan after Cancel',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true});const c=daemon(f);
 const secondTask=await c.request('/api/tasks',{title:'Second coding plan',description:'',repositoryId:f.repository.id});
 await c.request(`/api/tasks/${secondTask.id}/refinement`,{revision:1,sourceRevision:1,brief:'Second change',criteria:['Checked work']});
 await acceptFixturePlan(c,secondTask.id);
 const second=await c.request(`/api/tasks/${secondTask.id}/runs`,{revision:2});
 const launch={repository:{mode:'new'},team:f.team};
 await c.request(`/api/runs/${f.run.id}/launch`,{requestId:randomUUID(),launch});await f.idle();
 await c.request(`/api/runs/${second.id}/launch`,{requestId:randomUUID(),launch});
 const workspace=await c.request('/api/workspace');assert.equal(workspace.queue.slot?.runId,f.run.id);
 assert.equal((await c.request(`/api/runs/${second.id}/runtime`)).attempts.length,0);
 await f.pause();assert.equal((await c.request(`/api/runs/${second.id}/runtime`)).attempts.length,0);
 await c.request(`/api/runs/${f.run.id}/cancel`,{});
 const running=await until(async()=>{const v=await c.request(`/api/runs/${second.id}/runtime`);return v.run.status==='running'&&!v.busy?v:false;},30000);assert.equal(running.attempts.length,1);
 assert.equal((await c.request(`/api/runs/${f.run.id}/runtime`)).run.status,'cancelled');
 assert.equal((await c.request(`/api/runs/${second.id}/coding`)).checksAvailable,true);
});

test('a disconnected Start still finishes; unavailable Mail records a stopped hold and remains recoverable',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'research',0,false,{service:true});const c=daemon(f);f.holdMail();
 const abort=new AbortController();const pending=c.request(`/api/runs/${f.run.id}/start`,{team:f.team},undefined,abort.signal);const rejected=assert.rejects(pending,/abort/i);
 await until(()=>f.requests.length>0);abort.abort();await rejected;f.releaseMail();await f.idle();await f.pause();
 f.offline(true);await assert.rejects(f.resume(),/500: Unable to complete/);let v=await f.snapshot();assert.ok(v.attempts.every((a:any)=>a.state==='admitted'||a.ownership==='stopped'));
 f.offline(false);await f.resume();await f.idle();await c.close();
 assert.equal(existsSync(join(f.config.authority,'native-connection.json')),false);
 await f.reopen();v=await f.snapshot();assert.ok(v.attempts.every((a:any)=>a.ownership==='stopped'));
});

test('preflight failure holds an unprepared queued plan and the scheduler rechecks corrected role configuration',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'coding',0,false,{service:true});const c=daemon(f);const roles=JSON.parse(readFileSync(f.config.roles,'utf8'));roles.find((r:any)=>r.id==='coordinator').model='unavailable/model';writeFileSync(f.config.roles,JSON.stringify(roles));
 await c.request(`/api/runs/${f.run.id}/launch`,{requestId:randomUUID(),launch:{repository:{mode:'new'},team:f.team}});
 const held=await until(async()=>{const v=await f.snapshot();return v.run.launchHold?v:false;});assert.equal(held.attempts.length,0);assert.equal(held.run.launchHold.source,'preflight');
 assert.equal(existsSync(join(f.config.authority,'worktrees',f.run.id)),false);
 writeFileSync(f.config.roles,JSON.stringify(f.roles));await c.request(`/api/runs/${f.run.id}/steering`,{requestId:randomUUID(),body:'Launch prerequisites corrected.'});await f.idle();
 assert.equal((await f.snapshot()).run.status,'running');
});
