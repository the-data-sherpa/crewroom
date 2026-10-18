import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync, existsSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { Workbench } from '../../server/workbench';
import { acquireController } from '../../server/runtime/processes';
import { randomUUID } from 'node:crypto';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';
const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-rust-compat-')));
const path = join(root, 'workflow.sqlite');
const binary = resolve('target/debug/crewroomd');
let child: ReturnType<typeof spawn> | undefined;
let legacy: Workbench | undefined;
try {
  legacy = new Workbench(path);
  const original = legacy.importJira({siteUrl:'https://fixture.atlassian.net',issues:[{id:'1',key:'SIM-1',fields:{summary:'Original source',description:'Keep independent fields',status:{name:'Backlog'}}} ]})[0];
  legacy.refine(original.id,{revision:1,sourceRevision:1,brief:'Accepted in TypeScript',criteria:['Retain accepted history']});
  legacy.addNote(original.id,{mode:'decision',body:'An earlier decision'});
  seedRepository(join(root,'checkout'));
  const repository = saveRepositoryFixture(legacy,{name:'Compatibility fixture',path:join(root,'checkout'),targetBranch:'main',branchPrefix:'crewroom/',checks:[]});
  const local = legacy.createTask({title:'Queue compatibility',description:'A disposable plan',repositoryId:repository.id});
  legacy.refine(local.id,{revision:1,sourceRevision:1,brief:'Retain queue request identities',criteria:['Replay is safe across runtimes']});
  const queueInput = {revision:2,requestId:randomUUID()};
  const plan = legacy.queue(local.id,queueInput);
  const steeringInput = {requestId:randomUUID(),body:'Remain pending until a real runner accepts this.'};
  const steering = legacy.steer(plan.id,steeringInput);
  legacy.close();legacy = undefined;
  child = spawn(binary,['--data-dir',root,'--roles',join(root,'roles.json'),'--port','0'],{stdio:['ignore','pipe','pipe']});
  let diagnostic = '';child.stderr?.on('data',data=>diagnostic += String(data));
  const descriptor = join(root,'native-connection.json');
  for(let i=0;i<100 && !existsSync(descriptor);i++) {if(child.exitCode !== null) throw new Error(diagnostic);await delay(50);}
  const info = JSON.parse(readFileSync(descriptor,'utf8'));
  const request = async (path:string,body?:unknown) => {
    const response = await fetch(`${info.origin}/api${path}`,{headers:{Authorization:`Bearer ${info.token}`,...(body ? {'Content-Type':'application/json'}:{})},...(body ? {method:'POST',body:JSON.stringify(body)}:{})});
    const data = await response.json();assert.equal(response.ok,true,JSON.stringify(data));return data;
  };
  const initial = await request('/workspace');
  assert.equal(initial.tasks[0].id,original.id);assert.equal(initial.tasks[0].brief,'Accepted in TypeScript');assert.equal(initial.notes[0].body,'An earlier decision');
  await assert.rejects(acquireController(join(root,'controller')), /controller|authority|owned/i);
  const saved = await request(`/tasks/${original.id}/refinement`,{revision:2,sourceRevision:1,brief:'Accepted in Rust 日本語 🦀',criteria:['Survives both runtimes']});
  assert.equal(saved.revision,3);
  await request('/import/jira',{siteUrl:'https://fixture.atlassian.net/',issues:[{id:'1',key:'SIM-1',fields:{summary:'Refreshed through Rust',description:'Changed source',status:{name:'Review'}}}]});
  const current = await request('/workspace');assert.equal(current.tasks[0].brief,'Accepted in Rust 日本語 🦀');assert.equal(current.tasks[0].needsReacceptance,true);assert.equal(current.connections.runner,false);
  assert.deepEqual(await request(`/tasks/${local.id}/runs`,queueInput),plan);
  assert.deepEqual(await request(`/runs/${plan.id}/steering`,steeringInput),steering);
  const orderInput = {requestId:randomUUID(),expectedRevision:current.queue.revision,runIds:[plan.id]};
  const ordered = await request('/queue/order',orderInput);
  const cancelInput = {requestId:randomUUID()};
  const cancelled = await request(`/queue/${plan.id}/remove`,cancelInput);
  assert.equal(cancelled.status,'cancelled');
  seedRepository(join(root,'native-checkout'));
  const repoInput = {name:'Native repository',path:join(root,'native-checkout'),targetBranch:'main',checks:[{id:'unit',image:`sha256:${'a'.repeat(64)}`,command:'/usr/bin/true',args:[],timeoutMs:100,required:true}],requestId:randomUUID()};
  const nativeRepository = await request('/repositories',repoInput);
  rmSync(repoInput.path,{recursive:true,force:true});
  assert.deepEqual(await request('/repositories',repoInput),nativeRepository);
  const retireInput = {requestId:randomUUID(),revision:1};
  const retired = await request(`/repositories/${nativeRepository.id}/retire`,retireInput);
  const exited = once(child,'exit');child.kill('SIGTERM');await exited;child = undefined;
  legacy = new Workbench(path);
  const reopened = legacy.getTask(original.id);
  assert.equal(reopened.title,'Refreshed through Rust');assert.equal(reopened.jiraStatus,'Review');assert.equal(reopened.sourceRevision,2);assert.equal(reopened.revision,3);assert.equal(reopened.brief,'Accepted in Rust 日本語 🦀');
  assert.deepEqual(legacy.requirementRevisions(original.id).map(r=>r.brief),['Accepted in TypeScript','Accepted in Rust 日本語 🦀']);
  assert.deepEqual(legacy.reorderQueue(orderInput),ordered);
  assert.deepEqual(legacy.cancelQueued(plan.id,cancelInput),cancelled);
  assert.equal(legacy.steering(plan.id)[0].delivery,'not_delivered');
  assert.deepEqual(legacy.queue(local.id,queueInput),plan);
  assert.deepEqual(legacy.repositoryReplay(repoInput),nativeRepository);
  assert.deepEqual(legacy.retireRepository(nativeRepository.id,retireInput),retired);
  legacy.close();legacy = undefined;
  const controller = await acquireController(join(root,'controller'));
  try {
    const contender = spawn(binary,['--data-dir',root,'--roles',join(root,'roles.json'),'--port','0'],{stdio:'ignore'});
    const [code] = await once(contender,'exit');assert.notEqual(code,0);
  } finally {await controller.close();}
  console.log(JSON.stringify({passed:true,schema:26,cases:['TypeScript → Rust persistence','Rust → TypeScript persistence','source/refinement separation','Unicode persistence','accepted revision history','notes retained','queue request replay','steering request replay','queue reorder receipt replay','queue cancellation receipt replay','undelivered steering settlement','repository replay after checkout removal','repository retirement replay','Rust rejects TypeScript controller','TypeScript rejects Rust controller','SIGTERM releases ownership'],providerDelivery:'not exercised'},null,2));
} finally {
  legacy?.close();if(child && child.exitCode === null) {const exited=once(child,'exit');child.kill('SIGTERM');await exited;}
  rmSync(root,{recursive:true,force:true});
}
