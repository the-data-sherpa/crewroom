/** Production daemon/process lifecycle with SCRIPTED OMP responses and Mail.
 * Requires the immutable check image receipt built by for14-check-image.ts.
 * This proves supervision and containment boundaries, never instruction quality.
 */
import assert from 'node:assert/strict';
import {createHash,randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {copyFileSync,existsSync,readFileSync,writeFileSync} from 'node:fs';
import {createServer,request} from 'node:http';
import {join,resolve} from 'node:path';
import {setTimeout as sleep} from 'node:timers/promises';
import test,{type TestContext} from 'node:test';
import {workflowFixture,until,type FixtureCleanup} from './workflow-support';
import {NativeDaemon} from './daemon-client';
import {hashValue} from './for14-case-pack';

const imageReceipt=process.env.CREWROOM_FOR14_CHECK_IMAGE_RECEIPT??resolve('.crewroom/test-results/for14/check-image-v1-final/image-receipt.json');
const skip=!existsSync(imageReceipt)?'Build the protected image with for14-check-image.ts and set CREWROOM_FOR14_CHECK_IMAGE_RECEIPT.':false;
const parent=process.env.CREWROOM_FOR14_LIFECYCLE_ROOT;
async function fixture(t:TestContext,configure?:(root:string,config:any)=>Promise<void>) {
 const image=JSON.parse(readFileSync(imageReceipt,'utf8')).image;
 const cleanup:FixtureCleanup[]=[];
 const f=await workflowFixture(t,'coding',0,false,{service:true,...(parent?{parent}:{}),cleanup:r=>{cleanup.push(r);},configure:async({root,config})=>{
  // A real separately hashed executable exists, while launchPrefix explicitly
  // routes RPC to the existing scripted response fixture, never a provider.
  const executable=join(root,'scripted-omp-identity');copyFileSync('/usr/bin/false',executable);
  config.runtime.ompBinary=executable;config.runtime.evaluationImage=image;config.runtime.evaluationMode='scripted';
  await configure?.(root,config);
 }});
 assert.ok(f.c instanceof NativeDaemon);
 return Object.assign(f,{cleanup});
}
type Fixture=Awaited<ReturnType<typeof fixture>>;
function daemon(f:Fixture){assert.ok(f.c instanceof NativeDaemon);return f.c;}
async function create(f:Fixture){
 const c=daemon(f);const before=await c.request('/api/workspace');
 const current=before.roles.find((role:any)=>role.id==='implementer');
 const draft={...current,revision:current.revision??1,instructions:current.instructions+' Preserve incomplete verification honestly.'};
 const preview=await c.request('/api/comparisons/preflight',{kind:'role',targetId:'implementer',baseRevision:draft.revision,draft});
 assert.equal(preview.canStart,true,JSON.stringify(preview));assert.equal(preview.manifest.mode,'scripted');
 const input={requestId:randomUUID(),manifest:preview.manifest};
 const comparison=await c.request('/api/comparisons',input);
 assert.equal(comparison.cells.length,16);assert.equal(comparison.coverage.observed,0);
 const after=await c.request('/api/workspace');assert.deepEqual(after.tasks,before.tasks);assert.deepEqual(after.roles,before.roles);
 return {comparison,input,catalog:before.roles};
}
async function get(f:Fixture,id:string){return daemon(f).request(`/api/comparisons/${id}`);}
async function control(f:Fixture,id:string,action:string){return daemon(f).request(`/api/comparisons/${id}/control`,{requestId:randomUUID(),action});}
async function idle(f:Fixture,run:string){return until(async()=>{
 const value=await daemon(f).request(`/api/runs/${run}/runtime`);
 if(value.run.status==='attention_required')throw new Error(value.run.attentionReason);
 if(['completed','cancelled'].includes(value.run.status)){
  const comparison=await get(f,value.run.evaluation.comparisonId);
  throw new Error(`Evaluation stopped before idle: ${JSON.stringify(comparison.cells.find((cell:any)=>cell.runId===run)?.result)}`);
 }
 return !value.busy&&value.run.status==='running'&&value.assignments.length===2&&value.assignments.every((a:any)=>value.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?value:false;
},45000);}
async function stopped(f:Fixture,id:string){return until(async()=>{
 const value=await get(f,id);
 for(const cell of value.cells.filter((cell:any)=>cell.runId)){
  const run=await daemon(f).request(`/api/runs/${cell.runId}/runtime`);
  if(run.busy||run.attempts.some((a:any)=>a.state!=='admitted'&&a.ownership!=='stopped'))return false;
 }
 return value;
},45000);}

test('FOR-14 actual launched catalogs match frozen tools and evaluation messages address only frozen peers',{timeout:180000,skip},async t=>{
 const f=await fixture(t);const {comparison}=await create(f);await control(f,comparison.id,'start');
 const launched=await until(async()=>{const value=await get(f,comparison.id);return value.cells[0].runId?value:false;},30000);
 const firstRun=launched.cells[0].runId;let runtime=await idle(f,firstRun);
 const snapshots:any[]=[];
 function catalogs(value:any,phase:string){
  return value.assignments.map((assignment:any)=>{
   const actor=assignment.coordinator?'coordinator':'worker';
   const attempt=value.attempts.find((attempt:any)=>attempt.id===assignment.currentAttemptId);
   const wire=readFileSync(join(f.runtime,attempt.id,'rpc-requests.jsonl'),'utf8').trim().split('\n').map(line=>JSON.parse(line));
   const registrations=wire.filter(frame=>frame.type==='set_host_tools');
   assert.equal(registrations.length,1,'Inspect the actual production adapter registration received by the separate runner process');
   const actual=registrations[0].tools,expected=comparison.manifest.execution.tools[actor];
   assert.deepEqual(actual,expected,`${phase} ${actor} actual catalog must equal every frozen tool field`);
   const events=readFileSync(join(f.runtime,attempt.id,'events.jsonl'),'utf8').trim().split('\n').map(line=>JSON.parse(line).event);
   const ready=events.filter(event=>event.type==='host_ready');
   assert.equal(ready.length,1,'The ready receipt binds the catalog passed to this runner');
   assert.equal(ready[0].toolCatalogHashKind,'canonical-json-sha256');
   assert.equal(ready[0].toolCatalogHash,hashValue(actual),'The actual runner registration agrees with the retained host catalog hash');
   const peer=value.assignments.find((candidate:any)=>candidate.id!==assignment.id);
   assert.deepEqual(actual.find((tool:any)=>tool.name==='send_message').parameters.properties.to.enum,[peer.name]);
   const host=f.host(attempt.id);const prompt=host.args[host.args.indexOf('--system-prompt')+1];
   assert.equal(typeof prompt,'string');assert.ok(prompt.startsWith(assignment.role.instructions+'\n'));
   for(const marker of ['manifestHash','proposedHash','comparisonId','cellId','sourceCorrectness','falseCompletion','private/oracles.json','baseline/candidate'])assert.ok(!prompt.includes(marker),`Private marker exposed: ${marker}`);
   const captured={phase,actor,attemptId:attempt.id,promptSha256:createHash('sha256').update(prompt).digest('hex'),catalogSha256:createHash('sha256').update(JSON.stringify(actual)).digest('hex'),hostReadyCatalogSha256:ready[0].toolCatalogHash,recipients:[peer.name]};
   snapshots.push(captured);
   return {actor,prompt,promptTail:prompt.slice(assignment.role.instructions.length),assignment};
  });
 }
 const first=catalogs(runtime,'first launch');const originalAttempts=runtime.attempts.map((attempt:any)=>attempt.id);
 for(const assignment of runtime.assignments){
  const peer=runtime.assignments.find((candidate:any)=>candidate.id!==assignment.id);
  const before=(await daemon(f).request(`/api/runs/${firstRun}/runtime`)).messages.length;
  for(const to of [assignment.name,assignment.id,peer.id,randomUUID()]){
   await assert.rejects(f.tool(assignment.currentAttemptId,'agent-message',{to,body:'Unadmitted recipient must have no effect.'}),/frozen peer recipient/);
  }
  assert.equal((await daemon(f).request(`/api/runs/${firstRun}/runtime`)).messages.length,before,'Denied recipient creates no message intent');
  const sent=await f.tool(assignment.currentAttemptId,'agent-message',{to:peer.name,body:'Scripted peer delivery verifies the frozen tool contract.'});
  assert.equal(sent.from,assignment.id);assert.equal(sent.to,peer.id);
  await until(async()=>(await daemon(f).request(`/api/runs/${firstRun}/runtime`)).messages.some((message:any)=>message.id===sent.id&&message.disposition==='received'));
 }
 await idle(f,firstRun);await control(f,comparison.id,'pause');await stopped(f,comparison.id);
 await control(f,comparison.id,'resume');runtime=await idle(f,firstRun);
 assert.equal(runtime.attempts.length,4);assert.ok(runtime.assignments.every((assignment:any)=>!originalAttempts.includes(assignment.currentAttemptId)));
 catalogs(runtime,'replacement launch');
 const worker=runtime.assignments.find((assignment:any)=>!assignment.coordinator);
 await f.input(worker.currentAttemptId,{kind:'tool',name:'handoff_assignment',arguments:{status:'incomplete',summary:'Scripted catalog regression intentionally leaves the task incomplete.'}});
 const second=await until(async()=>{const value=await get(f,comparison.id);return value.cells[1].runId?value:false;},45000);
 const proposed=await idle(f,second.cells[1].runId);const next=catalogs(proposed,'paired variant launch');
 for(const actor of ['coordinator','worker']){
  const initial=first.find((entry:any)=>entry.actor===actor)!;const paired=next.find((entry:any)=>entry.actor===actor)!;
  assert.equal(initial.promptTail,paired.promptTail,'The paired composed prompt changes only the target instruction prefix');
  if(actor==='coordinator')assert.equal(initial.prompt,paired.prompt,'The supporting coordinator prompt is unchanged');
  else assert.notEqual(initial.assignment.role.instructions,paired.assignment.role.instructions,'The treatment reaches the evaluated worker');
 }
 await control(f,comparison.id,'cancel');await stopped(f,comparison.id);
 const receipt={mode:'scripted',comparisonId:comparison.id,manifestHash:comparison.manifestHash,actualCatalogsEqualFrozen:true,hostReadyCatalogHashesConfirmed:true,selfAndUuidRecipientsDeniedBeforeIntent:true,peerDeliveryConfirmed:true,pairedPromptIsolationConfirmed:true,snapshots};
 writeFileSync(join(f.root,'for14-tool-catalog-receipt.json'),JSON.stringify(receipt,null,2)+'\n',{mode:0o600});t.diagnostic(JSON.stringify(receipt));
});

test('FOR-14 scripted planned comparisons survive pause/reopen without launching and cancellation preserves all cells',{timeout:120000,skip},async t=>{
 const f=await fixture(t);const {comparison,input,catalog}=await create(f);
 await sleep(250);assert.ok((await get(f,comparison.id)).cells.every((c:any)=>!c.runId));
 await control(f,comparison.id,'pause');await f.kill();await f.reopen();
 const retained=await get(f,comparison.id);assert.equal(retained.desiredState,'paused');assert.ok(retained.cells.every((c:any)=>!c.runId));
 assert.deepEqual(await daemon(f).request('/api/comparisons',input),comparison,'Creation replay never starts a process');
 const cancelled=await control(f,comparison.id,'cancel');assert.equal(cancelled.cells.length,16);assert.ok(cancelled.cells.every((c:any)=>c.state==='cancelled'&&!c.runId));
 assert.equal(cancelled.coverage.observed,0);assert.deepEqual((await daemon(f).request('/api/workspace')).roles,catalog);
});

test('FOR-14 scripted cells share coding ownership, reject spoofed tools, and retain stopped/result history through restart',{timeout:180000,skip},async t=>{
 const f=await fixture(t);await f.start();await f.idle();
 const {comparison,input}=await create(f);await control(f,comparison.id,'start');
 await sleep(300);let value=await get(f,comparison.id);assert.ok(value.cells.every((c:any)=>!c.runId),'Coding ownership fences evaluation admission');
 await f.pause();await sleep(100);assert.ok((await get(f,comparison.id)).cells.every((c:any)=>!c.runId),'Paused coding still retains its slot');
 await daemon(f).request(`/api/runs/${f.run.id}/cancel`,{});
 const launched=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].runId?v:false;},30000);
 const run=launched.cells[0].runId;let runtime=await idle(f,run);
 const first=runtime.attempts.map((a:any)=>({id:a.id,hostPid:a.hostPid,ompPid:a.ompPid}));
 assert.equal(new Set(first.flatMap((a:any)=>[a.hostPid,a.ompPid])).size,4,'Coordinator and worker each have distinct OS processes');
 const worker=runtime.assignments.find((a:any)=>!a.coordinator);
 for(const route of ['coding/check-prepare','coding/check-finish','coding/shell','publication/prepare','learning/propose']){
  await assert.rejects(f.tool(worker.currentAttemptId,route,{}),/not registered|denied|unavailable/);
 }
 await assert.rejects(daemon(f).request(`/api/runs/${run}/budget`,{activeLimitMs:3600000}),/comparison controls/);
 await control(f,comparison.id,'pause');value=await stopped(f,comparison.id);assert.equal(value.state,'paused');
 assert.ok(value.cells[0].attempts.every((a:any)=>a.ownership==='stopped'));
 await f.kill();await f.reopen();value=await get(f,comparison.id);assert.deepEqual(value.cells[0].attempts.map((a:any)=>a.id),first.map((a:any)=>a.id));
 await control(f,comparison.id,'resume');runtime=await idle(f,run);
 assert.equal(runtime.attempts.length,4);assert.ok(runtime.assignments.every((a:any)=>!first.some((p:any)=>p.id===a.currentAttemptId)));
 const resumed=runtime.assignments.find((a:any)=>!a.coordinator);
 await f.input(resumed.currentAttemptId,{kind:'tool',name:'handoff_assignment',arguments:{status:'incomplete',summary:'Scripted lifecycle fixture deliberately leaves the behavior incomplete.'}});
 value=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].result?v:false;},45000);
 assert.equal(value.cells[0].result.observed,true,'Executed behavioral failure is retained as an observation');
 const resultHash=value.cells[0].resultHash;const attemptIds=value.cells[0].attempts.map((a:any)=>a.id);
 await control(f,comparison.id,'pause');await stopped(f,comparison.id);
 assert.deepEqual(await daemon(f).request('/api/comparisons',input),comparison);
 await f.kill();await f.reopen();value=await get(f,comparison.id);
 assert.equal(value.cells[0].resultHash,resultHash);assert.deepEqual(value.cells[0].attempts.map((a:any)=>a.id),attemptIds);
 assert.equal((await daemon(f).request('/api/workspace')).tasks.some((task:any)=>task.internal),false);
 await control(f,comparison.id,'cancel');await stopped(f,comparison.id);value=await get(f,comparison.id);
 assert.equal(value.cells.length,16);assert.equal(value.cells[0].resultHash,resultHash);
});

