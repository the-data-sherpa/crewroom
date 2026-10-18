/** N2.4b team change oracle. Every native operation opens the database in a new process.
 * Process identities, publication observations and unported control states below
 * are explicit fixtures: this suite never starts agents or calls a provider. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { copyFileSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench, Conflict, NotFound } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n24b-')));
const legacyPath=join(root,'legacy.sqlite'), nativePath=join(root,'native.sqlite');
let now=1000, legacy=new Workbench(legacyPath,()=>now);
// A few legacy message methods use wall time directly. Control only the oracle process clock.
const RealDate=Date;
globalThis.Date=class extends RealDate {
  constructor(value?:string|number|Date) { super(value === undefined ? now : value); }
  static now() { return now; }
} as DateConstructor;
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
    case 'teamProposals':return s.teamProposals(op.runId);
    case 'insertionReplay':return s.insertionReplay(op.runId,op.input);
    case 'proposeOperatorSpecialist':return s.proposeOperatorSpecialist(op.runId,op.input,op.role,op.model);
    case 'insertionBlockers':return s.insertionBlockers(op.runId);
    case 'proposeTeam':return s.proposeTeam(op.attemptId,op.toolCallId,op.specialists,op.retire??[]);
    case 'rejectUnavailableTeamProposal':return s.rejectUnavailableTeamProposal(op.id,op.roles??[]);
    case 'applyTeamProposal':return s.applyTeamProposal(op.id,op.roles??[]);
    case 'beginTeamTransition':return s.beginTeamTransition(op.id);
    case 'finishTeamTransition':return s.finishTeamTransition(op.runId);
    case 'beginCodingTransition':return s.beginCodingTransition(op.runId,op.id);
    case 'finishCodingTransition':return s.finishCodingTransition(op.runId);
    case 'recordAttemptIdentity':return s.recordAttemptIdentity(op.attemptId,op.identity);
    case 'replaceAttempt':return s.replaceAttempt(op.assignmentId,op.input);
    case 'steer':return s.steer(op.runId,op.input);
    case 'steering':return s.steering(op.runId);
    case 'prepareMessage':return s.prepareMessage(op.attemptId,op.toolCallId,op.input);
    case 'bindMailbox':return s.bindMailbox(op.assignmentId,op.name);
    case 'getPeerMessage':return s.getPeerMessage(op.id);
    case 'recordMessageStored':return s.recordMessageStored(op.id,op.transportId);
    case 'acceptMessage':return s.acceptMessage(op.id);
    case 'markMessageUncertain':return s.markMessageUncertain(op.id);
    case 'recordDispatch':return s.recordDispatch(op.id,op.received??false);
    case 'deliveryNotices':return s.deliveryNotices(op.runId);
    case 'noticeDelivery':return s.noticeDelivery(op.runId,op.payload);
    case 'dispatchDeliveryNotice':return s.dispatchDeliveryNotice(op.attemptId,op.id,op.received??false);
    case 'restoreUndispatchedInput':return s.restoreUndispatchedInput(op.attemptId,op.inputId);
    case 'dispatchSteering':return s.dispatchSteering(op.attemptId,op.ids,op.inputId);
    case 'receiveSteering':return s.receiveSteering(op.attemptId,op.inputId);
    case 'applySteering':return s.applySteering(op.attemptId,op.toolCallId,op.input);
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
  if(error instanceof Conflict || error instanceof NotFound)assert.equal(actual.error,error.message,`${label}: refusal reason`);
  if(error && typeof error==='object' && 'terminal' in error)assert.equal(actual.terminal,error.terminal,`${label}: terminal disposition`);
  if(!refused){expected=JSON.parse(JSON.stringify(expected));
    if(expected?.taskSnapshot && expected.status){delete expected.holdReason;delete actual.value.holdReason;}
    remember(expected,actual.value);assert.deepEqual(fromNative(actual.value),expected,`${label}: result`);}
  compare(label);cases.push(label);now+=1000;return expected;
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
  const repo=saveRepositoryFixture(legacy,{name:'Team oracle',path:repoPath,targetBranch:'main',checks:[]});
  const makeTask=(title:string,repository=true)=>{const task=legacy.createTask({title,description:'',...(repository?{repositoryId:repo.id}:{})});return legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Keep accepted scope',criteria:['Explain findings']});};
  const run=legacy.queue(makeTask('Operator insertion').id,{revision:2});
  const research=legacy.queueResearch(makeTask('Research roles',false).id,{revision:2,requestId:randomUUID()});
  const exploreTask=legacy.createTask({title:'Refinement',description:''});const explore=legacy.queueRefinement(exploreTask.id,{revision:1,purpose:'explore'});
  const roles=defaultRoles.map(r=>({...r,model:'fixture/model',effort:'low' as const}));
  const role=roles.find(r=>r.id==='tech-writer')!, coordRole=roles.find(r=>r.id==='coordinator')!;
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const request=(extra:any={})=>({requestId:randomUUID(),roleId:role.id,input:' Explain accepted findings. ',dependsOn:['Coordinator'],criteria:[0],...extra});
  const operator=(input:any,runId=run.id,selectedRole:any=role)=>({method:'proposeOperatorSpecialist',runId,input,role:selectedRole,model});
  const byId=(method:string,id:string,extra:any={})=>({method,id,...extra});
  const runOp=(method:string,runId=run.id,extra:any={})=>({method,runId,...extra});
  const teamMember=(name:string,coordinator=false)=>({name,role:coordinator?coordRole:role,model,coordinator,input:'Inspect accepted scope',brief:{criteria:[],dependsOn:[]}});
  const specialist=(name:string,changes:any={})=>({name,role,model,input:' Inspect accepted scope ',...changes});
  const propose=(attemptId:string,key:string,specialists:any[],retire:any[]=[])=>({method:'proposeTeam',attemptId,toolCallId:key,specialists,retire});
  const state=()=>legacy.runtime(run.id);
  const coordinator=()=>state().assignments.find(a=>a.coordinator)!.currentAttemptId;
  const stop=(stage:string)=>{
    step(`${stage}: begin pause`,runOp('beginPause'));
    for(const attempt of state().attempts)if(attempt.state!=='admitted' && attempt.ownership!=='stopped')step(`${stage}: stop generation ${attempt.generation}`,{method:'recordOwnership',attemptId:attempt.id,stopped:true});
    step(`${stage}: confirmed pause`,runOp('finishPause'));
  };
  const activate=(stage:string)=>{
    for(const assignment of state().assignments.filter(a=>!a.retiredAt))if(legacy.getAttempt(assignment.currentAttemptId).state!=='admitted')step(`${stage}: replace ${assignment.name}`,{method:'replaceAttempt',assignmentId:assignment.id,input:{automatic:false}});
    if(legacy.getRun(run.id).status==='paused')step(`${stage}: continuation`,runOp('prepareContinuation',run.id,{controlRevision:legacy.getRun(run.id).controlRevision}));
    step(`${stage}: desire running`,runOp('setDesiredState',run.id,{desiredState:'running'}));
    for(const assignment of state().assignments.filter(a=>!a.retiredAt)){
      step(`${stage}: claim ${assignment.name}`,{method:'claimAttempt',attemptId:assignment.currentAttemptId});
      step(`${stage}: verified identity ${assignment.name}`,{method:'recordAttemptIdentity',attemptId:assignment.currentAttemptId,identity:{hostPid:101,ompPid:102,sessionId:assignment.id,hostUrl:'http://127.0.0.1:59999',pane:`%${state().assignments.indexOf(assignment)}`}});
    }
    step(`${stage}: execution fixture`,runOp('startExecution'));
  };
  const writer=(attemptId?:string)=>fixture('Repository writer ownership fixture',(db,map)=>{
    const value=map({runId:run.id,repository:repo,...(attemptId?{writer:{attemptId}}:{})});
    db.prepare('INSERT INTO repository_work VALUES(?,?,?,?) ON CONFLICT(run_id) DO UPDATE SET data=excluded.data').run(map(run.id),'fixture-common-dir','fixture-branch',JSON.stringify(value));
  });
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  for(const [label,changes] of [['unknown field',{extra:true}],['invalid request',{requestId:'invalid'}],['empty input',{input:' '}],['unknown dependency',{dependsOn:['Foreign']}],['unknown criterion',{criteria:[1]}],['duplicate criterion',{criteria:[0,0]}],['too many dependencies',{dependsOn:Array(11).fill('Coordinator')}]] as const)step(`Insertion refuses ${label}`,operator(request(changes)),true);
  step('Disabled selected role refused',operator(request(),run.id,{...role,enabled:false}),true);
  step('Mismatched selected role refused',operator(request(),run.id,coordRole),true);
  step('Refinement refuses operator specialist',operator(request(),explore.id),true);
  for(const id of ['implementer','reviewer'])step(`Research refuses ${id}`,operator(request({roleId:id}),research.id,roles.find(r=>r.id===id)),true);
  step('Research specialist requires criterion',operator(request({criteria:[]}),research.id),true);
  const reporter=roles.find(r=>r.id==='delivery-reporter')!;
  step('Research reporter may have no criterion',operator(request({roleId:reporter.id,criteria:[],dependsOn:[]}),research.id,reporter));
  const value=request();const pending=step('Queued insertion captures role model accepted criterion and notice',operator(value));
  step('Queued insertion replays before current role validation',{...operator(value),role:{id:'missing'},model:{changed:true}});
  step('Saved insertion replay query',runOp('insertionReplay',run.id,{input:pending.request}));
  step('Changed insertion replay refused',operator({...value,input:'Different'}),true);
  step('Second pending proposal refused',operator(request()),true);
  step('Pending insertion holds readiness',runOp('insertionBlockers'));
  const launch={team:[{name:'Other',roleId:coordRole.id,coordinator:true,input:'Coordinate'}]};
  step('Queued launch must preserve insertion dependency',runOp('setQueueLaunch',run.id,{input:{requestId:randomUUID(),launch},roles}),true);
  step('Initial team capacity includes insertion reservation',runOp('admitInitialTeam',run.id,{team:[teamMember('Coordinator',true),teamMember('One'),teamMember('Two'),teamMember('Three')]}),true);
  step('Initial team cannot use reserved insertion name',runOp('admitInitialTeam',run.id,{team:[teamMember('Coordinator',true),teamMember('Specialist1')]}),true);
  step('Admit original coordinator',runOp('admitInitialTeam',run.id,{team:[teamMember('Coordinator',true)]}));
  step('Queued proposal cannot apply before confirmed pause',byId('applyTeamProposal',pending.id,{roles}),true);
  activate('Initial');step('Current available catalog needs no rejection',byId('rejectUnavailableTeamProposal',pending.id,{roles}));
  step('Begin durable team transition',byId('beginTeamTransition',pending.id));
  stop('Initial insertion');
  patch('attempts',coordinator(),{ownership:'uncertain'});step('Uncertain ownership blocks team application',byId('applyTeamProposal',pending.id,{roles}),true);
  step('Resolve ownership fixture',{method:'recordOwnership',attemptId:coordinator(),stopped:true});
  step('Stopped sender cannot prepare more input',{method:'prepareMessage',attemptId:coordinator(),toolCallId:'pending',input:{to:'Coordinator',body:'Reconcile'}} ,true);
  fixture('Pending peer delivery fixture',(db,map)=>{const m=map({id:'fixture-peer',runId:run.id,attemptId:coordinator(),toolCallId:'fixture-peer',from:state().assignments[0].id,to:state().assignments[0].id,body:'Ambiguous',createdAt:new Date(now).toISOString(),disposition:'uncertain'});db.prepare('INSERT INTO peer_messages VALUES(?,?,?,?,?)').run(m.id,m.runId,m.attemptId,m.toolCallId,JSON.stringify(m));});
  step('Unreconciled peer prevents insertion',byId('applyTeamProposal',pending.id,{roles}),true);
  patch('peer_messages','fixture-peer',{disposition:'received'});
  const steering=step('Pending instruction before team change',runOp('steer',run.id,{input:{requestId:'team-steering',body:'Reconcile before insertion'}}));
  step('Dispatch instruction receipt fixture',{method:'dispatchSteering',attemptId:coordinator(),ids:[steering.id],inputId:'team-steering'});
  step('Uncertain steering prevents insertion',byId('applyTeamProposal',pending.id,{roles}),true);
  step('Steering receipt reconciles input',{method:'receiveSteering',attemptId:coordinator(),inputId:'team-steering'});
  step('Operator pause persists',runOp('setDesiredState',run.id,{desiredState:'paused'}));
  step('Operator pause prevents insertion',byId('applyTeamProposal',pending.id,{roles}),true);
  step('Explicit running desire',runOp('setDesiredState',run.id,{desiredState:'running'}));
  const revisedRoles=roles.map(r=>r.id===role.id?{...r,revision:2,instructions:'Updated instruction at admission'}:r);
  patch('repositories',repo.id,{guidance:[{version:1,text:'Current project guidance',approvedAt:new Date(now).toISOString(),proposalId:'fixture-guidance'}]});
  const applied=step('Stopped insertion snapshots current compatible role and guidance',byId('applyTeamProposal',pending.id,{roles:revisedRoles}));
  assert.equal(state().assignments[1].role.revision,2);assert.equal(state().assignments[1].brief!.criteria[0].criterion,'Explain findings');
  step('Applied insertion replay keeps original admission',byId('applyTeamProposal',pending.id,{roles}));
  step('Applied proposal is not rejected by later catalog',byId('rejectUnavailableTeamProposal',pending.id,{roles:roles.map(r=>({...r,enabled:false}))}));
  step('Applied specialist holds readiness until retirement',runOp('insertionBlockers'));
  step('Finish team transition',runOp('finishTeamTransition'));
  activate('Inserted team');
  const ca=coordinator(), inserted=state().assignments[1];
  step('Only coordinator can propose team',propose(inserted.currentAttemptId,'forged',[specialist('Writer')]),true);
  for(const [label,members,retire] of [['empty',[],[]],['disabled role',[specialist('Writer',{role:{...role,enabled:false}})],[]],['duplicate names',[specialist('Writer'),specialist('Writer')],[]],['existing name',[specialist('Specialist1')],[]],['retire coordinator',[],[{name:'Coordinator',reason:'No'}]],['unknown retirement',[],[{name:'Missing',reason:'No'}]],['duplicate retirement',[],[{name:'Specialist1',reason:'First'},{name:'Specialist1',reason:'Again'}]],['empty retirement reason',[],[{name:'Specialist1',reason:' '}]],['four additions',[specialist('One'),specialist('Two'),specialist('Three'),specialist('Four')],[]]] as const)step(`Team refuses ${label}`,propose(ca,`invalid-${label}`,members as any,retire as any),true);
  writer(inserted.currentAttemptId);step('Current repository writer cannot retire',propose(ca,'writer',[],[{name:'Specialist1',reason:'Finished'}]),true);writer();
  const rotate=propose(ca,'rotate',[specialist('Writer'),specialist('Researcher')],[{name:'Specialist1',reason:' Reconciled findings '}]);
  const rotation=step('Coordinator proposes additions and reasoned retirement',rotate);
  step('Coordinator proposal keyed replay',rotate);
  step('Coordinator proposal changed content refused',{...rotate,specialists:[specialist('Writer',{input:'Different'}),specialist('Researcher')]},true);
  step('Pending proposal reserves concurrency',propose(ca,'second',[specialist('Another')]),true);
  step('Running proposal cannot apply',byId('applyTeamProposal',rotation.id),true);
  step('Begin rotation transition',byId('beginTeamTransition',rotation.id));
  stop('Rotation');
  // The replay is still bound to the authenticated current attempt before lifecycle guards.
  patch('attempts',ca,{ownership:'held',state:'idle'});step('Saved coordinator proposal replays while paused',rotate);step('Restore stopped coordinator proof',{method:'recordOwnership',attemptId:ca,stopped:true});
  writer(inserted.currentAttemptId);step('Writer appearing before apply prevents retirement',byId('applyTeamProposal',rotation.id),true);writer();
  const movedModel=roles.map(r=>r.id===role.id?{...r,revision:3,model:'fixture/other',instructions:'Must not overwrite captured model role'}:r);
  step('Atomic rotation preserves proposal snapshot when model changes',byId('applyTeamProposal',rotation.id,{roles:movedModel}));
  assert.equal(state().assignments[1].retirementReason,'Reconciled findings');assert.equal(state().assignments[2].role.model,'fixture/model');
  step('Retired insertion no longer holds readiness',runOp('insertionBlockers'));
  step('Rotation replay cannot duplicate assignments',byId('applyTeamProposal',rotation.id,{roles:movedModel}));
  step('Finish rotation transition',runOp('finishTeamTransition'));activate('Rotated team');
  const current=coordinator();
  step('Retired names cannot be reused',propose(current,'old-name',[specialist('Specialist1')]),true);
  step('Already retired specialist cannot retire again',propose(current,'retire-again',[],[{name:'Specialist1',reason:'Again'}]),true);
  step('Three specialists plus coordinator ceiling',propose(current,'over-limit',[specialist('Third'),specialist('Fourth')]),true);
  const unavailable=step('Pending proposal for role availability check',propose(current,'disabled',[specialist('Next')]));
  const disabled=roles.map(r=>r.id===role.id?{...r,enabled:false}:r);
  const rejected=step('Disabled role rejects before any stop',byId('rejectUnavailableTeamProposal',unavailable.id,{roles:disabled}));assert.equal(legacy.getRun(run.id).status,'running');
  step('Rejected proposal replay retains one decision and notice',byId('rejectUnavailableTeamProposal',unavailable.id,{roles:disabled}));
  step('Rejected proposal cannot become applied after reenable',byId('applyTeamProposal',unavailable.id,{roles}));
  step('Empty catalog preserves legacy unchecked store boundary',byId('rejectUnavailableTeamProposal',unavailable.id));
  const operator2=step('New operator insertion never reuses historical names',operator(request()));assert.equal(operator2.specialists[0].name,'Specialist2');
  step('Missing role rejects operator proposal and rewrites pending notice',byId('rejectUnavailableTeamProposal',operator2.id,{roles:[coordRole]}));
  step('Operator rejection replay retains one notice',byId('rejectUnavailableTeamProposal',operator2.id,{roles:[coordRole]}));
  const operator3=step('Next proposal after rejection has next reserved name',operator(request()));assert.equal(operator3.specialists[0].name,'Specialist3');
  patch('runs',run.id,{taskRevision:3});
  step('Changed accepted scope rejects pending operator insertion',byId('rejectUnavailableTeamProposal',operator3.id,{roles}));
  step('Rejected changed-scope request remains replayable',operator(operator3.request));
  const fourth=step('Fresh request captures latest adopted revision',operator(request()));
  stop('Role disabled at apply');
  step('Apply rechecks disabled role after stop',byId('applyTeamProposal',fourth.id,{roles:disabled}));
  step('Begin transition refuses non-running run',byId('beginTeamTransition',fourth.id),true);
  step('Missing proposal transition refused',byId('beginTeamTransition','missing'),true);
  step('Coding completion transition requires requested completion',runOp('beginCodingTransition',run.id,{id:'completion'}),true);
  activate('Completion transition');
  fixture('Requested coding completion fixture',(db,map)=>db.prepare('INSERT INTO coding VALUES(?,?)').run(map(run.id),JSON.stringify(map({runId:run.id,completions:[{id:'completion',state:'requested'}]}))));
  step('Requested completion captures control revision',runOp('beginCodingTransition',run.id,{id:'completion'}));
  step('Coding transition replay',runOp('beginCodingTransition',run.id,{id:'completion'}));
  step('Finish coding transition',runOp('finishCodingTransition'));
  step('Finish coding transition replay',runOp('finishCodingTransition'));
  step('Review all retained team proposals',runOp('teamProposals'));
  const terminal=step('Proposal before cancellation',operator(request()));
  step('Cancellation rejects pending insertion',runOp('setDesiredState',run.id,{desiredState:'cancelled'}));
  step('Cancelled run request replay remains durable',operator(terminal.request));
  step('Cancelled run refuses new insertion',operator(request()),true);
  step('Cancelled insertion no longer holds readiness',runOp('insertionBlockers'));
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps including legacy wall-clock calls',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Store reservations and admission only; process ownership and model availability are fixtures'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
