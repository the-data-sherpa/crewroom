/** SCRIPTED provider/Mail, production daemon and real contained checks.
 * The disposable active-time ledger is accelerated; no live timing result or
 * instruction-quality conclusion is inferred from this fault-injection test.
 */
import assert from 'node:assert/strict';
import {createHash,randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {copyFileSync,existsSync,mkdirSync,readFileSync,writeFileSync} from 'node:fs';
import {createServer,request} from 'node:http';
import {join,resolve} from 'node:path';
import {DatabaseSync} from 'node:sqlite';
import {setTimeout as sleep} from 'node:timers/promises';
import test from 'node:test';
import {NativeDaemon} from './daemon-client';
import {workflowFixture,until} from './workflow-support';

const imageReceipt=process.env.CREWROOM_FOR14_CHECK_IMAGE_RECEIPT??resolve('.crewroom/test-results/for14/check-image-v1-final/image-receipt.json');
const skip=!existsSync(imageReceipt)?'Build the protected FOR-14 check image first.':false;

test('FOR-14 budget stops actors while the workflow lane awaits a slow check receipt',{timeout:120000,skip},async t=>{
 let holdNextLog=false,heldLog=false,releasedLog=false;
 let releaseLog:()=>void=()=>{};
 const gate=new Promise<void>(resolve=>{releaseLog=()=>{releasedLog=true;resolve();};});
 let proxy:ReturnType<typeof createServer>|undefined;
 t.after(()=>{releaseLog();});
 const retention=process.env.CREWROOM_FOR14_BUDGET_ROOT;
 if(retention)mkdirSync(retention,{recursive:true,mode:0o700});
 const f=await workflowFixture(t,'coding',0,false,{
  service:true,...(retention?{parent:retention}:{}),cleanup:()=>{},
  configure:async({root,config})=>{
   const executable=join(root,'scripted-omp-identity');copyFileSync('/usr/bin/false',executable);
   config.runtime.ompBinary=executable;
   config.runtime.evaluationImage=JSON.parse(readFileSync(imageReceipt,'utf8')).image;
   config.runtime.evaluationMode='scripted';
   const socket=config.runtime.commandEngine.socket;
   proxy=createServer((req,res)=>{
    // Hold only the first final log reply. Host cleanup requests keep their own
    // independent path to Docker, so a blocked workflow lane cannot explain a
    // successful actor stop. Release before the engine's five-second timeout.
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
  },
 });
 t.after(()=>{proxy?.closeAllConnections();proxy?.close();});
 assert.ok(f.c instanceof NativeDaemon);const c=f.c;
 const current=(await c.request('/api/workspace')).roles.find((role:any)=>role.id==='implementer');
 const draft={...current,revision:current.revision??1,instructions:current.instructions+' Preserve verification limits.'};
 const preview=await c.request('/api/comparisons/preflight',{kind:'role',targetId:'implementer',baseRevision:draft.revision,draft});
 assert.equal(preview.canStart,true,JSON.stringify(preview));
 const comparison=await c.request('/api/comparisons',{requestId:randomUUID(),manifest:preview.manifest});
 const control=(action:string)=>c.request(`/api/comparisons/${comparison.id}/control`,{requestId:randomUUID(),action});
 const get=()=>c.request(`/api/comparisons/${comparison.id}`);
 await control('start');
 const run=await until(async()=>{const value=await get();return value.cells[0].runId||false;},30000);
 const runtime=await until(async()=>{
  const value=await c.request(`/api/runs/${run}/runtime`);
  if(value.run.status==='attention_required')throw new Error(value.run.attentionReason);
  return !value.busy&&value.run.status==='running'&&value.assignments.length===2&&value.assignments.every((a:any)=>value.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?value:false;
 },30000);
 const worker=runtime.assignments.find((a:any)=>!a.coordinator);
 const original=await f.tool(worker.currentAttemptId,'evaluation/read',{path:'delivery.cjs'});
 await f.tool(worker.currentAttemptId,'evaluation/write',{path:'delivery.cjs',expectedHash:original.hash,content:"'use strict';\nexports.deliveryLabel = function () { while (true) {} };\n"});
 // Accelerate elapsed history only. The public frozen allowance remains 180 s.
 // This avoids a three-minute wait while exercising the actual budget boundary.
 const remainingMs=3500;
 const db=new DatabaseSync(f.database);
 try{
  db.exec('PRAGMA busy_timeout=5000; BEGIN IMMEDIATE');
  const row=db.prepare('SELECT data FROM runs WHERE id=?').get(run)!;
  const value=JSON.parse(String(row.data));assert.equal(value.status,'running');assert.equal(value.activeLimitMs,180000);
  value.activeMs=value.activeLimitMs-remainingMs;value.activeSince=Date.now();
  db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(value),run);db.exec('COMMIT');
 }finally{db.close();}
 holdNextLog=true;
 const key=randomUUID();let checkSettled=false;
 const check=f.tool(worker.currentAttemptId,'evaluation/check',{checkId:'unit'},key)
  .then(value=>({value}),error=>({error:String(error)})).finally(()=>{checkSettled=true;});
 const jobRoot=join(f.runtime,worker.currentAttemptId,'tools',createHash('sha256').update(key).digest('hex'));
 try{
  await until(()=>existsSync(join(jobRoot,'container.json'))&&JSON.parse(readFileSync(join(jobRoot,'container.json'),'utf8')).startConfirmed,10000);
  const spec=JSON.parse(readFileSync(join(jobRoot,'container.json'),'utf8')).spec;
  assert.ok(spec.timeoutMs>0&&spec.timeoutMs<=remainingMs,`Check timeout must use remaining allowance: ${spec.timeoutMs}`);
  await until(()=>heldLog,10000);
  assert.equal(checkSettled,false,'Fault injection must retain a blocked workflow check');
  const holdStartedAt=Date.now();
  const stopped=await until(async()=>{
   const value=await c.request(`/api/runs/${run}/runtime`);
   return value.run.pauseReason==='budget'&&value.attempts.every((a:any)=>a.ownership==='stopped')?value:false;
  },3500);
  assert.equal(releasedLog,false,'Actors must stop before the blocked receipt is released');
  assert.equal(checkSettled,false,'Actor stop must not depend on workflow tool completion');
  assert.ok(stopped.run.activeMs>=stopped.run.activeLimitMs);
  assert.equal(stopped.attempts.length,2,'Budget expiry must not launch replacement actors');
  await control('pause');
  releaseLog();const outcome=await check;
  const journal=JSON.parse(readFileSync(join(jobRoot,'container.json'),'utf8'));
  assert.equal(journal.removed,true,'Check ownership must be reconciled and removed');
  assert.equal(journal.receipt.started,true,'A real contained check must have started');
  assert.equal(journal.receipt.completed,false,'Interrupted work must not become a completed check');
  assert.ok(journal.timedOut||journal.stopRequested,'Slow check must be interrupted at the allowance');
  const evidence={mode:'scripted',comparisonId:comparison.id,runId:run,manifestHash:comparison.manifestHash,acceleratedElapsedMs:180000-remainingMs,remainingMs,checkTimeoutMs:spec.timeoutMs,actorsStoppedWhileReceiptHeld:true,heldUntilStoppedMs:Date.now()-holdStartedAt,attemptIds:stopped.attempts.map((a:any)=>a.id),check:outcome,container:{removed:journal.removed,timedOut:journal.timedOut,stopRequested:journal.stopRequested}};
  writeFileSync(join(f.root,'for14-budget-receipt.json'),JSON.stringify(evidence,null,2)+'\n',{mode:0o600});
  t.diagnostic(JSON.stringify(evidence));
 }finally{
  releaseLog();await check;await control('cancel');
  await until(async()=>{const value=await get();return value.state==='cancelled'&&value.cells.filter((cell:any)=>cell.runId).every((cell:any)=>cell.attempts.every((a:any)=>a.state==='admitted'||a.ownership==='stopped'))?value:false;},30000);
 }
 await sleep(25);
 assert.ok((await get()).cells.slice(1).every((cell:any)=>!cell.runId),'No later cell launches after the budget pause');
});