test('FOR-14 scripted frozen executable drift is unavailable without fabricated coverage or a leaked writer slot',{timeout:120000,skip},async t=>{
 const f=await fixture(t);const {comparison}=await create(f);
 writeFileSync(f.config.runtime.ompBinary,'identity changed before any process launched',{mode:0o700});
 await control(f,comparison.id,'start');
 const value=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].result?v:false;},45000);
 assert.equal(value.cells[0].result.state,'unavailable');assert.equal(value.cells[0].result.observed,false);
 assert.match(value.cells[0].result.reason,/identity|changed|unavailable/i);assert.equal(value.cells[0].attempts.length,0);
 await control(f,comparison.id,'pause');await stopped(f,comparison.id);
 const workspace=await daemon(f).request('/api/workspace');assert.ok(!workspace.queue.slot||workspace.queue.slot.runId!==value.cells[0].runId);
 assert.equal((await get(f,comparison.id)).coverage.observed,0);
 await control(f,comparison.id,'cancel');
});

test('FOR-14 scripted paused cells reject executable drift before launching replacement processes',{timeout:120000,skip},async t=>{
 const f=await fixture(t);const {comparison}=await create(f);await control(f,comparison.id,'start');
 const launched=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].runId?v:false;},30000);
 const runtime=await idle(f,launched.cells[0].runId);const attemptIds=runtime.attempts.map((a:any)=>a.id);
 await control(f,comparison.id,'pause');await stopped(f,comparison.id);
 writeFileSync(f.config.runtime.ompBinary,'identity changed while the cell was paused',{mode:0o700});
 await f.kill();await f.reopen();await control(f,comparison.id,'resume');
 const value=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].result?v:false;},45000);
 assert.equal(value.cells[0].result.state,'unavailable');assert.match(value.cells[0].result.reason,/identity|changed|unavailable/i);
 assert.deepEqual(value.cells[0].attempts.map((a:any)=>a.id),attemptIds,'Frozen identity mismatch must not create replacement hosts');
 assert.ok(value.cells[0].attempts.every((a:any)=>a.ownership==='stopped'));assert.equal(value.desiredState,'paused');
 assert.ok(value.cells.slice(1).every((cell:any)=>!cell.runId),'Unavailable preparation pauses later cells');
 const workspace=await daemon(f).request('/api/workspace');assert.ok(!workspace.queue.slot||workspace.queue.slot.runId!==value.cells[0].runId);
 await control(f,comparison.id,'cancel');
});

