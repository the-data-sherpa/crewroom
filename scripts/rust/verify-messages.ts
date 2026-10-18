/** N2.4a durable message oracle. Every native operation opens the database in a new process.
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

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n24a-')));
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
  const task=legacy.createTask({title:'Message record oracle',description:''});
  legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Investigate evidence',criteria:['Explain findings']});
  const run=legacy.queueResearch(task.id,{revision:2,requestId:randomUUID()});
  const roles=['coordinator','reviewer'].map(id=>({...defaultRoles.find(r=>r.id===id)!,model:'fixture/model',effort:'low' as const}));
  const team=roles.map((role,i)=>({name:i?'Reader':'Coordinator',role,coordinator:!i,input:'Inspect messages',model:resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{}),brief:{criteria:[],dependsOn:[]}}));
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  const admitted=step('Admit two independent assignment identities',{method:'admitInitialTeam',runId:run.id,team});
  const [coord,reader]=admitted.map((x:any)=>x.assignment), [ca,ra]=admitted.map((x:any)=>x.attempt);
  const runOp=(method:string,extra:any={})=>({method,runId:run.id,...extra});
  const peer=(key:string,input:any,attemptId=ca.id)=>({method:'prepareMessage',attemptId,toolCallId:key,input});
  const msg=(method:string,id:string,extra:any={})=>({method,id,...extra});
  const steer=(key:string,body='Cover evidence')=>runOp('steer',{input:{requestId:key,body}});
  const dispatch=(ids:string[],inputId:string,attemptId=ca.id)=>({method:'dispatchSteering',attemptId,ids,inputId});
  const restore=(inputId:string,attemptId=ca.id)=>({method:'restoreUndispatchedInput',attemptId,inputId});
  step('Mailbox rejects punctuation',{method:'bindMailbox',assignmentId:coord.id,name:'Bad-Name'},true);
  step('Mailbox binds once',{method:'bindMailbox',assignmentId:coord.id,name:'QuietRiver'});
  step('Mailbox replay survives reopen',{method:'bindMailbox',assignmentId:coord.id,name:'QuietRiver'});
  step('Mailbox cannot change identity',{method:'bindMailbox',assignmentId:coord.id,name:'OtherRiver'},true);
  step('Missing mailbox assignment',{method:'bindMailbox',assignmentId:'missing',name:'OtherRiver'},true);
  for(const [label,input] of [['unknown recipient',{to:'Missing',body:'Read'}],['empty body',{to:'Reader',body:''}],['unknown field',{to:'Reader',body:'Read',extra:true}],['oversize Unicode body',{to:'Reader',body:'🙂'.repeat(40001)}],['null reply',{to:'Reader',body:'Read',replyTo:null}],['unaccepted reply',{to:'Reader',body:'Read',replyTo:'missing'}]] as const)step(`Peer refuses ${label}`,peer('invalid',input),true);
  step('Empty tool identity refused',peer('',{to:'Reader',body:'Read'}),true);
  step('Missing sender refused',peer('missing',{to:'Reader',body:'Read'},'missing'),true);
  step('Unicode message limit counts code points',peer('unicode-limit',{to:'Reader',body:'🙂'.repeat(40000)}));
  const first=step('Peer prepare persists pending only',peer('first',{to:'Reader',body:' Read 🙂 '}));
  step('Peer same identity may address immutable recipient ID',peer('first',{to:reader.id,body:' Read 🙂 '}));
  step('Peer key cannot change body',peer('first',{to:'Reader',body:'Changed'}),true);
  step('Peer key cannot change recipient',peer('first',{to:'Coordinator',body:' Read 🙂 '}),true);
  step('Pending is not accepted',msg('acceptMessage',first.id),true);
  step('Pending is not dispatched',msg('recordDispatch',first.id),true);
  step('Transport identity must be positive',msg('recordMessageStored',first.id,{transportId:0}),true);
  step('Record confirmed Mail storage fixture',msg('recordMessageStored',first.id,{transportId:17}));
  step('Stored receipt replay',msg('recordMessageStored',first.id,{transportId:17}));
  step('Transport identity cannot change',msg('recordMessageStored',first.id,{transportId:18}),true);
  step('Stored is not dispatchable',msg('recordDispatch',first.id),true);
  step('Confirm acceptance',msg('acceptMessage',first.id));
  step('Acceptance replay',msg('acceptMessage',first.id));
  const reply=step('Reply links accepted inbound message',peer('reply',{to:'Coordinator',body:'Acknowledged',replyTo:first.id},ra.id));
  step('Reply must belong to sender',peer('bad-reply',{to:'Reader',body:'Bad',replyTo:first.id}),true);
  step('Dispatch binds recipient attempt',msg('recordDispatch',first.id));
  step('Dispatch replay retains binding',msg('recordDispatch',first.id));
  step('Unstopped ownership cannot restore',restore(first.id,ra.id),true);
  step('Stop recipient fixture',{method:'recordOwnership',attemptId:ra.id,stopped:true});
  step('Stopped host without saved input restores acceptance',restore(first.id,ra.id));
  step('Restore replay',restore(first.id,ra.id));
  step('Redispatch restored message',msg('recordDispatch',first.id));
  step('Receipt records delivery',msg('recordDispatch',first.id,{received:true}));
  step('Received message is not redispatched',msg('recordDispatch',first.id));
  step('Received input is not restored',restore(first.id,ra.id));
  step('Read retained received receipt',msg('getPeerMessage',first.id));
  step('Uncertain send is retained',msg('markMessageUncertain',reply.id));
  step('Unknown storage can reconcile with transport receipt',msg('recordMessageStored',reply.id,{transportId:19}));
  step('Storage ambiguity after known receipt',msg('markMessageUncertain',reply.id));
  step('Stored replay cannot erase ambiguity',msg('recordMessageStored',reply.id,{transportId:19}));
  step('Uncertain send cannot be accepted',msg('acceptMessage',reply.id),true);
  step('Uncertain send cannot be dispatched',msg('recordDispatch',reply.id),true);
  for(const state of ['failed','exited','attention_required']){patch('attempts',ca.id,{state});step(`Sender ${state} refused`,peer('late',{to:'Reader',body:'Late'}),true);}
  patch('attempts',ca.id,{state:'idle',ownership:'held'});patch('attempts',ra.id,{state:'idle',ownership:'held'});
  patch('assignments',reader.id,{retiredAt:new Date(now).toISOString()});step('Retired recipient refused',peer('late',{to:'Reader',body:'Late'}),true);
  step('Retired sender refused',peer('late',{to:'Coordinator',body:'Late'},ra.id),true);
  fixture('Restore live assignment fixture',(db,map)=>{const a=JSON.parse(String(db.prepare('SELECT data FROM assignments WHERE id=?').get(map(reader.id))!.data));delete a.retiredAt;db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(a),map(reader.id));});
  patch('runs',run.id,{status:'running',desiredState:'running',activeSince:now});
  patch('assignments',coord.id,{currentAttemptId:'superseded-fixture'});
  step('Superseded sender refused',peer('superseded',{to:'Reader',body:'Late'}),true);
  step('Superseded coordinator cannot dispatch steering',dispatch([],'stale'),true);
  step('Superseded coordinator cannot apply steering',{method:'applySteering',attemptId:ca.id,toolCallId:'stale',input:{messageId:'missing',disposition:'applied',reason:'Stale'}},true);
  patch('assignments',coord.id,{currentAttemptId:ca.id});
  step('Steering trims JavaScript BOM whitespace',steer('bom','\uFEFFInstruction\uFEFF'));
  step('Steering retains non-whitespace NEL',steer('nel','\u0085Instruction\u0085'));
  const note=step('Operator steering trims and persists',steer('one','  Cover evidence 🙂  '));
  step('Steering replay',steer('one','Cover evidence 🙂'));
  step('Steering identity rejects changed content',steer('one','Changed'),true);
  step('Whitespace steering refused',steer('empty','   '),true);
  step('Oversize Unicode steering refused',steer('large','🙂'.repeat(40001)),true);
  step('Only coordinator dispatches steering',dispatch([note.id],'input',ra.id),true);
  step('Pending steering cannot be applied',{method:'applySteering',attemptId:ca.id,toolCallId:'apply',input:{messageId:note.id,disposition:'applied',reason:'Used'}},true);
  step('Batch invalid second item rolls back first',dispatch([note.id,'missing'],'input'),true);
  step('Dispatch records exact time',dispatch([note.id],'input'));
  step('Duplicate steering dispatch refused',dispatch([note.id],'input'),true);
  step('Wrong receipt identity changes nothing',{method:'receiveSteering',attemptId:ca.id,inputId:'other'});
  step('Receipt persists after reopen',{method:'receiveSteering',attemptId:ca.id,inputId:'input'});
  step('Receipt replay keeps time',{method:'receiveSteering',attemptId:ca.id,inputId:'input'});
  const apply={method:'applySteering',attemptId:ca.id,toolCallId:'apply',input:{messageId:note.id,disposition:'applied',reason:' Included in scope. '}};
  step('Only active coordinator applies steering',{...apply,attemptId:ra.id},true);
  step('Application retains reason and time',apply);
  step('Application replay after clock advance',apply);
  step('Conflicting disposition refused',{...apply,input:{...apply.input,disposition:'rejected'}},true);
  const lost=step('Steering awaiting receipt',steer('lost'));
  step('Lost reply dispatch',dispatch([lost.id],'lost'));
  step('Authenticated rejection establishes receipt and decision',{...apply,input:{messageId:lost.id,disposition:'rejected',reason:'Outside accepted scope'}});
  const legacyNote=step('Legacy history fixture base',steer('legacy'));
  fixture('Legacy dispatched steering has unknown timestamp',(db,map)=>{const m=JSON.parse(String(db.prepare('SELECT data FROM steering WHERE id=?').get(map(legacyNote.id))!.data));delete m.transitions;m.delivery='dispatched';m.attemptId=map(ca.id);m.inputId='old';db.prepare('UPDATE steering SET data=? WHERE id=?').run(JSON.stringify(m),map(legacyNote.id));});
  step('Legacy received transition preserves unknown dispatch time',{method:'receiveSteering',attemptId:ca.id,inputId:'old'});
  const payload={kind:'validation',subjectId:'candidate-1',state:'pending'};
  const notice=step('Controller notice is durable pending',runOp('noticeDelivery',{payload}));
  step('Same controller payload replays',runOp('noticeDelivery',{payload}));
  const nd=(id:string,received=false,attemptId=ca.id)=>({method:'dispatchDeliveryNotice',attemptId,id,received});
  step('Notice only targets coordinator',nd(notice.id,false,ra.id),true);
  step('Notice cannot receive before dispatch',nd(notice.id,true),true);
  patch('runs',run.id,{desiredState:'paused'});step('Paused desire blocks notice dispatch',nd(notice.id),true);
  patch('runs',run.id,{desiredState:'running'});step('Notice dispatch',nd(notice.id));
  step('Notice repeated dispatch refused',nd(notice.id),true);
  step('Notice restore requires stop',restore(notice.id),true);
  step('Stop coordinator fixture',{method:'recordOwnership',attemptId:ca.id,stopped:true});
  step('Notice restored after confirmed stop',restore(notice.id));
  patch('attempts',ca.id,{state:'idle',ownership:'held'});
  step('Notice redispatch after restoration',nd(notice.id));step('Notice receipt',nd(notice.id,true));
  step('Notice receipt repeated is unavailable',nd(notice.id,true),true);
  step('Received notice replay preserves receipt',runOp('noticeDelivery',{payload}));
  step('Controller payload order retains legacy receipt identity semantics',runOp('noticeDelivery',{payload:{state:payload.state,subjectId:payload.subjectId,kind:payload.kind}}));
  step('Changed controller observation makes new notice',runOp('noticeDelivery',{payload:{...payload,state:'passed'}}));
  step('Only latest matching subject is deduplicated',runOp('noticeDelivery',{payload}));
  const pending=step('Pending before cancellation',steer('cancel-pending'));
  const uncertain=step('Unconfirmed before cancellation',steer('cancel-uncertain'));
  step('Dispatch unconfirmed instruction',dispatch([uncertain.id],'uncertain-input'));
  step('Cancel settles only pending steering',runOp('setDesiredState',{desiredState:'cancelled'}));
  step('Cancelled request replay retains undelivered result',steer('cancel-pending'));
  step('Cancelled run refuses new steering',steer('cancel-new'),true);
  step('Cancelled run refuses dispatch',dispatch([pending.id],'cancel'),true);
  step('Stop cancelled coordinator fixture',{method:'recordOwnership',attemptId:ca.id,stopped:true});
  step('Terminal restoration cannot enqueue work',restore('uncertain-input'));
  step('Terminal restoration replay',restore('uncertain-input'));
  step('Read retained steering decisions',runOp('steering'));step('Read retained controller notices',runOp('deliveryNotices'));
  // Backlog boundary is seeded, then exercised through real transactions.
  patch('runs',run.id,{status:'running',desiredState:'running'});patch('attempts',ca.id,{state:'idle',ownership:'held'});
  fixture('99 pending peer messages and 99 pending steering receipts',(db,map)=>{
    db.prepare('DELETE FROM peer_messages').run();db.prepare('DELETE FROM steering').run();
    for(let i=0;i<99;i++){
      const m=map({id:`peer-${i}`,runId:run.id,attemptId:ca.id,toolCallId:`seed-${i}`,from:coord.id,to:reader.id,body:'fixture',createdAt:new Date(now).toISOString(),disposition:'pending'});
      db.prepare('INSERT INTO peer_messages VALUES(?,?,?,?,?)').run(m.id,m.runId,m.attemptId,m.toolCallId,JSON.stringify(m));
      const n=map({id:`operator-${i}`,runId:run.id,body:'fixture',createdAt:new Date(now).toISOString(),delivery:'pending',transitions:[]});db.prepare('INSERT INTO steering VALUES(?,?,?,?)').run(n.id,n.runId,`seed-${i}`,JSON.stringify(n));
    }
  });
  const hundred=step('Hundredth peer allowed',peer('hundred',{to:'Reader',body:'Last'}));
  step('Hundred-first peer refused',peer('overflow',{to:'Reader',body:'Overflow'}),true);
  step('Peer keyed replay allowed at capacity',peer('hundred',{to:'Reader',body:'Last'}));
  step('Hundredth steering allowed',steer('hundred'));step('Hundred-first steering refused',steer('overflow'),true);step('Steering replay allowed at capacity',steer('hundred'));
  step('Persist transport at full backlog',msg('recordMessageStored',hundred.id,{transportId:99}));step('Accept at capacity',msg('acceptMessage',hundred.id));step('Received peer frees capacity',msg('recordDispatch',hundred.id,{received:true}));step('New peer after receipt',peer('after-receipt',{to:'Reader',body:'Next'}));
  const atCapacity=legacy.steering(run.id).at(-1)!;step('Dispatch does not free steering capacity',dispatch([atCapacity.id],'capacity'));step('Still full while dispatched',steer('still-full'),true);step('Receipt frees steering capacity',{method:'receiveSteering',attemptId:ca.id,inputId:'capacity'});step('New steering after receipt',steer('after-receipt'));
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps including legacy wall-clock calls; no timestamp normalization',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Durable records only; no Mail or host dispatch'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
