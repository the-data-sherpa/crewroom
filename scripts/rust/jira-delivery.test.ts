import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {test} from 'node:test';
import {body,issue,jiraFixture,source,waitFor} from './jira-workflow-fixture';
const report={kind:'milestone',eventKey:'impact-reviewed',body:'The modal overflow cause is identified.'};
const page=(comments:any[])=>JSON.stringify({comments,startAt:0,total:comments.length});

test('native Jira intents survive outage and lost create reply without duplicate comments',async t=>{
 let offline=true,posts=0;const comments:any[]=[];
 const f=await jiraFixture(t,async(req,res)=>{if(offline){res.statusCode=503;res.end('private detail');return;}if(req.method==='POST'){comments.push({...await body(req),id:'77'});posts++;req.socket.destroy();return;}res.end(page(comments));});
 const action=await f.report(report);assert.equal(action.state,'pending');assert.equal(posts,0);assert.deepEqual(await f.report({...report,body:'  '+report.body+'  '}),action);
 await assert.rejects(f.report({...report,body:'Different'}),/different content/);await assert.rejects(f.report({...report,description:'Rewrite Jira'}),/Unrecognized/);
 await f.flush();assert.equal((await f.updates())[0].state,'pending');assert.equal(posts,0);
 offline=false;await f.flush();assert.equal((await f.updates())[0].state,'uncertain');assert.equal(posts,1);
 await f.reopen();await f.flush();await f.flush();assert.equal((await f.updates())[0].state,'confirmed');assert.equal((await f.updates())[0].externalId,'77');assert.equal(posts,1);
 const task=await f.c.command('task',{target:f.seed.task.id});assert.equal(task.description,'Keep buttons visible');assert.equal(task.jiraStatus,'To Do');assert.equal(JSON.stringify(await f.updates()).includes('private detail'),false);
});

test('native unknown comment outcome holds its agent lane even after inconclusive reads',async t=>{
 let posts=0;const f=await jiraFixture(t,(req,res)=>{if(req.method==='POST'){posts++;req.socket.destroy();}else res.end(page([]));});
 await f.report(report);await f.report({...report,eventKey:'later'});await f.flush();await f.flush();await f.flush();
 assert.deepEqual((await f.updates()).map((u:any)=>u.state),['uncertain','pending']);assert.equal(posts,1);
});

test('native reporter authority and configured transitions do not invent imported Jira status',async t=>{
 let status='1',posts=0,denied=false;
 const f=await jiraFixture(t,async(req,res)=>{if(denied){res.statusCode=403;res.end('private permissions');return;}if(req.method==='POST'){assert.deepEqual(await body(req),{transition:{id:'6'}});status='3';posts++;req.socket.destroy();return;}res.end(JSON.stringify(req.url!.includes('transitions')?{transitions:[{id:'6',to:{id:'3'},fields:{}}]}:issue('100','Fix scrolling',status)));},{evidence:false});
 await assert.rejects(f.report({kind:'in_progress',eventKey:'execution-started'},f.seed.writer.attempt.id),/reporter/);
 await assert.rejects(f.report({kind:'in_progress',eventKey:'execution-started',transitionId:'2'}),/Unrecognized/);
 await assert.rejects(f.report({kind:'done',eventKey:'close'}),/Invalid/);
 await assert.rejects(f.report({kind:'evidence',eventKey:'proof',candidateId:f.seed.run.id}),/evidence is unavailable/);
 await f.report({kind:'in_progress',eventKey:'execution-started'});await f.flush();assert.equal((await f.updates())[0].state,'uncertain');await f.flush();assert.equal((await f.updates())[0].state,'confirmed');assert.equal(posts,1);
 assert.equal((await f.c.command('task',{target:f.seed.task.id})).jiraStatus,'To Do');
 denied=true;await f.report({...report,kind:'blocker'});await f.flush();const blocked=(await f.updates())[1];assert.equal(blocked.state,'blocked');assert.match(blocked.reason,/403/);assert.equal(JSON.stringify(blocked).includes('private permissions'),false);
});

