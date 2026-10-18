/** Opt-in actual MH6 inference; disposable production daemon, Git, Docker and Rust Mail. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {randomUUID,createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createServer} from 'node:http';
import type {TestContext} from 'node:test';
import {NativeDaemon} from './daemon-client';
import {workflowFixture,until} from './workflow-support';
import {nativeUi} from './native-ui-driver';
const setup=JSON.parse(readFileSync(process.argv[2],'utf8'));
const cleanup:Array<()=>Promise<void>>=[];
const context={after:(f:()=>Promise<void>)=>cleanup.push(f),diagnostic:console.log} as TestContext;
const report:any={scope:'LIVE pair inference; production daemon/native controls, external Rust Mail, actual Git and Docker; no Jira/publication',job:setup.job,passed:false,steps:[]};
let finishing:Promise<void>|undefined;
function persist(){writeFileSync(join(setup.root,'pair-result.json'),JSON.stringify(report,null,2)+'\n',{mode:0o600});}
function finish(){return finishing??=(async()=>{for(const close of cleanup.reverse())try{await close();}catch(error){report.passed=false;report.cleanupError=String(error);}persist();console.log(JSON.stringify(report,null,2));})();}
process.once('SIGTERM',()=>{report.error='External time limit';void finish().then(()=>process.exit(1));});
const backendSource=`export function page(events, after, limit) { return events.slice(0, limit); }\n`;
const backendTest=`import assert from 'node:assert/strict';\nimport {page} from './page.mjs';\nassert.deepEqual(page([{id:0},{id:1},{id:2}],0,1),{items:[{id:1}],nextCursor:1});\nassert.deepEqual(page([{id:0},{id:1}],null,5),{items:[{id:0},{id:1}],nextCursor:null});\nassert.throws(()=>page([],null,0));\nconsole.log('pagination acceptance passed');\n`;
const uiSource=`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Team finder</title><body><main><h1>Team finder</h1><label for="search">Search people</label><input id="search" type="search"><ul id="people"><li>Ada Lovelace</li><li>Grace Hopper</li><li>Alan Turing</li></ul><p id="empty" hidden>No people match</p></main></body></html>\n`;
try {
 const common='This is a bounded live acceptance job. You are a real supervised agent. Use only Crewroom tools. On initial/recovery assignments, acknowledge briefly and wait for a concrete STEP instruction. Do not independently advance stages or respond to routine peer messages; later STEP instructions request your substantive contribution. Do not call finish_implementation or checkpoint unless that STEP requests it. Read current choreography_status before every choreography mutation and retry a stale revision by reloading. Do not modify validation to hide failures. End your turn after each requested step. Preserve exact model/route and all receipts.';
 let releaseMail:(()=>void)|undefined;let mailGate:Promise<void>|undefined;
 const proxy=createServer(async(req,res)=>{try{let body='';for await(const chunk of req)body+=chunk;const call=JSON.parse(body);if(call.params?.name==='send_message')await mailGate;const upstream=await fetch(setup.mail.url,{method:'POST',headers:{'content-type':req.headers['content-type']??'application/json',accept:req.headers.accept??'application/json','authorization':req.headers.authorization??''},body});res.writeHead(upstream.status,Object.fromEntries([...upstream.headers].filter(([key])=>!['content-encoding','transfer-encoding','content-length'].includes(key))));res.end(Buffer.from(await upstream.arrayBuffer()));}catch(error){res.writeHead(502);res.end(String(error));}});
 await new Promise<void>(resolve=>proxy.listen(0,'127.0.0.1',resolve));
 cleanup.push(async()=>{releaseMail?.();proxy.closeAllConnections();await new Promise<void>(resolve=>proxy.close(()=>resolve()));});
 const mail={...setup.mail,url:'http://127.0.0.1:'+(proxy.address() as any).port};
 const f=await workflowFixture(context,'coding',0,false,{parent:setup.root,service:true,configure({source,config,roles}){
  writeFileSync(join(source,setup.job==='ui'?'index.html':'page.mjs'),setup.job==='ui'?uiSource:backendSource);
  if(setup.job==='backend')writeFileSync(join(source,'check.mjs'),backendTest);
  writeFileSync(join(source,'README.md'),'# Pair fixture\n\nBehavior is not documented yet.\n');
  execFileSync('git',['-C',source,'add','.']);execFileSync('git',['-C',source,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','Seed bounded pair job'],{stdio:'ignore'});
  delete config.runtime.launchPrefix;
  config.runtime.codex={binary:setup.codex.codex,authFile:setup.codex.authFile};
  config.runtime.claudeCode={binary:setup.claude.claude,home:setup.claude.nativeHome};
  config.runtime.ompBinary=setup.omp.omp;config.runtime.models=setup.omp.ompModels;config.runtime.mail=mail;
  config.runtime.catalog=[{adapter:'codex',provider:'codex',id:setup.codex.model,api:'codex-app-server',inputs:['text'],efforts:['low'],tools:true},{adapter:'claude-code',provider:'claude-code',id:'opus',resolvedModel:setup.claude.resolvedModel,api:'claude-code-stream',inputs:['text'],efforts:['low'],tools:true},setup.omp.catalogProfile];
  const template=roles.find(r=>r.id==='tech-writer')!;
  roles.push({...template,id:'navigator',name:'Navigator'},{...template,id:'ui-specialist',name:'UI specialist'});
  for(const role of roles){role.model=role.id==='navigator'?'claude-code/opus':role.id==='tech-writer'?setup.omp.catalogProfile.provider+'/'+setup.omp.model:'codex/'+setup.codex.model;role.effort='low';role.instructions=common;}
  writeFileSync(config.roles,JSON.stringify(roles),{mode:0o600});
 }});
 report.fixtureRoot=f.root;persist();const client=()=>f.c as NativeDaemon;
 const requirement=setup.job==='backend'?'Implement page(events, after, limit) in page.mjs for ascending unique nonnegative integer event IDs. after is null or an ID (zero is valid). Return at most limit events whose id is greater than after, and nextCursor equal to the last returned id only if more matching events remain, otherwise null. Validate positive integer limit and reject invalid inputs. Preserve input order and do not mutate input. Add executable edge-case tests. Document semantics.':'Implement a responsive accessible people filter in index.html. Search is case-insensitive and trims whitespace. The list updates while typing; show a live empty state for no matches. Escape clears the query, restores all people and keeps keyboard focus in the search input. Make it usable at 390px. Keep it self-contained and document usage.';
 const task=await client().command('task',{target:f.task.id});
 const refined=await client().request(`/api/tasks/${task.id}/refinement`,{revision:task.revision,sourceRevision:task.sourceRevision,brief:requirement,criteria:[requirement]});
 const repo=(await client().request('/api/workspace')).repositories.find((r:any)=>r.id===f.repository.id);
 await client().request('/api/repositories/'+repo.id,{requestId:randomUUID(),revision:repo.revision,name:repo.name,path:repo.path,targetBranch:repo.targetBranch,checks:[{id:'required',image:setup.checkImage,command:'/usr/local/bin/node',args:setup.job==='backend'?['check.mjs']:['/opt/check-ui.cjs'],timeoutMs:45000,memoryMiB:1024,tmpExecutable:true,required:true}]});
 const source=await client().command('plan_source',{target:task.id,input:{revision:refined.revision}});
 const prior=(await client().request('/api/workspace')).intake[task.id].plans.at(-1);
 const plan=await client().command('plan_save',{target:task.id,input:{requestId:randomUUID(),revision:refined.revision,sourceId:source.id,predecessorId:prior.id,content:{schemaVersion:1,goal:requirement,scope:[requirement],exclusions:['No provider publication or Jira update'],assumptions:[],decisions:[],workPackages:[{id:'fixture',name:setup.job+' behavior',dependsOn:[],outputs:['Working behavior','Executable checks','Usage documentation'],criteria:[0],validation:['Required executable checks and independent review']}]}}});
 await client().command('plan_decide',{target:plan.id,input:{requestId:randomUUID(),revision:refined.revision,sourceId:source.id,decision:'accept'}});
 await client().request(`/api/queue/${f.run.id}/remove`,{requestId:randomUUID()});Object.assign(f.run,await client().request(`/api/tasks/${task.id}/runs`,{revision:refined.revision,requestId:randomUUID()}));
 const ui=(action:string,extra:any={})=>nativeUi(context,f.root,{descriptor:join(f.config.authority,'native-connection.json'),taskId:task.id,runId:f.run.id,action,...extra});
 await ui('queue');
 const runtime=()=>f.snapshot();const record=async()=>{
  const value=await client().command('record',{target:f.run.id});
  for(const field of ['edits','claims','completions','candidates','checks','reviewAssignments','reviews'])value[field]??=[];
  return value;
 };
 const state=async()=>{const r=await runtime();if(r.run.status==='attention_required')throw new Error(r.run.attentionReason??JSON.stringify(r.run));return r;};
 const idle=()=>until(async()=>{const r=await state();return !r.busy&&r.assignments.length&&r.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>r.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?r:false;},180000);
 const current=async(name:string)=>(await state()).assignments.find((a:any)=>a.name===name&&!a.retiredAt)?.currentAttemptId;
 async function ask(name:string,instruction:string,done:(r:any)=>boolean|Promise<boolean>,fault?:'mail'|'checkpoint'){
  await idle();const who=await current(name);assert.ok(who,`Missing current ${name}`);const key=randomUUID();console.log('STEP '+name+': '+instruction.slice(0,120));
  if(fault==='mail')mailGate=new Promise<void>(resolve=>{releaseMail=resolve;});
  const observedRevision=(await state()).choreography?.revision;
  const revisionHint=observedRevision===undefined?'':` At dispatch the current choreography revision is ${observedRevision}. Before any mutation read choreography_status and use its current choreography.revision, never a revision nested in a historical source/report/exchange.`;
  await f.input(who,{instruction:'STEP: '+instruction+revisionHint},key);
  if(fault==='mail'){
   const pending=await until(async()=>{const r=await state();return r.messages.find((m:any)=>m.disposition!=='received')??false;},180000);
   report.pendingExchangeBeforeRestart=pending;await until(async()=>{const r=await state();return r.attempts.find((a:any)=>a.id===who)?.state==='idle';},180000);await f.kill();releaseMail?.();mailGate=undefined;await f.reopen();await idle();
   await until(async()=>(await state()).messages.find((m:any)=>m.id===pending.id)?.disposition==='received',180000);
   assert.equal((await state()).messages.filter((m:any)=>m.id===pending.id).length,1);
  }
  if(fault==='checkpoint'){
   const pending=await until(async()=>{const r=await record();return r.completions.find((c:any)=>c.mode==='checkpoint'&&c.state==='requested')??false;},180000);
   report.transferBeforeRestart=pending;await f.kill();await f.reopen();await idle();
  }
  await until(async()=>done(await state()),180000);await idle();report.steps.push({name,instruction,inputId:key,attemptId:who,at:new Date().toISOString()});persist();
 }
 const member=(name:string,roleId:string,responsibility:string,stage:number,writes=false)=>({name,roleId,responsibility,stage,writes,packages:['fixture'],rationale:'Accepted '+setup.job+' '+responsibility+' responsibility',input:common});
 const members=[member('Writer','implementer','writer',0,true),member('Navigator','navigator','navigator',0),...(setup.job==='ui'?[member('Interface','ui-specialist','ui',0)]:[]),member('Docs','tech-writer','documentation',1,true),member('Review','reviewer','reviewer',2)];
 const schedule={uiRequired:setup.job==='ui',diagnosisReason:null,assignments:members};
 await ask('Coordinator','Call propose_choreography once with revision 0 and this schedule: '+JSON.stringify(schedule)+'. Then stop.',r=>!!r.choreography);
 async function team(names:string[],retire:string[]=[]){const proposal={specialists:names.map(name=>{const m=members.find(m=>m.name===name)!;return {name,roleId:m.roleId,input:m.input,dependsOn:[],criteria:[{index:0,obligation:requirement,checkIds:['required']}]};}),retire:retire.map(name=>({name,reason:'Contribution and delivered exchange reconciled'}))};await ask('Coordinator','Call propose_team with '+JSON.stringify(proposal)+'. Stop; Crewroom applies it when settled.',r=>names.every(name=>r.assignments.some((a:any)=>a.name===name&&!a.retiredAt))&&retire.every(name=>r.assignments.find((a:any)=>a.name===name)?.retiredAt));}
 await team(['Writer','Navigator',...(setup.job==='ui'?['Interface']:[])]);
 await ui('packages',{width:390,labels:['Planned · No process','Writer','Navigator','Docs']});
 await ask('Coordinator','Read choreography_status. Start package fixture using record_package kind=start, summary=Begin accepted pair work, artifacts=[]. Then claim_implementation for Writer with the accepted requirement as instruction. Stop.',r=>r.packageWriter?.attemptId);
 await ask('Writer','Inspect the source and requirements, then send a structured pair_exchange kind=approach to Navigator for package fixture using the current choreography revision. Explain your intended algorithm/design, source artifact and concrete validation proposal. Do not edit yet. Stop.',r=>r.choreography.exchanges.some((e:any)=>e.kind==='approach'),'mail');
 await ask('Writer','Implement the accepted '+setup.job+' requirement using repository_read and write_file. Add meaningful executable tests (backend: check.mjs). '+(setup.job==='ui'?'Keep #empty as a single persistent live region (role=status, aria-live=polite) with initial silence and dynamically updated text. Avoid duplicate exact empty-state messages. Read and run the unchanged real browser check using run_shell command="/usr/local/bin/node /opt/check-ui.cjs"; fix actual failures without weakening checks. ':'')+'Do not freeze or checkpoint. End with a brief summary.',async()=>{const r=await record();return (r.edits??[]).some((e:any)=>e.state==='applied');});
 await ask('Navigator','Independently read the changed source and tests. Find a concrete missed edge case, counterexample or UI interaction/test gap. Use pair_exchange kind=challenge to Writer with source artifact, exact concern and proposed regression/change, using the current choreography revision. Avoid empty praise; challenge something testable. Wait for approach receipt if needed. Stop.',r=>r.choreography.exchanges.some((e:any)=>e.kind==='challenge'));
 await ask('Writer','Read the actual navigator challenge in choreography_status and inspect the source. Address it with a concrete patch/test or evidence-backed disagreement. For the UI job run the unchanged browser check with run_shell command="/usr/local/bin/node /opt/check-ui.cjs", inspect output, and repair any failure before responding. Send pair_exchange kind=response to Navigator referencing the challenge ID and source artifact, explaining the resulting change and test. Do not freeze. Stop.',r=>r.choreography.exchanges.some((e:any)=>e.kind==='response'));
 // Interrupt with an actual writer lease; the native Resume must restore only one writer.
 const before=await state();report.beforeWriterPause=before.packageWriter;await ui('pause-pair');assert.ok((await runtime()).attempts.every((a:any)=>a.ownership==='stopped'));await ui('resume');await idle();
 report.afterWriterResume=(await state()).packageWriter;assert.ok(report.afterWriterResume?.attemptId);
 await ask('Coordinator','Inspect the navigator challenge and writer response, including referenced source. If reconciled, send pair_exchange kind=resolution to Navigator for fixture referencing both challengeId and responseId with concrete disposition and artifact. Do not close the package yet. Stop.',r=>r.choreography.exchanges.some((e:any)=>e.kind==='resolution'));
 for(const name of ['Writer','Navigator',...(setup.job==='ui'?['Interface']:[])])await ask(name,'Inspect current source and the resolved challenge. '+(name==='Interface'?'Run a real browser check using shell and node /opt/check-ui.cjs, and report actual results. ':'')+'Record your package fixture contribution using record_package kind=report with a meaningful summary and artifacts from repository_read path/sha256. Do not complete the package. Stop.',r=>r.choreography.reports.some((p:any)=>p.name===name));
 await ask('Writer','Call checkpoint_implementation once with summary describing the completed source contribution and successor Docs. End your turn immediately; this must not create a candidate.',async()=>{const r=await record();return r.completions.some((c:any)=>c.mode==='checkpoint'&&c.state==='completed');},'checkpoint');
 const checkpoint=await record();assert.equal(checkpoint.candidates.length,0);report.checkpoint=checkpoint.completions.at(-1);await f.kill();await f.reopen();await idle();assert.equal((await record()).candidates.length,0);
 await team(['Docs'],['Writer','Navigator',...(setup.job==='ui'?['Interface']:[])]);
 await ask('Coordinator','Claim implementation for Docs to document the completed code. Do not edit yourself. Stop.',r=>r.packageWriter?.attemptId);
 await ask('Docs','Read the implemented source and update README.md to accurately explain behavior, edge cases and how to run checks. Use write_file under your writer claim. Then record_package kind=report for fixture with artifacts from repository_read and documentation {disposition:"changed",reason:<specific documentation impact>}. Do not freeze yet. Stop.',r=>r.choreography.reports.some((p:any)=>p.name==='Docs'));
 await ask('Coordinator','Read choreography_status and all reports. Complete package fixture with record_package kind=complete, meaningful summary and artifacts=[]. Stop.',r=>r.choreography.packages[0].state==='complete');
 await ask('Docs','Call finish_implementation with a concise summary of the code and documentation change. End your turn immediately and let Crewroom stop and freeze the exact clean source.',async()=>(await record()).candidates.length===1);
 const frozen=(await record()).candidates[0];report.candidate=frozen;await f.kill();await f.reopen();await idle();assert.deepEqual((await record()).candidates,[frozen]);
 await team(['Review'],['Docs']);
 await ask('Coordinator','Assign independent review to Review for candidate '+frozen.id+' with instruction to inspect exact source, check edge cases, documentation and criterion 0. Stop.',async()=>((await record()).reviewAssignments??[]).length>0);
 report.beforeReviewPause={assignments:(await record()).reviewAssignments,reviews:(await record()).reviews,candidateId:frozen.id};
 await ui('pause-pair');await ui('resume');await idle();assert.deepEqual((await record()).candidates,[frozen]);
 await ask('Review','Independently inspect source, tests and README for candidate '+frozen.id+'. Run the required check with run_check checkId=required. Submit_review for that exact candidate using actual verdict, concrete findings and coverage for criterion 0 (checkIds:["required"]). Pair collaboration is not independent review. Stop.',async()=>(await record()).reviews.length>0);
 const final=await record();report.assessment=await client().command('evidence',{target:f.run.id});assert.equal(report.assessment.ready,true,JSON.stringify(report.assessment));
 const finalRuntime=await state();report.assignments=finalRuntime.assignments;report.attempts=finalRuntime.attempts.map((a:any)=>({id:a.id,assignmentId:a.assignmentId,runnerKind:a.runnerKind??'omp',hostPid:a.hostPid,runnerPid:a.runnerPid??a.ompPid,sessionId:a.sessionId,model:a.model,observedModel:a.observedModel,ownership:a.ownership}));report.choreography=finalRuntime.choreography;report.messages=finalRuntime.messages;report.checks=final.checks;report.reviews=final.reviews;
 assert.ok(report.attempts.some((a:any)=>a.runnerKind==='omp'));assert.ok(report.attempts.some((a:any)=>a.runnerKind==='codex'));assert.ok(report.attempts.some((a:any)=>a.runnerKind==='claude-code'&&JSON.stringify(a.observedModel).includes('opus')));
 assert.ok(finalRuntime.messages.every((m:any)=>m.disposition==='received'));assert.equal(new Set(finalRuntime.messages.map((m:any)=>m.id)).size,finalRuntime.messages.length);
 for(const width of [1280,390])await ui('packages',{width,labels:['Transferred · Successor claim recorded','Documentation: changed','Delivery: received']});
 const events=await client().command('events',{target:f.run.id});writeFileSync(join(setup.root,'agent-events.json'),JSON.stringify(events,null,2)+'\n',{mode:0o600});report.eventsSha256=createHash('sha256').update(JSON.stringify(events)).digest('hex');
 await f.pause();report.passed=true;
}catch(error){report.error=error instanceof Error?error.stack:String(error);}
finally{await finish();}
if(!report.passed)process.exitCode=1;
