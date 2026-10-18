/** Production comparison finalization with SCRIPTED RPC peers and Mail.
 * Opt in only after the serial evaluated-writer slot has been released:
 * CREWROOM_FOR14_COMPLETION_EXECUTE=1 node --import tsx --test this-file
 * No live inference, accelerated budget, or production test hook is used.
 */
import assert from 'node:assert/strict';
import {createHash,randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {copyFileSync,existsSync,mkdirSync,readFileSync,rmSync,writeFileSync} from 'node:fs';
import {createServer,request} from 'node:http';
import {join,resolve} from 'node:path';
import test,{type TestContext} from 'node:test';
import {NativeDaemon} from './daemon-client';
import {workflowFixture,until} from './workflow-support';

const imageReceipt=process.env.CREWROOM_FOR14_CHECK_IMAGE_RECEIPT??resolve('.crewroom/test-results/for14/check-image-v1-final/image-receipt.json');
const skip=process.env.CREWROOM_FOR14_COMPLETION_EXECUTE!=='1'?'Explicitly release the serial evaluated-writer slot before running this regression.':!existsSync(imageReceipt)?'Build the protected FOR-14 check image first.':false;
function lines(path:string){return existsSync(path)?readFileSync(path,'utf8').trim().split('\n').filter(Boolean).map(line=>JSON.parse(line)):[];}

async function fixture(t:TestContext,configure?:(root:string,config:any)=>Promise<void>){
 const parent=process.env.CREWROOM_FOR14_COMPLETION_ROOT;
 if(parent)mkdirSync(parent,{recursive:true,mode:0o700});
 const f=await workflowFixture(t,'coding',0,false,{service:true,...(parent?{parent}:{}),cleanup:()=>{},configure:async({root,config})=>{
  const executable=join(root,'scripted-omp-identity');copyFileSync('/usr/bin/false',executable);
  config.runtime.ompBinary=executable;config.runtime.evaluationMode='scripted';
  config.runtime.evaluationImage=JSON.parse(readFileSync(imageReceipt,'utf8')).image;
  await configure?.(root,config);
 }});
 const daemon=()=>{assert.ok(f.c instanceof NativeDaemon);return f.c;};const c=daemon();
 const current=(await c.request('/api/workspace')).roles.find((role:any)=>role.id==='implementer');
 const draft={...current,revision:current.revision??1,instructions:current.instructions+' Preserve the final source and verification limits.'};
 const preview=await c.request('/api/comparisons/preflight',{kind:'role',targetId:'implementer',baseRevision:draft.revision,draft});
 assert.equal(preview.canStart,true,JSON.stringify(preview));assert.equal(preview.manifest.mode,'scripted');
 const comparison=await c.request('/api/comparisons',{requestId:randomUUID(),manifest:preview.manifest});
 const get=()=>daemon().request(`/api/comparisons/${comparison.id}`);
 const control=(action:string)=>daemon().request(`/api/comparisons/${comparison.id}/control`,{requestId:randomUUID(),action});
 await control('start');
 const run=await until(async()=>{const value=await get();return value.cells[0].runId||false;},30000);
 const runtime=()=>daemon().request(`/api/runs/${run}/runtime`);
 const initial=await until(async()=>{
  const value=await runtime();
  if(value.run.status==='attention_required')throw new Error(value.run.attentionReason);
  return value.run.status==='running'&&!value.busy&&value.assignments.length===2&&value.assignments.every((a:any)=>value.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?value:false;
 },45000);
 const worker=initial.assignments.find((a:any)=>!a.coordinator),coordinator=initial.assignments.find((a:any)=>a.coordinator);
 const requests=(attempt:string)=>lines(join(f.runtime,attempt,'rpc-requests.jsonl'));
 async function hostState(attempt:string){
  const endpoint=JSON.parse(readFileSync(join(f.runtime,attempt,'endpoint.json'),'utf8'));
  const response=await fetch(`http://127.0.0.1:${endpoint.port}`,{method:'POST',headers:{Authorization:`Bearer ${f.host(attempt).token}`},body:JSON.stringify({op:'state'}),signal:AbortSignal.timeout(15000)});
  assert.ok(response.ok,await response.clone().text());return response.json();
 }
 async function tool(attempt:string,name:string,args:any){
  const id=randomUUID(),wire='completion-tool-'+id;
  await f.input(attempt,{kind:'tool',id:wire,toolCallId:wire,name,arguments:args},id);
  const reply=await until(()=>requests(attempt).find(frame=>frame.type==='host_tool_result'&&frame.id===wire),30000);
  assert.ok(!reply.isError,JSON.stringify(reply));
  await until(()=>lines(join(f.runtime,attempt,'events.jsonl')).some(envelope=>envelope.event.type==='prompt_result'&&envelope.event.id===id));
  return JSON.parse(reply.result.content[0].text);
 }
 const original=await tool(worker.currentAttemptId,'repository_read',{path:'delivery.cjs'});
 const content=original.content.replace("  return days === 1 ? 'Tomorrow' : `In ${days} days`;","  if (days === 0) return 'Today';\n  return days === 1 ? 'Tomorrow' : `In ${days} days`;");
 assert.notEqual(content,original.content);
 await tool(worker.currentAttemptId,'write_file',{path:'delivery.cjs',expectedHash:original.hash,content});
 for(const checkId of ['unit','acceptance'])await tool(worker.currentAttemptId,'run_check',{checkId});
 await tool(coordinator.currentAttemptId,'send_message',{to:worker.name,body:'SCRIPTED coordinator remains active after the worker submits its final status.'});
 await until(async()=>(await runtime()).messages.some((message:any)=>message.disposition==='received'));
 const coordinatorRoot=join(f.runtime,coordinator.currentAttemptId);
 const supportingPromptId=randomUUID();
 await f.input(coordinator.currentAttemptId,{kind:'supporting-turn',gate:'supporting-turn'},supportingPromptId);
 await until(()=>existsSync(join(coordinatorRoot,'supporting-turn-waiting')));
 writeFileSync(join(coordinatorRoot,'busy'),'Keep the scripted supporting actor busy until owned stop.',{mode:0o600});
 // Direct scripted input does not synthesize an agent_start event. Assert the
 // authenticated native state used by agents_idle instead of guessing its
 // separately persisted activity label. The busy marker survives gate expiry.
 const supportingState=await hostState(coordinator.currentAttemptId);
 assert.equal(supportingState.isStreaming,true);
 const held=await runtime();
 assert.equal(held.attempts.length,2);assert.equal(held.run.activeLimitMs,180000);
 let released=false;
 const release=()=>{released=true;rmSync(join(coordinatorRoot,'busy'),{force:true});writeFileSync(join(coordinatorRoot,'supporting-turn-release'),'release');writeFileSync(join(f.runtime,worker.currentAttemptId,'handoff-result-release'),'release');};
 async function cancel(){if((await get()).state!=='cancelled')await control('cancel');await until(async()=>{const value=await get();return value.state==='cancelled'&&value.cells.filter((cell:any)=>cell.runId).every((cell:any)=>cell.attempts.every((attempt:any)=>attempt.state==='admitted'||attempt.ownership==='stopped'));},45000);}
 return {f,comparison,get,control,run,runtime,worker,coordinator,initial,held,content,release,cancel,released:()=>released,requests,hostState,supportingPromptId,supportingState};
}

type Fixture=Awaited<ReturnType<typeof fixture>>;
async function rejectStoppedEffects(x:Fixture){
 const cell=(await x.get()).cells[0],runtime=await x.runtime();
 assert.ok(cell.attempts.every((attempt:any)=>attempt.ownership==='stopped'),'These API assertions cover stopped ownership, not a live pre-stop race');
 const path=join(x.f.runtime,'cases',x.run,'workspace','delivery.cjs');
 const source=readFileSync(path,'utf8'),expectedHash=createHash('sha256').update(source).digest('hex');
 const calls=[
  {attempt:x.worker.currentAttemptId,route:'evaluation/write',args:{path:'delivery.cjs',expectedHash,content:source+'\n// forbidden late edit\n'}},
  {attempt:x.worker.currentAttemptId,route:'evaluation/edit',args:{path:'delivery.cjs',expectedHash,replacements:[{oldText:"'Today'",newText:"'Changed after handoff'"}]}},
  {attempt:x.worker.currentAttemptId,route:'evaluation/check',args:{checkId:'unit'}},
  {attempt:x.worker.currentAttemptId,route:'evaluation/handoff',args:{status:'incomplete',summary:'Forbidden second handoff.'}},
  {attempt:x.worker.currentAttemptId,route:'agent-message',args:{to:x.coordinator.name,body:'Forbidden worker message after handoff.'}},
  {attempt:x.coordinator.currentAttemptId,route:'agent-message',args:{to:x.worker.name,body:'Forbidden coordinator message after handoff.'}},
 ];
 const rejected=[];
 for(const call of calls){
  let reason='';
  await assert.rejects(x.f.tool(call.attempt,call.route,call.args),error=>{
   reason=String(error);assert.match(reason,/no longer owns active work|not authorized|no longer.*current|handoff|finaliz|sealed/i);return true;
  });
  rejected.push({route:call.route,attemptId:call.attempt,reason});
 }
 assert.equal(readFileSync(path,'utf8'),source);
 assert.deepEqual((await x.get()).cells[0].events,cell.events,'Rejected effects append no comparison event');
 assert.deepEqual((await x.runtime()).messages,runtime.messages,'Rejected peer calls create no message intent');
 assert.deepEqual((await x.get()).cells[0].attempts.map((attempt:any)=>attempt.id),cell.attempts.map((attempt:any)=>attempt.id));
 return {scope:'after-verified-stopped-ownership',rejected,sourceSha256:expectedHash,messageHistory:runtime.messages};
}

for(const status of ['complete','incomplete'] as const){
 test(`FOR-14 ${status} worker handoff finalizes while the supporting coordinator stays busy`,{timeout:150000,skip},async t=>{
  const x=await fixture(t);const {f,worker,coordinator}=x;let receipt:any;
  try{
   const promptId=randomUUID();
   await f.input(worker.currentAttemptId,{kind:'tool',toolCallId:'handoff-'+promptId,name:'handoff_assignment',arguments:{status,summary:status==='complete'?'SCRIPTED source is correct and both required checks passed.':'SCRIPTED source checks passed, but the worker explicitly declines to claim completion.'}},promptId);
   await until(async()=>lines(join(f.runtime,worker.currentAttemptId,'events.jsonl')).some(envelope=>envelope.event.type==='prompt_result'&&envelope.event.id===promptId)||(await x.get()).cells[0].events.some((event:any)=>event.type==='finalization'));
   const before=await x.runtime();
   const capturedCell=(await x.get()).cells[0];
   if(!capturedCell.events.some((event:any)=>event.type==='finalization')){
    assert.equal(before.attempts.find((attempt:any)=>attempt.id===worker.currentAttemptId).state,'idle','Reproduce the settled worker, continuously busy coordinator boundary');
    assert.equal((await x.hostState(coordinator.currentAttemptId)).isStreaming,true,'Supporting coordinator remains natively busy after worker settlement');
   }
   const capture=capturedCell.events.find((event:any)=>event.type==='handoff');assert.ok(capture);
   const started=Date.now();let value:any;
   try{value=await until(async()=>{const comparison=await x.get();return comparison.cells[0].result?comparison:false;},15000);}
   catch(error){
    const comparison=await x.get(),runtime=await x.runtime();
    const supportingStateAfter=await x.hostState(coordinator.currentAttemptId).catch(probeError=>({unavailable:true,error:String(probeError)}));
    receipt={mode:'scripted',passed:false,phase:'bounded-finalization',error:String(error),comparisonId:x.comparison.id,runId:x.run,manifestHash:x.comparison.manifestHash,handoff: capture,coordinatorGateReleased:x.released(),supportingPromptId:x.supportingPromptId,supportingStateBefore:x.supportingState,supportingStateAfter,runtime,cell:comparison.cells[0]};
    assert.fail('Accepted worker handoff did not finalize within 15s while the coordinator remained busy; retained the exact pre-cleanup diagnostic.');
   }
   const cell=value.cells[0],result=cell.result;
   assert.equal(x.released(),false,'Finalization must not depend on releasing the supporting actor');
   assert.equal(result.state,'completed');assert.equal(result.observed,true);
   assert.equal(result.scores.sourceCorrectness,true,'The independent oracle still assesses the exact stopped source');
   assert.equal(result.scores.completionClaim,status==='complete');assert.equal(result.scores.successful,status==='complete');
   assert.deepEqual(result.source,capture.payload.receipt.source,'Final assessment stays on the handoff source');
   assert.equal(cell.events.filter((event:any)=>event.type==='finalization').length,1);
   assert.deepEqual(cell.attempts.map((attempt:any)=>attempt.id),x.initial.attempts.map((attempt:any)=>attempt.id));
   assert.ok(cell.attempts.every((attempt:any)=>attempt.ownership==='stopped'));
   const terminal=await x.runtime();assert.equal(terminal.run.activeLimitMs,180000);
   assert.ok(result.activeMs>=x.held.run.activeMs&&result.activeMs<180000,'Finalization preserves elapsed time without spending or resetting the full allowance');
   assert.equal(value.manifestHash,x.comparison.manifestHash);
   const postStopFence=await rejectStoppedEffects(x);
   receipt={mode:'scripted',passed:true,status,comparisonId:value.id,runId:x.run,manifestHash:value.manifestHash,boundedFinalizationMs:Date.now()-started,coordinatorGateReleased:false,supportingPromptId:x.supportingPromptId,supportingStateBefore:x.supportingState,originalAttemptIds:x.initial.attempts.map((attempt:any)=>attempt.id),activeMs:result.activeMs,activeLimitMs:terminal.run.activeLimitMs,sourceHash:result.source.sha256,resultHash:cell.resultHash,completionClaim:result.scores.completionClaim,sourceCorrectness:result.scores.sourceCorrectness,successful:result.scores.successful,messageHistory:terminal.messages,postStopFence};
  }finally{
   if(receipt)writeFileSync(join(f.root,'for14-completion-receipt.json'),JSON.stringify(receipt,null,2)+'\n',{mode:0o600});
   x.release();await x.cancel();
  }
 });
}

for(const action of ['pause','cancel','source-mismatch'] as const){
 const title=action==='source-mismatch'?'FOR-14 changed sealed source is held after restart without a result or replacement actors':`FOR-14 accepted handoff survives ${action} and restart during stopped-source final assessment`;
 test(title,{timeout:210000,skip},async t=>{
  let holdNextLog=false,heldLog=false;
  let releaseLog:()=>void=()=>{};
  const gate=new Promise<void>(resolve=>{releaseLog=resolve;});
  let proxy:ReturnType<typeof createServer>|undefined;
  t.after(()=>releaseLog());
  const x=await fixture(t,async(root,config)=>{
   const socket=config.runtime.commandEngine.socket;
   // Delay a genuine Docker log response only after the worker checks finish.
   // The barrier is outside the daemon and remains under test control when the
   // daemon dies; neither source, database state nor oracle output is fabricated.
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
   const socketPath=join(root,'engine.sock');proxy.listen(socketPath);await once(proxy,'listening');
   config.runtime.commandEngine.socket=socketPath;
  });
  t.after(()=>{proxy?.closeAllConnections();proxy?.close();});
  let receipt:any;
  let sourceRestoration:{path:string,bytes:Buffer}|undefined;
  try{
   holdNextLog=true;
   await x.f.input(x.worker.currentAttemptId,{kind:'tool',name:'handoff_assignment',arguments:{status:'complete',summary:'SCRIPTED correct source and fresh required checks are final before controlled restart.'}});
   const before=await until(async()=>{
    const comparison=await x.get(),cell=comparison.cells[0];
    assert.ok(!cell.result,'The controlled crash must precede a committed result');
    return heldLog&&cell.events.some((event:any)=>event.type==='finalization')&&cell.attempts.every((attempt:any)=>attempt.ownership==='stopped')?comparison:false;
   },45000);
   const cell=before.cells[0],handoffs=cell.events.filter((event:any)=>event.type==='handoff'),intents=cell.events.filter((event:any)=>event.type==='finalization');
   assert.equal(handoffs.length,1);assert.equal(intents.length,1);
   assert.deepEqual(intents[0].payload.source,handoffs[0].payload.receipt.source);
   assert.deepEqual(cell.attempts.map((attempt:any)=>attempt.id),x.initial.attempts.map((attempt:any)=>attempt.id));
   const oraclePath=join(x.f.runtime,x.worker.currentAttemptId,'tools',createHash('sha256').update('final-oracle').digest('hex'),'container.json');
   const oracleIntent=JSON.parse(readFileSync(oraclePath,'utf8'));
   assert.equal(oracleIntent.startConfirmed,true);assert.equal(typeof oracleIntent.id,'string');
   const postStopFence=await rejectStoppedEffects(x);
   const controlled=action==='source-mismatch'?await x.get():await x.control(action);
   if(action!=='source-mismatch')assert.equal(controlled.desiredState,action==='pause'?'paused':'cancelled');
   assert.ok(controlled.cells.slice(1).every((other:any)=>!other.runId),'No future cell launches during the accepted assessment');
   assert.ok(!controlled.cells[0].result,'Pending assessment has no substituted incomplete result');
   receipt={mode:'scripted',passed:false,phase:'before-controlled-restart',action,comparisonId:x.comparison.id,runId:x.run,manifestHash:before.manifestHash,acceptedHandoff:handoffs[0],finalizationIntent:intents[0],originalAttemptIds:cell.attempts.map((attempt:any)=>attempt.id),source:handoffs[0].payload.receipt.source,postStopFence,oracle:{intentPath:oraclePath,id:oracleIntent.id,startConfirmed:oracleIntent.startConfirmed},beforeCrash:controlled};
   writeFileSync(join(x.f.root,'for14-completion-restart-receipt.json'),JSON.stringify(receipt,null,2)+'\n',{mode:0o600});
   await x.f.kill();
   if(action==='source-mismatch'){
    const path=join(x.f.runtime,'cases',x.run,'workspace','delivery.cjs');
    sourceRestoration={path,bytes:readFileSync(path)};
    // Explicit fault injection in isolated test state, after daemon exit and
    // verified stopped ownership. Never mutate an active comparison workspace.
    writeFileSync(path,Buffer.concat([sourceRestoration.bytes,Buffer.from('\n// Deliberate stopped-source mismatch fixture.\n')]),{mode:0o600});
   }
   releaseLog();await x.f.reopen();
   if(action==='source-mismatch'){
    const held=await until(async()=>{
     const comparison=await x.get(),runtime=await x.runtime();
     assert.ok(!comparison.cells[0].result,'Mismatched source must not produce an assessment');
     return comparison.state==='held'&&runtime.run.status==='attention_required'?{comparison,runtime}:false;
    },30000);
    assert.match(held.runtime.run.attentionReason,/source changed after finalization/);
    assert.equal(held.comparison.attentionReason,held.runtime.run.attentionReason);
    assert.equal(held.comparison.cells[0].attentionReason,held.runtime.run.attentionReason);
    assert.deepEqual(held.comparison.cells[0].attempts.map((attempt:any)=>attempt.id),cell.attempts.map((attempt:any)=>attempt.id));
    assert.ok(held.comparison.cells[0].attempts.every((attempt:any)=>attempt.ownership==='stopped'));
    assert.deepEqual(held.comparison.cells[0].events,cell.events,'The mismatch adds no handoff, check, edit or finalization event');
    assert.deepEqual(held.runtime.messages,postStopFence.messageHistory,'Restart dispatches no new peer messages');
    assert.ok(held.comparison.cells.slice(1).every((other:any)=>!other.runId));
    assert.equal(held.comparison.manifestHash,before.manifestHash);
    const heldFence=await rejectStoppedEffects(x);
    const retained=await x.get();
    assert.equal(retained.state,'held');assert.equal(retained.attentionReason,held.runtime.run.attentionReason);
    assert.ok(!retained.cells[0].result);
    receipt={...receipt,passed:true,phase:'sealed-source-mismatch-held',faultInjection:'Isolated source changed only after daemon and actor ownership stopped.',held:held.comparison,heldRuntime:held.runtime,heldFence,futureCellsUnlaunched:true};
    return;
   }
   const after=await until(async()=>{const comparison=await x.get();return comparison.cells[0].result?comparison:false;},45000);
   const finished=after.cells[0],result=finished.result;
   assert.equal(after.manifestHash,before.manifestHash);
   assert.equal(after.desiredState,action==='pause'?'paused':'cancelled');
   assert.ok(after.cells.slice(1).every((other:any)=>!other.runId));
   assert.deepEqual(finished.attempts.map((attempt:any)=>attempt.id),cell.attempts.map((attempt:any)=>attempt.id),'Restart cannot launch a replacement generation for accepted work');
   assert.ok(finished.attempts.every((attempt:any)=>attempt.ownership==='stopped'));
   assert.deepEqual(finished.events.filter((event:any)=>event.type==='handoff'),handoffs);
   assert.deepEqual(finished.events.filter((event:any)=>event.type==='finalization'),intents);
   assert.deepEqual((await x.runtime()).messages,postStopFence.messageHistory,'Restart dispatches no new peer messages');
   assert.equal(result.state,'completed');assert.equal(result.observed,true);
   assert.equal(result.scores.sourceCorrectness,true);assert.equal(result.scores.completionClaim,true);assert.equal(result.scores.successful,true);
   assert.deepEqual(result.source,handoffs[0].payload.receipt.source);
   assert.ok(result.activeMs>=x.held.run.activeMs&&result.activeMs<180000);
   receipt={...receipt,passed:true,phase:'recovered-assessment',resultHash:finished.resultHash,result,finalAttemptIds:finished.attempts.map((attempt:any)=>attempt.id),futureCellsUnlaunched:true};
  }catch(error){
   receipt={...receipt,mode:'scripted',passed:false,action,error:String(error)};throw error;
  }finally{
   if(receipt)writeFileSync(join(x.f.root,'for14-completion-restart-receipt.json'),JSON.stringify(receipt,null,2)+'\n',{mode:0o600});
   try{
    releaseLog();x.release();
    if(sourceRestoration)writeFileSync(sourceRestoration.path,sourceRestoration.bytes,{mode:0o600});
    await x.cancel();
    if(sourceRestoration){
     const cleaned=await until(async()=>{const comparison=await x.get();return comparison.cells[0].result?comparison:false;},45000),cell=cleaned.cells[0];
     assert.equal(cell.result.state,'completed');assert.equal(cell.result.scores.sourceCorrectness,true);
     assert.deepEqual(cell.result.source,receipt.source);
     assert.deepEqual(cell.attempts.map((attempt:any)=>attempt.id),receipt.originalAttemptIds);
     assert.ok(cleaned.cells.slice(1).every((other:any)=>!other.runId));
     receipt={...receipt,cleanupAssessment:{originalBytesRestored:true,action:'cancel',resultHash:cell.resultHash,result:cell.result,originalAttemptIdsRetained:true}};
    }
   }catch(error){receipt={...receipt,mode:'scripted',passed:false,cleanupError:String(error)};throw error;}
   finally{if(receipt)writeFileSync(join(x.f.root,'for14-completion-restart-receipt.json'),JSON.stringify(receipt,null,2)+'\n',{mode:0o600});}
  }
 });
}