test('native credential failure before dispatch retains a retryable intent',async t=>{
 let posts=0;const f=await jiraFixture(t,(req,res)=>{if(req.method==='POST'){posts++;res.end(JSON.stringify({id:'88'}));}else res.end(page([]));});
 f.rules({failAt:2});await f.report(report);await f.flush();assert.equal((await f.updates())[0].state,'pending');assert.equal(posts,0);
 f.rules({});await f.flush();assert.equal((await f.updates())[0].state,'confirmed');assert.equal(posts,1);
});

test('native final authorization blocks pause, cancellation and close during credentials',{timeout:60000},async t=>{
 for(const mode of ['pause','cancel','close'])await t.test(mode,async t=>{
  let posts=0,gets=0;const f=await jiraFixture(t,(req,res)=>{if(req.method==='POST'){posts++;res.end(JSON.stringify({id:'1'}));}else{gets++;res.end(page([]));}});
  f.rules({holdAt:2});await f.report(report);const pending=f.flush();await f.entered(2);
  let closing:Promise<any>|undefined;
  if(mode==='close'){closing=f.c.command('jira_delivery_close');await assert.rejects(f.report({...report,eventKey:'late'}),/stopping/);}
  else await f.c.command('desired',{target:f.seed.run.id,input:mode==='pause'?'paused':'cancelled'});
  assert.ok((await f.snapshot()).runs.length);f.release(2);await pending;await closing;
  assert.equal(posts,0);assert.equal((await f.updates())[0].state,mode==='cancel'?'blocked':'pending');
  if(mode==='close'){await f.flush();assert.equal(gets,1);await f.c.command('jira_delivery_close');}
 });
});

test('native concurrent flushes share driven work after one waiter is cancelled',async t=>{
 let hold:any,posts=0,gets=0;const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){posts++;res.end(JSON.stringify({id:'shared'}));}else{gets++;hold=res;}});
 await f.report(report);const id=randomUUID();const first=f.flush(id).then(()=>({ok:true}),e=>({error:String(e)}));await waitFor(()=>!!hold);
 const rest=Array.from({length:8},()=>f.flush());await f.c.command('cancel_waiter',{requestId:id});assert.match((await first as any).error,/cancelled/);
 hold.end(page([]));await Promise.all(rest);assert.equal(gets,1);assert.equal(posts,1);assert.equal((await f.updates())[0].state,'confirmed');
});

function publication(f:any){return {id:randomUUID(),runId:f.seed.run.id,attemptId:f.seed.reporter.attempt.id,candidateId:randomUUID(),push:{state:'confirmed',expectedHead:null},pr:{state:'pending'},ready:false,blockers:[]};}
// These are publication policy fixtures; actual PR acquisition is N5.3b.
test('native confirmed PR and verified merge derive independent review and close intents once',async t=>{
 let status='3';const writes:string[]=[];
 const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){const id=(await body(req)).transition?.id;if(id){writes.push(id);status=({'8':'10004','2':'10003','6':'3'} as any)[id];res.statusCode=204;res.end();}else res.end(JSON.stringify({id:'comment'}));return;}if(req.url!.includes('/comment')){res.end(page([]));return;}res.end(JSON.stringify(req.url!.includes('transitions')?{transitions:[{id:'8',to:{id:'10004'},fields:{}},{id:'2',to:{id:'10003'},fields:{}},{id:'6',to:{id:'3'},fields:{}}]}:issue('100','Fix scrolling',status)));});
 const p=publication(f);await f.c.command('save_publication',{input:p});await f.flush();assert.deepEqual(writes,[]);
 await f.c.command('save_publication',{input:{...p,pr:{state:'confirmed',id:9,providerState:'DECLINED'}}});await f.flush();assert.deepEqual(writes,[]);
 const open={...p,pr:{state:'confirmed',id:9,providerState:'OPEN'}};await f.c.command('save_publication',{input:open});await f.flush();await f.flush();assert.deepEqual(writes,['8']);
 await f.report(report);await f.report({kind:'in_progress',eventKey:'execution-started'});await f.c.command('desired',{target:f.seed.run.id,input:'paused'});
 await f.c.command('save_publication',{input:{...open,merged:{commit:'c'.repeat(40),observedAt:new Date().toISOString()}}});await f.flush();await f.flush();assert.deepEqual(writes,['8','2']);
 const updates=await f.updates();assert.equal(updates.find((u:any)=>u.kind==='done').state,'confirmed');assert.equal(updates.find((u:any)=>u.kind==='in_progress').reason,'Superseded by the merge.');assert.equal(updates.find((u:any)=>u.kind==='milestone').state,'pending');
 await f.c.command('desired',{target:f.seed.run.id,input:'running'});await f.flush();assert.deepEqual(writes,['8','2']);
});

