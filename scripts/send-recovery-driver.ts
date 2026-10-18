import assert from 'node:assert/strict';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { Workbench } from '../server/workbench';
import { backendFixture } from './backend-fixture';
import { fixtureRepository } from './fixture-repository';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await backendFixture(config);
const request = async (path: string, body?: unknown): Promise<any> => {
  const response = await fetch(app.origin+'/api'+path, body === undefined ? {} : {method:'POST',headers:{Origin:app.origin,'Content-Type':'application/json'},body:JSON.stringify(body)});
  const value = await response.json(); assert.ok(response.ok,JSON.stringify(value)); return value;
};
const observe = createRuntimeObserver(request);
try {
  const repo = await request('/repositories',{name:'Fixture',path:fixtureRepository(config.root),targetBranch:'main'});
  const task = await request('/tasks',{title:'Committed send interruption',description:'',repositoryId:repo.id});
  await request(`/tasks/${task.id}/refinement`,{revision:1,sourceRevision:1,brief:'Preserve one logical send across coordinator loss.',criteria:['One delivery and causal reply']});
  const run = await request(`/tasks/${task.id}/runs`,{revision:2});
  await request(`/runs/${run.id}/start`,{team:[
    {name:'Coordinator',roleId:'triage',coordinator:true,input:'Send COMMITTED_BEFORE_CRASH to SpecialistOne.'},
    {name:'SpecialistOne',roleId:'implementer',coordinator:false,input:'Reply once to the coordinator message.'},
  ]});
  const deadline = Date.now()+15000;
  while (!existsSync(join(config.root,'send-committed')) && Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,20));
  assert.ok(existsSync(join(config.root,'send-committed')));
  const before = await observe(run.id);
  const original = before.attempts.find((a:any)=>a.id===before.assignments[0].currentAttemptId);
  assert.equal(before.messages.length,1);
  assert.equal(before.messages[0].transportId,undefined,'The external send committed before the application recorded its reply.');
  app.freeze();
  const durable=new Workbench(join(config.root,'workflow.sqlite'));
  try { durable.beginPause(run.id); } finally { durable.close(); }
  process.kill(original.ompPid,'SIGKILL');
  const specialist = before.attempts.find((a:any)=>a.id===before.assignments[1].currentAttemptId);
  process.kill(specialist.ompPid,0); process.kill(specialist.hostPid,0);
  writeFileSync(join(config.root,'release-send'),'drop reply');
  await app.restart();
  // Restart retains the pending action; explicit Resume reconciles before dispatching any new work.
  await request(`/runs/${run.id}/pause`,{});
  await request(`/runs/${run.id}/resume`,{});
  const completed = (value: any) => {
    const id=value.assignments.find((a:any)=>a.coordinator).currentAttemptId;
    return value.attempts.find((a:any)=>a.id===id)?.state==='idle' && value.events.some((e:any)=>e.attemptId===id && e.event.type==='message_end' && e.event.message?.role==='assistant' && e.event.message.content?.some((p:any)=>p.type==='text' && p.text==='SEND_RECOVERY_COMPLETE'));
  };
  let state: any;
  const until = Date.now()+35000;
  do {
    state=await observe(run.id);
    if (completed(state) && state.messages.length===2 && state.messages.every((m:any)=>m.transportId && m.disposition==='received')) break;
    await new Promise(resolve=>setTimeout(resolve,50));
  } while(Date.now()<until);
  assert.ok(completed(state),JSON.stringify(state));
  assert.equal(state.messages.length,2);
  assert.equal(state.messages[0].id,before.messages[0].id);
  assert.equal(state.messages[1].replyTo,state.messages[0].id);
  assert.ok(state.messages.every((m:any)=>m.transportId && m.disposition==='received'));
  assert.equal(state.attempts.find((a:any)=>a.id===original.id).ownership,'stopped');
  const oldHost=JSON.parse(readFileSync(join(config.runtime.root,original.id,'host.json'),'utf8'));
  const stale=await fetch(app.origin+'/internal/agent-message',{method:'POST',headers:{Origin:app.origin,'Content-Type':'application/json',Authorization:`Bearer ${oldHost.token}`},body:JSON.stringify({attemptId:original.id,toolCallId:'stale-send',arguments:{to:'SpecialistOne',body:'STALE_EFFECT'}})});
  assert.equal(stale.status,409);
  assert.equal((await observe(run.id)).messages.length,2);
  await request(`/runs/${run.id}/pause`,{});
  console.log(JSON.stringify({passed:true,runId:run.id,messages:state.messages,oldCoordinator:original.id,specialistObservableAtCrash:true}));
} finally { app.unfreeze(); await app.close(); }
