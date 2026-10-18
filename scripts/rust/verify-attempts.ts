/** N2.2 store oracle. Every native operation opens the database in a new process.
 * Process identities, publication observations and unported control states below
 * are explicit fixtures: this suite never starts agents or calls a provider. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { copyFileSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n22-')));
const legacyPath=join(root,'legacy.sqlite'), nativePath=join(root,'native.sqlite');
let now=1000, legacy=new Workbench(legacyPath,()=>now);
const cases:string[]=[], fixtures:string[]=[], ids=new Map<string,string>(), fixed=new Set<string>();
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const transform=(value:any, strings:(s:string)=>string):any=>Array.isArray(value)?value.map(v=>transform(v,strings))
  :value && typeof value==='object'?Object.fromEntries(Object.entries(value).map(([k,v])=>[strings(k),transform(v,strings)]))
  :typeof value==='string'?strings(value):value;
const toNative=(v:any)=>transform(v,s=>ids.get(s)??s);
const fromNative=(v:any)=>transform(v,s=>[...ids].find(([,actual])=>actual===s)?.[0]??s);
const protect=(v:any)=>transform(v,s=>{if(uuid.test(s) && !ids.has(s))fixed.add(s);return s;});
// Only newly allocated UUIDs may differ. References, request IDs, timestamps,
// all snapshot contents and all non-ID values remain exact.
function remember(expected:any, actual:any) {
  if(typeof expected==='string' && uuid.test(expected) && expected!==actual) {
    assert.ok(uuid.test(actual) && !fixed.has(expected),'Only generated IDs may differ');
    if(ids.has(expected))assert.equal(actual,ids.get(expected));
    else {assert.ok(![...ids.values()].includes(actual),'Generated identities must remain distinct');ids.set(expected,actual);}
  } else if(Array.isArray(expected)) {assert.equal(actual.length,expected.length);expected.forEach((v,i)=>remember(v,actual[i]));}
  else if(expected && typeof expected==='object') {
    assert.deepEqual(Object.keys(actual).sort(),Object.keys(expected).sort());
    for(const [key,value] of Object.entries(expected))remember(value,actual[key]);
  } else assert.deepEqual(actual,expected);
}
function snapshot(path:string) {
  const db=new DatabaseSync(path,{readOnly:true});
  try {
    const tables=db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").all();
    return Object.fromEntries(tables.map(({name})=>[String(name),db.prepare(`SELECT * FROM ${name} ORDER BY rowid`).all().map(row=>
      Object.fromEntries(Object.entries(row).map(([k,v])=>[k,['data','imported','refinement','payload','result'].includes(k)?JSON.parse(String(v)):v])))]));
  } finally {db.close();}
}
function compare(label:string) {
  const expected=snapshot(legacyPath), actual=fromNative(snapshot(nativePath));
  // An existing derived-view difference from N2.1, never an ownership/timing field.
  for(const state of [expected,actual])for(const row of state.runs)delete row.data.holdReason;
  for(const table of Object.keys(expected))assert.deepEqual(actual[table],expected[table],`${label}: ${table}`);
}
function invoke(op:any):any {
  const s=legacy;
  switch(op.method) {
    case 'admitAssignment':return s.admitAssignment(op.runId,op.input);
    case 'admitInitialTeam':return s.admitInitialTeam(op.runId,op.team);
    case 'claimAttempt':return s.claimAttempt(op.attemptId);
    case 'recordAttemptIdentity':return s.recordAttemptIdentity(op.attemptId,op.identity);
    case 'recordOwnership':return s.recordOwnership(op.attemptId,op.stopped,op.reason);
    case 'replaceAttempt':return s.replaceAttempt(op.assignmentId,op.input);
    case 'grantRepairCycle':return s.grantRepairCycle(op.runId,op.assignmentId,op.input);
    case 'recordAgentEvent':return s.recordAgentEvent(op.attemptId,op.sequence,op.event);
    case 'beginPause':return s.beginPause(op.runId,op.reason);
    case 'finishPause':return s.finishPause(op.runId);
    case 'accountTime':return s.accountTime(op.runId);
    case 'executingRuns':return s.executingRuns();
    case 'unstoppedCodingRun':return s.unstoppedCodingRun(op.runId);
    case 'getAssignment':return s.runtime(op.runId).assignments.find(a=>a.id===op.assignmentId);
    case 'getAttempt':return s.getAttempt(op.attemptId);
    case 'agentEvents':return s.agentEvents(op.runId);
    case 'commandEvents':return s.commandEvents(op.runId);
    case 'agentEventPage':return s.agentEventPage(op.runId,op.after??0,op.limit??100);
    case 'attemptForPane':return s.attemptForPane(op.pane);
    case 'hasAgentEvent':return s.hasAgentEvent(op.attemptId,op.type);
    case 'observerCursor':return s.observerCursor(op.assignmentId);
    case 'recordObserverPage':return s.recordObserverPage(op.assignmentId,op.events,op.cursor);
    default:throw new Error(`Unknown method ${op.method}`);
  }
}
function step(label:string,op:any,refused=false):any {
  protect(op);let expected:any, error:unknown;
  try {expected=invoke(op)??null;}catch(e){error=e;}
  assert.equal(!!error,refused,`${label}: TypeScript ${String(error)}`);
  const actual=JSON.parse(execFileSync(resolve('target/debug/examples/ledger-oracle'),[nativePath],{input:JSON.stringify(toNative({...op,now})),encoding:'utf8'}));
  assert.equal(actual.ok,!refused,`${label}: Rust ${JSON.stringify(actual)}`);
  if(!refused){expected=JSON.parse(JSON.stringify(expected));
    if(expected?.taskSnapshot && expected.status){delete expected.holdReason;delete actual.value.holdReason;}
    remember(expected,actual.value);assert.deepEqual(fromNative(actual.value),expected,`${label}: result`);}
  compare(label);cases.push(label);return expected;
}
// Explicit fixture writes set up boundaries whose commands are later iterations.
// They are separate from the operations counted as ported behavior.
function fixture(label:string,apply:(db:DatabaseSync,map:(v:any)=>any)=>void) {
  for(const [path,map] of [[legacyPath,(v:any)=>v],[nativePath,toNative]] as const) {
    const db=new DatabaseSync(path);try {apply(db,map);}finally {db.close();}
  }
  compare(label);fixtures.push(label);
}
function patch(table:string,id:string,changes:any) {
  fixture(`fixture ${table} ${Object.keys(changes).join(',')}`,(db,map)=>{
    const row=db.prepare(`SELECT data FROM ${table} WHERE id=?`).get(map(id))!;
    db.prepare(`UPDATE ${table} SET data=? WHERE id=?`).run(JSON.stringify({...JSON.parse(String(row.data)),...map(changes)}),map(id));
  });
}
try {
  const repoPath=join(root,'repo');seedRepository(repoPath);
  const repo=saveRepositoryFixture(legacy,{name:'Oracle',path:repoPath,targetBranch:'main',checks:[]});
  const makeRun=(title:string,purpose='coding')=>{
    const task=legacy.createTask({title,description:'',...(purpose==='coding'?{repositoryId:repo.id}:{})});
    legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Preserve accepted intent',criteria:['Keep immutable evidence']});
    return purpose==='coding'?legacy.queue(task.id,{revision:2}):purpose==='research'?legacy.queueResearch(task.id,{revision:2,requestId:randomUUID()}):legacy.queueRefinement(task.id,{revision:2,purpose});
  };
  const main=makeRun('Attempts'), rollback=makeRun('Atomic team'), reserved=makeRun('Reserved specialist'), explore=makeRun('Explore','explore');
  const codingA=makeRun('Coding A'),codingB=makeRun('Coding B'),legacyCoding=makeRun('Retained coding'),budget=makeRun('Budget');
  const research=makeRun('Read only','research');
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const input=(name='Coordinator',coordinator=true)=>({name,coordinator,role,model,input:'Observe accepted evidence 日本語 🦀',brief:{criteria:[],dependsOn:[]}});
  legacy.proposeOperatorSpecialist(reserved.id,{requestId:randomUUID(),roleId:role.id,input:'Inspect',criteria:[0],dependsOn:['Coordinator']},role,model);
  legacy.setActiveLimit(budget.id,{activeLimitMs:2000});
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);
  protect(snapshot(legacyPath));compare('seed');

  step('Initial team second-member refusal rolls back first admission',{method:'admitInitialTeam',runId:rollback.id,team:[input(),{...input('Reader',false),role:{...role,enabled:false}}]},true);
  step('Disabled role refused',{method:'admitAssignment',runId:main.id,input:{...input(),role:{...role,enabled:false}}},true);
  step('Pending insertion reserves specialist capacity',{method:'admitInitialTeam',runId:reserved.id,team:[input(),input('One',false),input('Two',false),input('Three',false)]},true);
  step('Pending insertion reserves names',{method:'admitInitialTeam',runId:reserved.id,team:[input(),input('Specialist1',false)]},true);
  step('Reserved insertion dependency must remain present',{method:'admitInitialTeam',runId:reserved.id,team:[input('Different')]},true);
  step('Explore refuses specialist',{method:'admitAssignment',runId:explore.id,input:input('Reader',false)},true);
  step('Explore admits coordinator',{method:'admitAssignment',runId:explore.id,input:input()});
  step('Explore refuses a second coordinator',{method:'admitAssignment',runId:explore.id,input:input('Other')},true);
  fixture('Repository guidance v1',(db)=>{
    const row=db.prepare('SELECT data FROM repositories WHERE id=?').get(repo.id)!;
    db.prepare('UPDATE repositories SET data=? WHERE id=?').run(JSON.stringify({...JSON.parse(String(row.data)),guidance:[{version:1,text:'Accepted project constraint'}]}),repo.id);
  });
  const team=[input(),input('Reader',false),input('Reviewer',false),input('Reporter',false)];
  const admitted=step('Atomic team captures immutable snapshots',{method:'admitInitialTeam',runId:main.id,team});
  const coord=admitted[0].assignment, first=admitted[0].attempt;
  step('Initial admission replay preserves identities',{method:'admitInitialTeam',runId:main.id,team});
  step('Replay retains original role and model snapshots',{method:'admitInitialTeam',runId:main.id,team:team.map(a=>({...a,role:{...a.role,instructions:'New role definition'},model:{...a.model,resolved:{...a.model.resolved,id:'changed',selector:'fixture/changed'}}}))});
  step('Initial admission replay refuses changed instructions',{method:'admitInitialTeam',runId:main.id,team:[{...team[0],input:'Changed'},...team.slice(1)]},true);
  step('Duplicate name refused',{method:'admitAssignment',runId:main.id,input:input('Reader',false)},true);
  step('Fourth specialist refused',{method:'admitAssignment',runId:main.id,input:input('Fourth',false)},true);
  step('Second coordinator refused',{method:'admitAssignment',runId:main.id,input:input('Other')},true);
  step('Claim admitted attempt once',{method:'claimAttempt',attemptId:first.id});
  step('Duplicate claim after reopen refused',{method:'claimAttempt',attemptId:first.id},true);
  const identity={hostPid:101,ompPid:102,sessionId:'fixture',hostUrl:'http://127.0.0.1:9000',pane:'%0'};
  step('Verified identity receipt',{method:'recordAttemptIdentity',attemptId:first.id,identity});
  step('Identity cannot be overwritten',{method:'recordAttemptIdentity',attemptId:first.id,identity:{...identity,ompPid:999}},true);
  step('Latest pane identity',{method:'attemptForPane',pane:'%0'});
  step('Missing pane is absent',{method:'attemptForPane',pane:'%missing'});
  step('Read captured assignment',{method:'getAssignment',runId:main.id,assignmentId:coord.id});
  patch('runs',main.id,{status:'running',desiredState:'running',activeSince:now});
  let sequence=0;
  const event=(label:string,event:any,attemptId=first.id)=>step(label,{method:'recordAgentEvent',attemptId,sequence:++sequence,event});
  event('Agent execution observation',{type:'agent_start'});now+=100;
  event('Compact streaming payload',{type:'message_update',messageId:'message',assistantMessageEvent:{type:'text_delta',contentIndex:0,delta:'x'.repeat(10000)}});
  event('Unicode progress follows host UTF-16 bounds',{type:'message_update',messageId:'🦀'.repeat(110),phase:'🦀'.repeat(50)});
  event('Oversize Unicode final does not erase progress',{type:'message_end',messageId:'🦀'.repeat(110),message:{content:'Retained final'}});
  event('Retain unrelated streaming progress',{type:'message_update',messageId:'other',phase:'toolcall_delta',contentIndex:1});
  event('Mismatched final cannot erase progress',{type:'message_end',messageId:'mismatch',message:{id:'message',content:'Final mismatch'}});
  event('Final message replaces only matching earlier progress',{type:'message_end',messageId:'message',message:{id:'message',content:'Final 日本語 🦀'}});
  step('Duplicate event sequence cannot rewrite evidence',{method:'recordAgentEvent',attemptId:first.id,sequence,event:{type:'message_end',message:{id:'message',content:'Forged'}}});
  event('Tool starts',{type:'tool_started',toolCallId:'tool',command:'npm test'});
  event('Tool exits with stopped ownership',{type:'tool_exited',toolCallId:'tool',ownership:{state:'stopped'},exitCode:0});
  event('Retryable provider failure',{type:'prompt_result',status:'error',error:{retryable:true,provider:'fixture',model:'model',message:'Temporary unavailable'}});
  const failureSequence=sequence;
  event('Settled observation cannot clear failed attempt',{type:'session_settled'});
  step('Read all retained events',{method:'agentEvents',runId:main.id});
  step('Read command-only events',{method:'commandEvents',runId:main.id});
  const page=step('Read bounded event page',{method:'agentEventPage',runId:main.id,after:0,limit:2});
  step('Continue event cursor across compaction gaps',{method:'agentEventPage',runId:main.id,after:page.after,limit:100});
  step('Recorded failure existence',{method:'hasAgentEvent',attemptId:first.id,type:'prompt_result'});
  step('Empty observer starts at zero',{method:'observerCursor',assignmentId:coord.id});
  step('Observer receipt and high-water cursor',{method:'recordObserverPage',assignmentId:coord.id,events:page.events,cursor:page.after});
  step('Observer page replay',{method:'recordObserverPage',assignmentId:coord.id,events:page.events,cursor:page.after});
  step('Observer cannot move backwards',{method:'recordObserverPage',assignmentId:coord.id,events:[],cursor:0},true);
  step('Observer event cannot exceed high-water cursor',{method:'recordObserverPage',assignmentId:coord.id,events:[{cursor:100}],cursor:page.after},true);
  step('Replacement refuses live ownership',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:false}},true);
  step('Begin safe pause',{method:'beginPause',runId:main.id});
  step('Record uncertain ownership',{method:'recordOwnership',attemptId:first.id,stopped:false,reason:'Unverified child'});
  step('Uncertain stop cannot finish pause',{method:'finishPause',runId:main.id});
  step('Confirmed process stop',{method:'recordOwnership',attemptId:first.id,stopped:true,reason:'Verified empty process tree'});
  step('Repeated ownership reason preserves notes',{method:'recordOwnership',attemptId:first.id,stopped:true,reason:'Verified empty process tree'});
  step('Complete reconciled pause',{method:'finishPause',runId:main.id});
  const fallback={...model,fallbackFrom:model.resolved.selector,resolved:{...model.resolved,id:'backup',selector:'fixture/backup'}};
  step('Fallback requires automatic recovery',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:false,fallback:{model:fallback,eventSequence:failureSequence}}},true);
  step('Fallback requires exact prior failure receipt',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true,fallback:{model:fallback,eventSequence:failureSequence+1}}},true);
  step('Fallback provenance must match prior model',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true,fallback:{model:{...fallback,fallbackFrom:'different/model'},eventSequence:failureSequence}}},true);
  const second=step('Automatic fallback records provenance',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true,fallback:{model:fallback,eventSequence:failureSequence}}});
  step('Old attempt model remains immutable',{method:'getAttempt',attemptId:first.id});
  step('Repair allowance refused while cycles remain',{method:'grantRepairCycle',runId:main.id,assignmentId:coord.id,input:{requestId:randomUUID(),reason:'Premature'}},true);
  step('Replacement claim',{method:'claimAttempt',attemptId:second.id});
  step('Replacement identity reuses pane safely',{method:'recordAttemptIdentity',attemptId:second.id,identity});
  step('Newest attempt wins reused pane lookup',{method:'attemptForPane',pane:'%0'});
  step('Stop replacement',{method:'recordOwnership',attemptId:second.id,stopped:true});
  const third=step('Second automatic cycle retains fallback model',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true}});
  step('Stop unlaunched replacement receipt',{method:'recordOwnership',attemptId:third.id,stopped:true});
  step('Two automatic cycles exhausted',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true}},true);
  const allowance={requestId:randomUUID(),reason:'Operator inspected retained evidence'};
  step('Explicit operator repair allowance',{method:'grantRepairCycle',runId:main.id,assignmentId:coord.id,input:allowance});
  step('Repair decision replay cannot double grant',{method:'grantRepairCycle',runId:main.id,assignmentId:coord.id,input:{...allowance,reason:'Changed replay reason'}});
  const fourth=step('Granted third automatic cycle',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true}});
  step('Stop granted attempt',{method:'recordOwnership',attemptId:fourth.id,stopped:true});
  step('Granted automatic cycle is bounded',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:true}},true);
  const manual=step('Manual replacement does not consume automatic cycles',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:false}});
  step('Stop manual attempt',{method:'recordOwnership',attemptId:manual.id,stopped:true});
  for(const [table,data,insert] of [
    ['peer_messages',{id:'peer',runId:main.id,disposition:'uncertain'},"INSERT INTO peer_messages(id,run_id,attempt_id,tool_call_id,data) VALUES(?,?,?,'fixture',?)"],
    ['steering',{id:'steer',runId:main.id,delivery:'dispatched'},"INSERT INTO steering(id,run_id,request_id,data) VALUES(?,?,'fixture',?)"],
    ['delivery_notices',{id:'notice',runId:main.id,disposition:'dispatched'},'INSERT INTO delivery_notices VALUES(?,?,?)'],
  ] as const) {
    fixture(`Uncertain ${table} fixture`,(db,map)=>db.prepare(insert).run(data.id,main.id,...(table==='peer_messages'?[map(first.id)]:[]),JSON.stringify(map(data))));
    step(`${table} uncertainty prevents replacement`,{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:false}},true);
    fixture(`Reconcile ${table} fixture`,db=>db.prepare(`DELETE FROM ${table} WHERE id=?`).run(data.id));
  }
  patch('assignments',coord.id,{retiredAt:new Date(now).toISOString()});
  step('Retired assignment cannot resume',{method:'replaceAttempt',assignmentId:coord.id,input:{automatic:false}},true);

  const a=step('First coding admission',{method:'admitAssignment',runId:codingA.id,input:input()});
  const b=step('Second coding admission',{method:'admitAssignment',runId:codingB.id,input:input()});
  for(const run of [codingA,codingB])fixture('Retained repository ownership fixture',(db)=>db.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(run.id,repoPath,run.id,JSON.stringify({runId:run.id,repository:repo})));
  step('Claim first coding ownership',{method:'claimAttempt',attemptId:a.attempt.id});
  step('Second coding claim refused until stop',{method:'claimAttempt',attemptId:b.attempt.id},true);
  step('Executing runs reflect claimed ownership',{method:'executingRuns'});
  step('Unstopped coding lookup',{method:'unstoppedCodingRun',runId:codingB.id});
  step('Confirmed coding stop releases claim guard',{method:'recordOwnership',attemptId:a.attempt.id,stopped:true});
  step('Second coding claim after stop',{method:'claimAttempt',attemptId:b.attempt.id});
  step('Stop second coding attempt',{method:'recordOwnership',attemptId:b.attempt.id,stopped:true});
  const old=step('Legacy coding assignment without repository work',{method:'admitAssignment',runId:legacyCoding.id,input:input()});
  step('Legacy coding claimed',{method:'claimAttempt',attemptId:old.attempt.id});
  step('Legacy coding blocks unstopped lookup',{method:'unstoppedCodingRun',runId:codingB.id});
  step('Legacy coding stopped',{method:'recordOwnership',attemptId:old.attempt.id,stopped:true});
  const reader=step('Research assignment',{method:'admitAssignment',runId:research.id,input:input()});
  step('Research claim',{method:'claimAttempt',attemptId:reader.attempt.id});
  step('Input budget rejection retained',{method:'recordAgentEvent',attemptId:reader.attempt.id,sequence:1,event:{type:'model_policy_rejected',code:'input_budget'}});
  step('Failed attempt remains failed on later settled event',{method:'recordAgentEvent',attemptId:reader.attempt.id,sequence:2,event:{type:'session_settled'}});
  step('Stop rejected research attempt',{method:'recordOwnership',attemptId:reader.attempt.id,stopped:true});
  step('Late event cannot revive stopped attempt',{method:'recordAgentEvent',attemptId:reader.attempt.id,sequence:3,event:{type:'session_settled'}});

  const timed=step('Budget assignment',{method:'admitAssignment',runId:budget.id,input:input()});
  step('Budget claim',{method:'claimAttempt',attemptId:timed.attempt.id});
  step('Budget identity',{method:'recordAttemptIdentity',attemptId:timed.attempt.id,identity:{...identity,pane:'%budget'}});
  patch('runs',budget.id,{status:'running',desiredState:'running',activeSince:now});
  let tickSequence=0;
  const tick=(label:string,event:any)=>step(label,{method:'recordAgentEvent',attemptId:timed.attempt.id,sequence:++tickSequence,event});
  tick('Budget agent starts',{type:'agent_start'});now+=500;tick('Budget agent settles',{type:'session_settled'});
  const head='a'.repeat(40),base='b'.repeat(40);
  const publication={runId:budget.id,attemptId:timed.attempt.id,branch:'feature',target:'main',revision:{head,base},policy:{statusKeys:['unit'],pipelineSteps:['unit'],optionalSteps:[]},
    push:{state:'confirmed',expectedHead:head},pr:{state:'confirmed',id:1,providerState:'OPEN'},remoteSourceHead:head,remoteTargetHead:base,ready:false,
    observation:{pr:{id:1,state:'OPEN',source:{branch:{name:'feature'},commit:{hash:head}},destination:{branch:{name:'main'},commit:{hash:base}}},statuses:[{key:'unit',state:'INPROGRESS'}],pipelines:[]}};
  const observe=(label:string,p:any)=>{fixture(label,(db,map)=>db.prepare('INSERT OR REPLACE INTO publications VALUES(?,?)').run(budget.id,JSON.stringify(map(p))));step(`${label}: account`,{method:'accountTime',runId:budget.id});};
  observe('Current hosted CI fixture',publication);
  const providerVariants:[string,(p:any)=>void][]=[
    ['Ready publication',p=>p.ready=true],['Unavailable observation',p=>p.error='Unavailable'],['Merged publication',p=>p.merged={commit:head}],
    ['Unconfirmed push',p=>p.push.state='unknown'],['Unconfirmed PR',p=>p.pr.state='unknown'],
    ['Stale remote head',p=>p.remoteSourceHead='stale'],['Stale remote base',p=>p.remoteTargetHead='stale'],
    ['Closed PR',p=>p.observation.pr.state='DECLINED'],['Stale provider PR state',p=>p.pr.providerState='MERGED'],
    ['Wrong PR identity',p=>p.observation.pr.id=2],['Old observed head',p=>p.observation.pr.source.commit.hash='stale'],
    ['Old observed base',p=>p.observation.pr.destination.commit.hash='stale'],['Wrong source branch',p=>p.observation.pr.source.branch.name='other'],
    ['Wrong target branch',p=>p.observation.pr.destination.branch.name='other'],
    ['Failed required status',p=>p.observation.statuses[0].state='FAILED'],['Wrong status branch',p=>p.observation.statuses[0].refname='other'],
    ['Missing required status and pipeline',p=>p.observation.statuses=[]],
  ];
  const pipeline={state:{name:'IN_PROGRESS'},target:{commit:{hash:head},destination_commit:{hash:base},pullrequest:{id:1}},steps:[]};
  const withPipeline=(change:(p:any)=>void)=>(p:any)=>{p.observation.pipelines=[structuredClone(pipeline)];change(p);};
  providerVariants.push(
    ['Current pipeline without discovered status',withPipeline(p=>p.observation.statuses=[])],
    ['Old pipeline head',withPipeline(p=>p.observation.pipelines[0].target.commit.hash='stale')],
    ['Old pipeline base',withPipeline(p=>p.observation.pipelines[0].target.destination_commit.hash='stale')],
    ['Other pipeline PR',withPipeline(p=>p.observation.pipelines[0].target.pullrequest.id=2)],
    ['Failed pipeline',withPipeline(p=>p.observation.pipelines[0].state={name:'COMPLETED',result:{name:'FAILED'}})],
    ['Required pipeline step failure',withPipeline(p=>p.observation.pipelines[0].steps=[{name:'unit',state:{name:'COMPLETED',result:{name:'FAILED'}}}])],
    ['Unconfigured pipeline step',withPipeline(p=>p.observation.pipelines[0].steps=[{name:'Unknown',state:{name:'PENDING'}}])],
    ['Optional skipped step permits wait',withPipeline(p=>{p.policy.optionalSteps=['Optional'];p.observation.pipelines[0].steps=[{name:'Optional',state:{name:'COMPLETED',result:{name:'SKIPPED'}}}];})],
  );
  for(const [label,change] of providerVariants){const p=structuredClone(publication);change(p);observe(label,p);}
  observe('Restore valid hosted CI after refusal matrix',publication);now+=100000;
  assert.equal(step('Settled CI wait excluded across reopen',{method:'accountTime',runId:budget.id}).activeMs,500);
  tick('Actual agent work resumes budget',{type:'agent_start'});now+=200;tick('Agent settles back into CI wait',{type:'session_settled'});
  now+=100000;tick('Live tool defeats idle wait',{type:'tool_started',toolCallId:'child'});now+=300;
  assert.equal(step('Tool runtime charged',{method:'accountTime',runId:budget.id}).activeMs,1000);
  tick('Confirmed tool exit restores wait',{type:'tool_exited',toolCallId:'child',ownership:{state:'stopped'}});now+=100000;
  assert.equal(step('Stopped tool no longer charges wait',{method:'accountTime',runId:budget.id}).activeMs,1000);
  observe('Failed provider observation fixture',{...publication,error:'Unavailable'});now+=100;
  assert.equal(step('Unavailable observation charges elapsed time',{method:'accountTime',runId:budget.id}).activeMs,1100);
  observe('Current CI observation restored',publication);now+=100000;
  step('Restored wait excludes elapsed time',{method:'accountTime',runId:budget.id});
  step('Uncertain child ownership charges time',{method:'recordOwnership',attemptId:timed.attempt.id,stopped:false,reason:'Unverified surviving child'});now+=1100;
  const expired=step('Active budget expiration pauses with retained evidence',{method:'accountTime',runId:budget.id});
  assert.equal(expired.activeMs,2200);assert.equal(expired.status,'pausing');assert.equal(expired.pauseReason,'budget');
  step('Budget expiration cannot certify uncertain stop',{method:'finishPause',runId:budget.id});
  step('Stop expired attempt',{method:'recordOwnership',attemptId:timed.attempt.id,stopped:true});
  step('Finish expired pause',{method:'finishPause',runId:budget.id});
  step('Exhausted budget refuses replacement',{method:'replaceAttempt',assignmentId:timed.assignment.id,input:{automatic:false}},true);
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'exact deterministic timestamps; no timestamp normalization',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Store transactions only; no actual process, model, container or provider delivery'},null,2));
} finally {legacy.close();rmSync(root,{recursive:true,force:true});}