test('FOR-14 scripted crash after durable finalization stops cannot launch another worker generation',{timeout:180000,skip},async t=>{
 let holdNextLog=false,heldLog=false;
 let releaseLog:()=>void=()=>{};
 const gate=new Promise<void>(resolve=>{releaseLog=resolve;});
 let proxy:ReturnType<typeof createServer>|undefined;
 t.after(()=>releaseLog());
 const f=await fixture(t,async(root,config)=>{
  const socket=config.runtime.commandEngine.socket;
  proxy=createServer((req,res)=>{
   const delay=holdNextLog&&!!req.url?.includes('/logs?');
   if(delay)holdNextLog=false;
   const upstream=request({socketPath:socket,path:req.url,method:req.method,headers:req.headers},response=>{
    if(delay){
     const chunks:Buffer[]=[];response.on('data',chunk=>chunks.push(Buffer.from(chunk)));
     response.on('end',()=>{heldLog=true;void gate.then(()=>{
      if(!res.destroyed){res.writeHead(response.statusCode!,response.headers);res.end(Buffer.concat(chunks));}
     });});
    }else{res.writeHead(response.statusCode!,response.headers);response.pipe(res);}
   });
   upstream.on('error',()=>res.destroy());req.pipe(upstream);res.on('close',()=>upstream.destroy());
  });
  const proxySocket=join(root,'engine.sock');proxy.listen(proxySocket);await once(proxy,'listening');
  config.runtime.commandEngine.socket=proxySocket;
 });
 t.after(()=>{proxy?.closeAllConnections();proxy?.close();});
 const {comparison}=await create(f);await control(f,comparison.id,'start');
 const launched=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].runId?v:false;},30000);
 const runtime=await idle(f,launched.cells[0].runId);const attemptIds=runtime.attempts.map((a:any)=>a.id);
 const worker=runtime.assignments.find((a:any)=>!a.coordinator);
 holdNextLog=true;
 await f.input(worker.currentAttemptId,{kind:'tool',name:'handoff_assignment',arguments:{status:'incomplete',summary:'Scripted crash recovery leaves the task incomplete.'}});
 await until(async()=>{
  const cell=(await get(f,comparison.id)).cells[0];
  assert.ok(!cell.result,'Crash window must be observed before a terminal result commits');
  return heldLog&&cell.events.some((event:any)=>event.type==='finalization')&&cell.attempts.every((attempt:any)=>attempt.ownership==='stopped');
 },45000);
 const oracleIntent=JSON.parse(readFileSync(join(f.runtime,worker.currentAttemptId,'tools',createHash('sha256').update('final-oracle').digest('hex'),'container.json'),'utf8'));
 assert.equal(oracleIntent.startConfirmed,true);assert.equal(typeof oracleIntent.id,'string');
 await f.kill();releaseLog();await f.reopen();
 const value=await until(async()=>{const v=await get(f,comparison.id);return v.cells[0].result?v:false;},45000);
 assert.deepEqual(value.cells[0].attempts.map((attempt:any)=>attempt.id),attemptIds,'Recovery finishes the sealed source without another agent generation');
 assert.equal(value.cells[0].events.filter((event:any)=>event.type==='finalization').length,1);
 assert.equal(value.cells[0].result.observed,true);assert.ok(value.cells[0].attempts.every((attempt:any)=>attempt.ownership==='stopped'));
 await control(f,comparison.id,'pause');await stopped(f,comparison.id);
 const resultHash=(await get(f,comparison.id)).cells[0].resultHash;
 await f.kill();await f.reopen();assert.equal((await get(f,comparison.id)).cells[0].resultHash,resultHash);
 await control(f,comparison.id,'cancel');await stopped(f,comparison.id);
});