test('native unknown review transition never repeats and does not hold close after merge',async t=>{
 let status='3';const writes:string[]=[];
 const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){const id=(await body(req)).transition.id;writes.push(id);if(id==='8'){req.socket.destroy();return;}status='10003';res.statusCode=204;res.end();return;}res.end(JSON.stringify(req.url!.includes('transitions')?{transitions:[{id:'8',to:{id:'10004'},fields:{}},{id:'2',to:{id:'10003'},fields:{}}]}:issue('100','Fix scrolling',status)));});
 const p={...publication(f),pr:{state:'confirmed',id:9,providerState:'OPEN'}};await f.c.command('save_publication',{input:p});await f.flush();await f.flush();assert.deepEqual(writes,['8']);assert.equal((await f.updates())[0].state,'uncertain');
 await f.c.command('save_publication',{input:{...p,merged:{commit:'c'.repeat(40),observedAt:new Date().toISOString()}}});await f.flush();await f.flush();assert.deepEqual(writes,['8','2']);assert.match((await f.updates())[0].reason,/outcome is unknown/);
});

test('actual native controller SIGKILL before dispatch and after remote effect reconciles without retry',{timeout:60000},async t=>{
 for(const afterEffect of [false,true])await t.test(afterEffect?'after effect':'before dispatch',async t=>{
  let posts=0;const comments:any[]=[];
  const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){posts++;comments.push({id:'killed-receipt',body:(await body(req)).body});return;}res.end(page(comments));});
  if(!afterEffect)f.rules({holdAt:2});await f.report(report);const pending=f.flush().catch(e=>String(e));
  if(afterEffect)await waitFor(()=>comments.length===1);else await f.entered(2);
  assert.equal((await f.updates())[0].state,'sending');f.c.proc.kill('SIGKILL');await f.c.exit;await pending;await f.reopen();await f.flush();await f.flush();
  assert.equal(posts,afterEffect?1:0);assert.equal((await f.updates())[0].state,afterEffect?'confirmed':'uncertain');
 });
});

test('native mutation HTTP refusals and accepted malformed replies retain distinct retry decisions',async t=>{
 for(const [status,response,expected]of [[429,{},'pending'],[403,{},'blocked'],[201,{id:null},'uncertain']]as const)await t.test(String(status),async t=>{
  let posts=0;const f=await jiraFixture(t,(req,res)=>{if(req.method==='POST'){posts++;res.statusCode=status;res.end(JSON.stringify(response));}else res.end(page([]));});
  await f.report(report);await f.flush();assert.equal((await f.updates())[0].state,expected);assert.equal(posts,1);if(expected!=='pending'){await f.flush();assert.equal(posts,1);}
 });
});

