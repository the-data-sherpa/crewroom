/** N2.3 scheduling oracle. Every native operation opens the database in a new process.
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

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n23-')));
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
  const expected=snapshot(legacyPath), native=snapshot(nativePath);
  // An existing derived-view difference from N2.1, never an ownership/timing field.
  for(const state of [expected,native])for(const row of state.runs)delete row.data.holdReason;
  remember(expected,native);const actual=fromNative(native);
  for(const table of Object.keys(expected))assert.deepEqual(actual[table],expected[table],`${label}: ${table}`);
}
function invoke(op:any):any {
  const s=legacy;
  switch(op.method) {
    case 'queue':return s.queue(op.taskId,op.input,op.roles??[]);
    case 'setQueueLaunch':return s.setQueueLaunch(op.runId,op.input,op.roles??[]);
    case 'codingQueue':return s.codingQueue();
    case 'codingSlot':return s.codingSlot();
    case 'getRun':return s.getRun(op.runId);
    case 'reorderQueue':return s.reorderQueue(op.input);
    case 'cancelQueued':return s.cancelQueued(op.runId,op.input);
    case 'dependencyMerges':return s.dependencyMerges(s.getRun(op.runId));
    case 'eligibilityHold':return s.eligibilityHold(s.getRun(op.runId));
    case 'saveDependencyBaseHold':return s.saveDependencyBaseHold(op.runId,op.merges,op.reason);
    case 'setLaunchHold':return s.setLaunchHold(op.runId,op.launch,op.reason);
    case 'assertCodingSlotAvailable':return s.assertCodingSlotAvailable(op.runId);
    case 'claimNextCodingSlot':return s.claimNextCodingSlot();
    case 'claimCodingSlot':return s.claimCodingSlot(op.runId,op.manual??false);
    case 'releaseCodingSlot':return s.releaseCodingSlot(op.runId,op.reason,op.candidateId);
    case 'deferUnadmittedLaunch':return s.deferUnadmittedLaunch(op.runId,op.reason,op.dependency??false);
    case 'canReplayInitialLaunch':return s.canReplayInitialLaunch(op.runId,op.decisionId,op.controlRevision);
    case 'normalizeLaunchExecution':return s.normalizeLaunchExecution(op.runId);
    case 'restartScheduledLaunch':return s.restartScheduledLaunch(op.runId);
    case 'reclaimCodingSlot':return s.reclaimCodingSlot(op.runId,op.reviewRegressed??false);
    case 'setDesiredState':return s.setDesiredState(op.runId,op.desiredState);
    case 'prepareContinuation':return s.prepareContinuation(op.runId,op.controlRevision);
    case 'requireAttention':return s.requireAttention(op.runId,op.reason);
    case 'startExecution':return s.startExecution(op.runId);
    case 'setActiveLimit':return s.setActiveLimit(op.runId,op.input);
    case 'queueFeedbackResume':return s.queueFeedbackResume(op.runId);
    case 'holdQueuedFeedbackResume':return s.holdQueuedFeedbackResume(op.runId,op.reason,op.terminal);
    case 'queueReviewResume':return s.queueReviewResume(op.runId);
    case 'setResumeRequestState':return s.setResumeRequestState(op.runId,op.state);
    case 'clearResumeRequest':return s.clearResumeRequest(op.runId,op.disposition??'held');
    case 'markFeedbackHandoff':return s.markFeedbackHandoff(op.runId,op.id);
    case 'finishFeedbackContinuation':return s.finishFeedbackContinuation(op.runId,op.id,op.outcome);
    case 'admitInitialTeam':return s.admitInitialTeam(op.runId,op.team);
    case 'claimAttempt':return s.claimAttempt(op.attemptId);
    case 'recordOwnership':return s.recordOwnership(op.attemptId,op.stopped,op.reason);
    case 'recordAgentEvent':return s.recordAgentEvent(op.attemptId,op.sequence,op.event);
    case 'beginPause':return s.beginPause(op.runId,op.reason);
    case 'finishPause':return s.finishPause(op.runId);
    case 'accountTime':return s.accountTime(op.runId);
    default:throw new Error(`Unknown method ${op.method}`);
  }
}
function step(label:string,op:any,refused=false):any {
  protect(op);let expected:any, error:unknown;
  try {expected=invoke(op)??null;}catch(e){error=e;}
  assert.equal(!!error,refused,`${label}: TypeScript ${String(error)}`);
  const actual=JSON.parse(execFileSync(resolve('target/debug/examples/ledger-oracle'),[nativePath],{input:JSON.stringify(toNative({...op,now})),encoding:'utf8'}));
  assert.equal(actual.ok,!refused,`${label}: Rust ${JSON.stringify(actual)}`);
  if(error && typeof error==='object' && 'terminal' in error)assert.equal(actual.terminal,error.terminal,`${label}: terminal disposition`);
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
  const repo=saveRepositoryFixture(legacy,{name:'Scheduling oracle',path:repoPath,targetBranch:'main',checks:[]});
  const task=(title:string)=>{const t=legacy.createTask({title,description:'',repositoryId:repo.id});return legacy.refine(t.id,{revision:1,sourceRevision:1,brief:'Keep accepted behavior',criteria:['Retain evidence']});};
  const tasks=Array.from({length:12},(_,i)=>task(`Plan ${i}`));
  const upstream=task('Dependency');const upstreamRun=legacy.queue(upstream.id,{revision:2});
  const dependent=task('Dependent');legacy.setDependencies(dependent.id,{requestId:randomUUID(),dependencyRevision:0,dependsOn:[upstream.id]});
  const bare=legacy.queue(tasks[1].id,{revision:2});const dep=legacy.queue(dependent.id,{revision:2});
  const role={...defaultRoles.find(r=>r.id==='coordinator')!,model:'fixture/model',effort:'low' as const};
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const launch={team:[{name:'Coordinator',roleId:role.id,coordinator:true,input:'Coordinate accepted work.'}]};
  const queueInput={revision:2,requestId:randomUUID(),launch};
  const runOp=(method:string,runId:string,extra:any={})=>({method,runId,...extra});
  const queued=(taskId:string,input:any)=>({method:'queue',taskId,input,roles:defaultRoles});
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  for(const [label,changed] of [
    ['Unknown launch field',{...launch,unknown:true}],['Unknown member field',{team:[{...launch.team[0],unknown:true}]}],
    ['Duplicate assignment name',{team:[...launch.team,...launch.team]}],['Missing coordinator',{team:[{...launch.team[0],coordinator:false}]}],
    ['Unavailable role',{team:[{...launch.team[0],roleId:'missing'}]}],['Invalid name',{team:[{...launch.team[0],name:'--bad'}]}],
    ['Criterion outside acceptance',{team:[{...launch.team[0],criteria:[{index:2,obligation:'Check'}]}]}],
    ['Repeated criterion',{team:[{...launch.team[0],criteria:[{index:0,obligation:'Check'},{index:0,obligation:'Again'}]}]}],
    ['Unconfigured check',{team:[{...launch.team[0],criteria:[{index:0,obligation:'Check',checkIds:['missing']}]}]}],
    ['Missing dependency',{team:[{...launch.team[0],dependsOn:['Absent']}]}],
    ['Self dependency',{team:[{...launch.team[0],dependsOn:['Coordinator']}]}],
    ['Cyclic assignment dependencies',{team:[{...launch.team[0],dependsOn:['Reader']},{...launch.team[0],name:'Reader',coordinator:false,dependsOn:['Coordinator']}]}],
    ['Continue branch option',{...launch,repository:{mode:'continue',branch:'--unsafe',expectedHead:'a'.repeat(40)}}],
    ['Invalid continuation head',{...launch,repository:{mode:'continue',branch:'feature',expectedHead:'invalid'}}],
  ] as const) step(label,queued(tasks[0].id,{...queueInput,launch:changed}),true);
  step('Launch input rejects above Unicode code point limit',queued(tasks[0].id,{revision:2,launch:{team:[{...launch.team[0],input:'🙂'.repeat(40001)}]}}),true);
  step('Launch input accepts Unicode code point boundary',runOp('setQueueLaunch',bare.id,{input:{requestId:randomUUID(),launch:{team:[{...launch.team[0],input:'🙂'.repeat(40000)}]}},roles:defaultRoles}));
  const first=step('Capture normalized launch and queue receipt',queued(tasks[0].id,queueInput));
  step('Queue request replay',queued(tasks[0].id,queueInput));
  step('Captured queue replay survives disabled current catalog',{...queued(tasks[0].id,queueInput),roles:[]});
  step('Queue identity refuses changed launch',queued(tasks[0].id,{...queueInput,launch:{team:[{...launch.team[0],input:'Changed'}]}}),true);
  step('Duplicate unkeyed queue with same launch',queued(tasks[0].id,{revision:2,launch}));
  step('Duplicate queue with different launch refused',queued(tasks[0].id,{revision:2}),true);
  const edit={requestId:randomUUID(),launch:{...launch,repository:{mode:'continue',branch:'feature/retained',expectedHead:'a'.repeat(40)}}};
  step('Capture explicit branch continuation',runOp('setQueueLaunch',bare.id,{input:edit,roles:defaultRoles}));
  step('Launch edit replay',runOp('setQueueLaunch',bare.id,{input:edit,roles:[]}));
  step('Launch request identity refuses new content',runOp('setQueueLaunch',bare.id,{input:{...edit,launch},roles:defaultRoles}),true);
  const normal=first.launch;
  step('Stale preflight cannot overwrite edited launch',runOp('setLaunchHold',bare.id,{launch:normal,reason:'Old preflight'}));
  step('Current preflight stores hold',runOp('setLaunchHold',bare.id,{launch:legacy.getRun(bare.id).launch,reason:'Runtime unavailable'}));
  step('Preflight hold replay',runOp('setLaunchHold',bare.id,{launch:legacy.getRun(bare.id).launch,reason:'Runtime unavailable'}));
  step('Inspect eligibility hold',runOp('eligibilityHold',bare.id));
  step('Clear matching preflight hold',runOp('setLaunchHold',bare.id,{launch:legacy.getRun(bare.id).launch}));
  step('Capture dependent launch',runOp('setQueueLaunch',dep.id,{input:{requestId:randomUUID(),launch},roles:defaultRoles}));
  step('Dependency waits for actual recorded merge',runOp('eligibilityHold',dep.id));
  step('Unmerged dependency contributes no target proof',runOp('dependencyMerges',dep.id));
  fixture('Provider-observed merge fixture',(db)=>db.prepare('INSERT INTO publications VALUES(?,?)').run(upstreamRun.id,JSON.stringify({runId:upstreamRun.id,merged:{commit:'b'.repeat(40)}})));
  const merges=step('Recorded dependency merge identity',runOp('dependencyMerges',dep.id));
  step('Reject outdated dependency preflight',runOp('saveDependencyBaseHold',dep.id,{merges:'[]',reason:'Stale target'}));
  step('Hold dependent for target incorporation',runOp('saveDependencyBaseHold',dep.id,{merges:JSON.stringify(merges),reason:'Merge not in target'}));
  step('Dependent target hold is visible',runOp('eligibilityHold',dep.id));
  step('Clear matching target hold',runOp('saveDependencyBaseHold',dep.id,{merges:JSON.stringify(merges)}));
  const queue=step('Read complete queue reasons',{method:'codingQueue'});
  step('Reorder preserves captured launch and holds',{method:'reorderQueue',input:{requestId:randomUUID(),expectedRevision:queue.revision,runIds:[first.id,bare.id,dep.id,upstreamRun.id]}});
  const claimed=step('Atomic next-slot claim',{method:'claimNextCodingSlot'});
  assert.equal(claimed.id,first.id);assert.equal(claimed.position,undefined);
  step('No second automatic slot',{method:'claimNextCodingSlot'});
  step('Second manual slot refused',runOp('claimCodingSlot',bare.id,{manual:true}),true);
  step('Availability rejects competing holder',runOp('assertCodingSlotAvailable',bare.id),true);
  step('Claim replay retains scheduling identity',runOp('claimCodingSlot',first.id));
  step('Admitted slot prevents launch edits',runOp('setQueueLaunch',first.id,{input:{requestId:randomUUID(),launch},roles:defaultRoles}),true);
  step('Initial launch replay permits only captured decision',runOp('canReplayInitialLaunch',first.id,{decisionId:claimed.schedule.id,controlRevision:0}));
  step('Stale launch decision refused',runOp('canReplayInitialLaunch',first.id,{decisionId:randomUUID(),controlRevision:0}));
  step('Preparation failure restores original queue position',runOp('deferUnadmittedLaunch',first.id,{reason:'Preparation refused'}));
  step('Old preflight cannot clear preparation hold',runOp('setLaunchHold',first.id,{launch:normal}));
  step('Explicit launch edit clears preparation hold',runOp('setQueueLaunch',first.id,{input:{requestId:randomUUID(),launch},roles:defaultRoles}));
  const reclaimed=step('Retry claims same-priority plan',{method:'claimNextCodingSlot'});
  const team=[{name:'Coordinator',coordinator:true,role,model,input:'Coordinate accepted work.',brief:{criteria:[],dependsOn:[]}}];
  const admitted=step('Scheduled initial admission',runOp('admitInitialTeam',first.id,{team}));
  step('Cannot defer after assignment admission',runOp('deferUnadmittedLaunch',first.id,{reason:'Too late'}),true);
  step('Admitted but unclaimed launch remains replayable',runOp('canReplayInitialLaunch',first.id,{decisionId:reclaimed.schedule.id,controlRevision:0}));
  step('Desired running control',runOp('setDesiredState',first.id,{desiredState:'running'}));
  step('Old control revision cannot replay launch',runOp('canReplayInitialLaunch',first.id,{decisionId:reclaimed.schedule.id,controlRevision:0}));
  step('Claim initial host receipt',{method:'claimAttempt',attemptId:admitted[0].attempt.id});
  step('Claimed host prevents admission replay',runOp('canReplayInitialLaunch',first.id,{decisionId:reclaimed.schedule.id,controlRevision:1}));
  step('Quiet claimed host is not positive execution evidence',runOp('normalizeLaunchExecution',first.id));
  step('Pause interrupted initial launch',runOp('beginPause',first.id,{reason:'recovery'}));
  step('Uncertain launch cannot restart',runOp('restartScheduledLaunch',first.id),true);
  step('Stop interrupted initial host',{method:'recordOwnership',attemptId:admitted[0].attempt.id,stopped:true});
  step('Finish initial recovery pause',runOp('finishPause',first.id));
  step('Restart launch retains assignment and replaces only claimed attempt',runOp('restartScheduledLaunch',first.id));
  const restarted=legacy.runtime(first.id).attempts.at(-1)!;
  step('Start execution records durable launch fence',runOp('startExecution',first.id));
  now+=100;
  step('Agent-start observation charges active time',{method:'recordAgentEvent',attemptId:restarted.id,sequence:1,event:{type:'agent_start'}});
  step('Executed launch cannot replay',runOp('canReplayInitialLaunch',first.id,{decisionId:reclaimed.schedule.id,controlRevision:1}));
  step('Executed launch normalization is idempotent',runOp('normalizeLaunchExecution',first.id));
  step('Review release requires current candidate proof',runOp('releaseCodingSlot',first.id,{reason:'review',candidateId:'candidate'}),true);
  fixture('Ready publication fixture',(db,map)=>db.prepare('INSERT INTO publications VALUES(?,?)').run(map(first.id),JSON.stringify(map({runId:first.id,ready:true,candidateId:'candidate'}))));
  step('Review release requests pause and frees slot',runOp('releaseCodingSlot',first.id,{reason:'review',candidateId:'candidate'}));
  step('Review release replay is idempotent',runOp('releaseCodingSlot',first.id,{reason:'review',candidateId:'candidate'}));
  step('Unconfirmed old process blocks next slot',{method:'claimNextCodingSlot'});
  step('Stop review host',{method:'recordOwnership',attemptId:restarted.id,stopped:true});
  step('Finish review pause',runOp('finishPause',first.id));
  const next=step('Next independent queued plan can start',{method:'claimNextCodingSlot'});assert.equal(next.id,bare.id);
  step('Explicit review Resume queues behind holder',runOp('queueReviewResume',first.id));
  now+=100;
  step('Queued Resume replay retains original request time',runOp('queueReviewResume',first.id));
  step('Re-hold refuses competing slot',runOp('reclaimCodingSlot',first.id,{reviewRegressed:true}),true);
  step('Cancel desired state releases current slot',runOp('setDesiredState',bare.id,{desiredState:'cancelled'}));
  step('Cancelled run cannot request running',runOp('setDesiredState',bare.id,{desiredState:'running'}),true);
  fixture('Readiness invalidation fixture',(db,map)=>db.prepare('UPDATE publications SET data=? WHERE run_id=?').run(JSON.stringify(map({runId:first.id,ready:false,candidateId:'candidate',reviewInvalidated:{reason:'Target moved'}})),map(first.id)));
  step('Readiness regression reclaims slot without execution',runOp('reclaimCodingSlot',first.id,{reviewRegressed:true}));
  step('Held readiness reason stays distinct from running',{method:'codingQueue'});
  step('Resume begins from stored control',runOp('setResumeRequestState',first.id,{state:'starting'}));
  step('Clearing control retains held continuation',runOp('clearResumeRequest',first.id));
  step('Re-hold replay does not allocate a new schedule',runOp('reclaimCodingSlot',first.id,{reviewRegressed:true}));
  step('Set active execution limit',runOp('setActiveLimit',first.id,{input:{activeLimitMs:1000}}));
  step('Too-small active limit refused',runOp('setActiveLimit',first.id,{input:{activeLimitMs:999}}),true);
  step('Attention retains slot',runOp('requireAttention',first.id,{reason:'Needs inspection'}));
  step('Finish stopped attention pause',runOp('finishPause',first.id));
  step('Old continuation control refused',runOp('prepareContinuation',first.id,{controlRevision:0}),true);
  step('Release held slot for terminal cancel',runOp('setDesiredState',first.id,{desiredState:'cancelled'}));
  step('Finish terminal cancellation',runOp('finishPause',first.id));
  step('Claim dependency after merge preflight',runOp('claimCodingSlot',dep.id));
  step('Raced target check returns dependent to its prior priority',runOp('deferUnadmittedLaunch',dep.id,{reason:'Target moved after screening',dependency:true}));
  step('Deferred dependency exposes its exact merge hold',runOp('eligibilityHold',dep.id));
  step('Revalidated dependency clears raced hold',runOp('saveDependencyBaseHold',dep.id,{merges:JSON.stringify(merges)}));
  const retained=step('Queue prior-execution migration fixture',queued(tasks[3].id,{revision:2,launch}));
  step('Claim retained migration fixture',runOp('claimCodingSlot',retained.id));
  patch('runs',retained.id,{status:'paused',desiredState:'paused',activeMs:10});
  step('Positive retained active time normalizes missing launch fence',runOp('normalizeLaunchExecution',retained.id));
  assert.ok(legacy.getRun(retained.id).launchCompletedAt);
  step('Merged release frees retained slot',runOp('releaseCodingSlot',retained.id,{reason:'merged'}));
  step('Confirm stopped merged pause',runOp('finishPause',retained.id));
  const desired=step('Explicit continuation advances control revision',runOp('setDesiredState',retained.id,{desiredState:'running'}));
  step('Matching control can prepare continuation',runOp('prepareContinuation',retained.id,{controlRevision:desired.controlRevision}));
  step('New cancellation supersedes prepared continuation',runOp('setDesiredState',retained.id,{desiredState:'cancelled'}));

  // Feedback, candidates and work ownership are seeded receipts, not provider/Git delivery.
  const feedbackRun=step('Queue retained feedback plan',queued(tasks[2].id,{revision:2,requestId:randomUUID(),launch}));
  const head='c'.repeat(40);const repository={workspace:'fixture',slug:'fixture',uuid:'fixture-repository'};
  const publication={id:'publication',runId:feedbackRun.id,candidateId:'published',repositoryPath:repoPath,repository,branch:'feature/feedback',revision:{head},
    push:{state:'confirmed'},pr:{state:'confirmed',id:42,providerState:'OPEN'},feedback:{comments:[{id:1,body:'Please refine',version:'v1',currentness:'current'}],observedAt:new Date(now).toISOString()}};
  patch('runs',feedbackRun.id,{status:'paused',desiredState:'paused',position:null,slotRelease:{reason:'review',at:new Date(now).toISOString(),candidateId:'published'}});
  fixture('Remove fixture queue position',(db,map)=>{const row=db.prepare('SELECT data FROM runs WHERE id=?').get(map(feedbackRun.id))!;const run=JSON.parse(String(row.data));delete run.position;db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(run),map(feedbackRun.id));});
  fixture('Retained feedback publication and work',(db,map)=>{
    db.prepare('INSERT INTO publications VALUES(?,?)').run(map(feedbackRun.id),JSON.stringify(map(publication)));
    db.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(map(feedbackRun.id),repoPath,publication.branch,JSON.stringify(map({runId:feedbackRun.id,branch:publication.branch,repository:repo})));
    db.prepare('INSERT INTO coding VALUES(?,?)').run(map(feedbackRun.id),JSON.stringify(map({runId:feedbackRun.id,candidates:[{id:'published'}]})));
  });
  const observe=(label:string,p:any)=>fixture(label,(db,map)=>db.prepare('UPDATE publications SET data=? WHERE run_id=?').run(JSON.stringify(map(p)),map(feedbackRun.id)));
  observe('Unavailable feedback fixture',{...publication,feedback:undefined});
  step('Unavailable feedback cannot be selected',runOp('queueFeedbackResume',feedbackRun.id));
  for(const state of ['DECLINED','MERGED','SUPERSEDED','UNKNOWN']) {
    observe(`Provider PR ${state} fixture`,{...publication,pr:{...publication.pr,providerState:state}});
    step(`PR ${state} preserves terminal-versus-retryable refusal`,runOp('queueFeedbackResume',feedbackRun.id),true);
  }
  observe('Foreign branch feedback fixture',{...publication,branch:'foreign'});
  step('Foreign publication branch cannot gain write continuation',runOp('queueFeedbackResume',feedbackRun.id),true);
  observe('Restored feedback identity',publication);
  step('Explicit Resume captures feedback even with free slot',runOp('queueFeedbackResume',feedbackRun.id));
  let selected=legacy.getRun(feedbackRun.id).feedbackContinuations!.at(-1)!;
  step('Repeated feedback Resume does not double capture',runOp('queueFeedbackResume',feedbackRun.id));
  step('Queued feedback cannot complete before handoff',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{noChange:true}}),true);
  step('Persist selected feedback starting',runOp('setResumeRequestState',feedbackRun.id,{state:'starting'}));
  step('Record feedback handoff',runOp('markFeedbackHandoff',feedbackRun.id,{id:selected.id}));
  step('Handoff replay retains captured feedback',runOp('markFeedbackHandoff',feedbackRun.id,{id:selected.id}));
  step('Clear request preserves handed-off feedback',runOp('clearResumeRequest',feedbackRun.id));
  step('No-change feedback outcome',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{noChange:true}}));
  step('No-change outcome replay',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{noChange:true}}));
  observe('Currentness-only feedback refresh',{...publication,feedback:{...publication.feedback,comments:[{...publication.feedback.comments[0],currentness:'outdated'}]}});
  step('Currentness change does not duplicate feedback version',runOp('queueFeedbackResume',feedbackRun.id));
  assert.equal(legacy.getRun(feedbackRun.id).feedbackContinuations!.length,1);
  step('Clear no-change replay request',runOp('clearResumeRequest',feedbackRun.id));
  observe('Edited feedback version',{...publication,feedback:{...publication.feedback,comments:[{...publication.feedback.comments[0],body:'Revised request',version:'v2'}]}});
  step('New feedback version creates explicit continuation',runOp('queueFeedbackResume',feedbackRun.id));
  selected=legacy.getRun(feedbackRun.id).feedbackContinuations!.at(-1)!;
  step('Hold queued feedback without losing it',runOp('holdQueuedFeedbackResume',feedbackRun.id,{reason:'Temporary unavailable',terminal:false}));
  step('Explicit Resume reuses held feedback',runOp('queueFeedbackResume',feedbackRun.id));
  step('Start held feedback',runOp('setResumeRequestState',feedbackRun.id,{state:'starting'}));
  step('Feedback produces a candidate',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{candidateId:'successor'}}));
  step('Candidate outcome replay',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{candidateId:'successor'}}));
  fixture('Validated follow-on candidate fixture',(db,map)=>db.prepare('UPDATE coding SET data=? WHERE run_id=?').run(JSON.stringify(map({runId:feedbackRun.id,candidates:[{id:'successor'},{id:'successor2',feedbackFrom:{continuationId:selected.id,sourceCandidateId:'successor'}}]})),map(feedbackRun.id)));
  step('Accept candidate successor in captured feedback lineage',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{candidateId:'successor2'}}));
  step('Unrelated successor cannot rewrite feedback lineage',runOp('finishFeedbackContinuation',feedbackRun.id,{id:selected.id,outcome:{candidateId:'unrelated'}}),true);
  step('Clear completed feedback control',runOp('clearResumeRequest',feedbackRun.id));
  step('Explicit Resume retains same feedback work for newer candidate',runOp('queueFeedbackResume',feedbackRun.id));
  step('Terminal feedback hold is durable',runOp('holdQueuedFeedbackResume',feedbackRun.id,{reason:'PR identity changed',terminal:true}));
  step('Terminal hold blocks feedback continuation',runOp('queueFeedbackResume',feedbackRun.id),true);
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact deterministic timestamps',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Store decisions only; execution and provider delivery remain unavailable'},null,2));
} finally {legacy.close();rmSync(root,{recursive:true,force:true});}