test('native marker conflicts and configured-source changes block delivery without mutations',async t=>{
 let posts=0,remote:any[]=[];const f=await jiraFixture(t,(req,res)=>{if(req.method==='POST'){posts++;res.end('{}');}else res.end(page(remote));});
 const action=await f.report(report);remote=[{id:'foreign',body:`Conflicting content\nCrewroom update ${action.id}`}];await f.flush();assert.equal((await f.updates())[0].state,'blocked');assert.match((await f.updates())[0].reason,/conflicting content/);assert.equal(posts,0);
 await f.report({...report,eventKey:'separate'});await f.reopen({jira:{...f.config.jira,source:{...source,siteUrl:'https://different.atlassian.net'}}});await f.flush();
 // An earlier blocked agent update holds its lane. Use its first intent in a fresh fixture for source refusal.
 const other=await jiraFixture(t,(_req,res)=>{res.end(page([]));});await other.report(report);await other.reopen({jira:{...other.config.jira,source:{...source,siteUrl:'https://different.atlassian.net'}}});await other.flush();assert.equal((await other.updates())[0].reason,'The configured Jira site changed.');assert.equal(await other.c.command('jira_credentials'),0);
});

test('native transitions reject a wrong target or required fields before dispatch',async t=>{
 for(const transition of [{id:'6',to:{id:'other'},fields:{}},{id:'6',to:{id:'3'},fields:{resolution:{required:true}}}])await t.test(JSON.stringify(transition),async t=>{
  let posts=0;const f=await jiraFixture(t,(req,res)=>{if(req.method==='POST'){posts++;res.end('{}');return;}res.end(JSON.stringify(req.url!.includes('transitions')?{transitions:[transition]}:issue()));});
  await f.report({kind:'in_progress',eventKey:'execution-started'});await f.flush();assert.equal((await f.updates())[0].state,'blocked');assert.match((await f.updates())[0].reason,/unavailable or requires/);assert.equal(posts,0);
 });
});

test('merge arriving during review credentials prevents the stale transition and permits only close',async t=>{
 let status='3';const writes:string[]=[];
 const f=await jiraFixture(t,async(req,res)=>{if(req.method==='POST'){const id=(await body(req)).transition.id;writes.push(id);status='10003';res.statusCode=204;res.end();return;}res.end(JSON.stringify(req.url!.includes('transitions')?{transitions:[{id:'8',to:{id:'10004'},fields:{}},{id:'2',to:{id:'10003'},fields:{}}]}:issue('100','Fix scrolling',status)));});
 const p={...publication(f),pr:{state:'confirmed',id:9,providerState:'OPEN'}};await f.c.command('save_publication',{input:p});f.rules({holdAt:3});const pending=f.flush();await f.entered(3);
 await f.c.command('save_publication',{input:{...p,merged:{commit:'c'.repeat(40),observedAt:new Date().toISOString()}}});f.release(3);await pending;assert.deepEqual(writes,[]);assert.equal((await f.updates())[0].state,'blocked');await f.flush();assert.deepEqual(writes,['2']);
});

test('native report boundaries normalize whitespace and enforce Unicode, event and field limits',async t=>{
 let requests=0;const f=await jiraFixture(t,(_req,res)=>{requests++;res.end(page([]));});
 const valid=await f.report({...report,body:'\uFEFF\u00a0'+report.body+'\n'});assert.equal(valid.body,report.body);
 await f.report({...report,eventKey:'a'.repeat(200),body:'😀'.repeat(12000)});
 for(const input of [{...report,eventKey:'a'.repeat(201)},{...report,eventKey:'_no'},{...report,eventKey:'a/b'},{...report,eventKey:'a\n'},{...report,body:'😀'.repeat(12001)},{...report,body:'\uFEFF\u00a0'},{...report,body:null},{...report,unknown:true},{kind:'in_progress',eventKey:'wrong'},{kind:'evidence',eventKey:'x',candidateId:'not-a-uuid'},null,[]])await assert.rejects(f.report(input));
 assert.equal(requests,0);assert.equal((await f.updates()).length,2);
});
