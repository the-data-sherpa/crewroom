/** N2.5a refinement and research outcome oracle. Every native operation opens the database in a new process.
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

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n25a-')));
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
    case 'refinementProposals':return s.refinementProposals(op.runId);
    case 'proposeRefinement':return s.proposeRefinement(op.attemptId,op.toolCallId,op.input);
    case 'decideRefinement':return s.decideRefinement(op.id,op.input);
    case 'acceptRequirements':return s.acceptRequirements(op.taskId,op.input,op.provenance);
    case 'researchArtifacts':return s.researchArtifacts(op.runId);
    case 'saveResearchArtifact':return s.saveResearchArtifact(op.artifact);
    case 'requestResearchCompletion':return s.requestResearchCompletion(op.runId,op.attemptId,op.toolCallId);
    case 'completeResearch':return s.completeResearch(op.runId,op.requestId,op.revision);
    case 'setDesiredState':return s.setDesiredState(op.runId,op.desiredState);
    case 'recordOwnership':return s.recordOwnership(op.attemptId,op.stopped,op.reason);
    case 'beginPause':return s.beginPause(op.runId,op.reason);
    case 'finishPause':return s.finishPause(op.runId);
    case 'steer':return s.steer(op.runId,op.input);
    case 'proposeOperatorSpecialist':return s.proposeOperatorSpecialist(op.runId,op.input,op.role,op.model);
    case 'rejectUnavailableTeamProposal':return s.rejectUnavailableTeamProposal(op.id,op.roles);
    case 'adoptRequirements':return s.adoptRequirements(op.runId,op.input);
    case 'closeRefinement':return s.closeRefinement(op.runId);
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
  fixture(`fixture ${table}: ${Object.keys(changes).join(',')}`,(db,map)=>{
    const row=db.prepare(`SELECT data FROM ${table} WHERE id=?`).get(map(id))!;
    const value={...JSON.parse(String(row.data)),...map(changes)};
    db.prepare(`UPDATE ${table} SET data=? WHERE id=?`).run(JSON.stringify(value),map(id));
  });
}
try {
  const role={...defaultRoles.find(r=>r.id==='coordinator')!,model:'fixture/model',effort:'low' as const};
  const writer={...role,id:'tech-writer'};
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  function seed(purpose:'explore'|'grill'|'research') {
    const task=legacy.createTask({title:purpose,description:''});
    const accepted=purpose==='research'?legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Assess a decision',criteria:['Compare alternatives','Explain risks']}):task;
    const run=purpose==='research'?legacy.queueResearch(task.id,{revision:accepted.revision,requestId:randomUUID()}):legacy.queueRefinement(task.id,{revision:1,purpose});
    const coordinator=legacy.admitAssignment(run.id,{name:'Coordinator',coordinator:true,role,model,input:'Inspect scope'});
    const specialist=purpose==='research'?legacy.admitAssignment(run.id,{name:'Writer',coordinator:false,role:writer,model,input:'Explain findings'}):undefined;
    for(const {attempt} of [coordinator,...(specialist?[specialist]:[])]) {legacy.claimAttempt(attempt.id);legacy.recordAttemptIdentity(attempt.id,{hostPid:123,ompPid:124,sessionId:'fixture',hostUrl:'http://127.0.0.1:9',pane:'%1'});}
    legacy.setDesiredState(run.id,'running');legacy.startExecution(run.id);
    return {task:accepted,run,coordinator,specialist};
  }
  const explore=seed('explore'),grill=seed('grill'),research=seed('research'),pinned=seed('research');
  const standalone=legacy.createTask({title:'Manual accepted history',description:''});
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  const proposal={brief:' Refined brief ',criteria:[' Comparison '],decisions:[' Keep history '],unresolvedQuestions:[' Source uncertainty? ']};
  const propose=(key:string,input:any=proposal,attemptId=explore.coordinator.attempt.id)=>({method:'proposeRefinement',attemptId,toolCallId:key,input});
  const decide=(id:string,decision='accept',extra:any={})=>({method:'decideRefinement',id,input:{decision,requestId:randomUUID(),...extra}});
  const accept=(taskId:string,revision:number,extra:any={},provenance:any={origin:'operator'})=>({method:'acceptRequirements',taskId,input:{revision,sourceRevision:1,brief:'Accepted current brief',criteria:['Compare alternatives','Explain risks'],...extra},provenance});
  for(const [label,input] of [['empty criteria',{...proposal,criteria:[]}],['empty brief',{...proposal,brief:' '}],['missing decisions',{brief:proposal.brief,criteria:proposal.criteria,unresolvedQuestions:[]}],['unknown field',{...proposal,model:'other'}],['empty question',{...proposal,unresolvedQuestions:['\uFEFF']}],['too many criteria',{...proposal,criteria:Array(101).fill('x')}]] as const)step(`Proposal refuses ${label}`,propose(label,input),true);
  step('Missing proposal attempt refused',propose('missing',proposal,'missing'),true);
  step('Research coordinator cannot propose refinement',propose('research',proposal,research.coordinator.attempt.id),true);
  step('Empty tool identity refused',propose(''),true);
  const first=step('Refinement proposal and linked notes save together',propose('first'));
  step('Proposal identity replays normalized content',propose('first',{...proposal,brief:'Refined brief'}));
  step('Changed proposal content cannot reuse identity',propose('first',{...proposal,decisions:['Different']}),true);
  step('Read proposals for one run',{method:'refinementProposals',runId:explore.run.id});
  step('Unknown run proposal collection is empty',{method:'refinementProposals',runId:'missing'});
  for(const input of [{decision:'accept',requestId:'bad'},{decision:'accept',requestId:randomUUID(),criteria:[]},{decision:'accept',requestId:randomUUID(),extra:true}])step('Invalid refinement decision rejected',{method:'decideRefinement',id:first.id,input},true);
  step('Missing refinement proposal rejected',decide('missing'),true);
  const approval=decide(first.id,'accept',{brief:' Operator amended brief ',reason:' Chosen direction '});
  const accepted=step('Accept proposal edits requirements and every linked note atomically',approval);
  assert.equal(accepted.decision.revision,2);assert.equal(legacy.getRun(explore.run.id).taskRevision,1);
  step('Acceptance keyed replay',approval);
  step('Acceptance replay cannot alter reason',{...approval,input:{...approval.input,reason:'Changed'}},true);
  step('Acceptance replay cannot omit edited brief',{...approval,input:{decision:'accept',requestId:approval.input.requestId,reason:approval.input.reason}},true);
  step('New decision identity cannot accept again',decide(first.id),true);
  step('Accepted proposal cannot be discarded',decide(first.id,'discard',{reason:'Late'}),true);
  const stale=step('Proposal retains original run requirements',propose('stale'));
  step('Stale local revision cannot be accepted',decide(stale.id),true);
  step('Discard requires reason',decide(stale.id,'discard'),true);
  step('Discard cannot edit proposal',decide(stale.id,'discard',{reason:'Changed scope',brief:'Edit'}),true);
  const discard=decide(stale.id,'discard',{reason:' Changed scope '});step('Discard retains notes and records reason',discard);step('Discard keyed replay',discard);
  step('Discard replay cannot change reason',{...discard,input:{...discard.input,reason:'Different'}},true);
  step('Read all proposal history',{method:'refinementProposals'});
  step('Stop refinement coordinator',{method:'recordOwnership',attemptId:explore.coordinator.attempt.id,stopped:true});
  step('Stored proposal replays after coordinator stop',propose('first'));
  step('Stopped coordinator cannot create new proposal',propose('after-stop'),true);
  step('Pause refinement',{method:'beginPause',runId:explore.run.id});step('Finish refinement pause',{method:'finishPause',runId:explore.run.id});step('Close refinement retains outcomes',{method:'closeRefinement',runId:explore.run.id});
  const grilled=step('Grill proposal records its own note mode',propose('grilled',proposal,grill.coordinator.attempt.id));
  fixture('Jira source conflict and reacceptance fixture',(db,map)=>{const row=db.prepare('SELECT imported,refinement FROM tickets WHERE id=?').get(map(grill.task.id))!;db.prepare('UPDATE tickets SET imported=?,refinement=? WHERE id=?').run(JSON.stringify({...JSON.parse(String(row.imported)),sourceRevision:2}),JSON.stringify({...JSON.parse(String(row.refinement)),needsReacceptance:true}),map(grill.task.id));});
  step('Changed Jira source cannot accept old proposal',decide(grilled.id),true);
  step('Operator reacceptance retains source provenance',accept(grill.task.id,1,{sourceRevision:2,decisionNote:' Reviewed source '}));
  step('Refinement provenance requires proposal identity',accept(standalone.id,1,{}, {origin:'refinement'}),true);
  step('Manual acceptance normalizes criteria and decision note',accept(standalone.id,1,{brief:' ',criteria:['\uFEFF  Criterion \uFEFF'],decisionNote:'\uFEFF Decision \uFEFF',ignored:true}));
  step('Manual acceptance stale revision refused',accept(standalone.id,1),true);
  step('Manual acceptance source mismatch refused',accept(standalone.id,2,{sourceRevision:2}),true);
  step('Manual acceptance permits empty draft criteria',accept(standalone.id,2,{brief:'',criteria:[]}));
  step('Manual criteria Unicode limit applies after trimming',accept(standalone.id,3,{criteria:[' '.repeat(2100)+'🙂'.repeat(2000)+' ']}));
  step('Manual oversized Unicode criterion refused',accept(standalone.id,4,{criteria:['🙂'.repeat(2001)]}),true);
  const artifact=(team=research,extra:any={})=>({id:randomUUID(),runId:team.run.id,assignmentId:team.specialist!.assignment.id,attemptId:team.specialist!.attempt.id,toolCallId:randomUUID(),title:'Decision brief',kind:'research',body:{sha256:'a'.repeat(64),path:'fixture/artifact.md',bytes:10},model:{requested:model.requested,selector:model.resolved.selector,provider:model.resolved.provider,id:model.resolved.id,effort:model.resolved.effort},requirementRevision:2,citations:[],limitations:'Fixture verification only',coverage:[{criterionId:0,section:'Comparison'}],state:'verified',reasons:[],createdAt:new Date(now).toISOString(),...extra});
  const save=(value:any)=>({method:'saveResearchArtifact',artifact:value});
  const request=(team=research,key='complete',attemptId=team.coordinator.attempt.id)=>({method:'requestResearchCompletion',runId:team.run.id,attemptId,toolCallId:key});
  const complete=(team:any,requested:any,extra:any={})=>({method:'completeResearch',runId:team.run.id,requestId:requested.requestId,revision:requested.revision,...extra});
  step('Missing artifact run rejected',{method:'researchArtifacts',runId:'missing'},true);
  step('Read empty artifact history',{method:'researchArtifacts',runId:research.run.id});
  step('Foreign assignment cannot submit artifact',save(artifact(research,{attemptId:grill.coordinator.attempt.id})),true);
  step('Wrong requirement revision cannot submit artifact',save(artifact(research,{requirementRevision:1})),true);
  step('Unpinned run cannot accept pinned artifact',save(artifact(research,{pinnedCommit:'a'.repeat(40)})),true);
  patch('assignments',research.specialist!.assignment.id,{retiredAt:'fixture'});step('Retired research assignment cannot submit',save(artifact()),true);patch('assignments',research.specialist!.assignment.id,{retiredAt:undefined});
  patch('runs',research.run.id,{sourceConflict:{sourceRevision:2}});step('Conflicted source cannot accept artifacts',save(artifact()),true);patch('runs',research.run.id,{sourceConflict:undefined});
  const one=artifact();step('Save trusted verified artifact record',save(one));step('Artifact store replay retains original record',save({...one,title:'Lower boundary ignores replay content'}));
  const rejected=artifact(research,{state:'rejected',reasons:['Fixture rejection']});step('Rejected replacement supersedes earlier same-title artifact',save(rejected));
  const corrected=artifact();step('Corrected replacement keeps complete rejected history',save(corrected));
  step('Read complete supersession history',{method:'researchArtifacts',runId:research.run.id});
  step('Specialist cannot request completion',request(research,'wrong',research.specialist!.attempt.id),true);
  const asked=step('Coordinator records completion request',request());step('Completion request replay',request());step('Different completion identity refused',request(research,'other'),true);
  step('Artifact submission stops once completion requested',save(artifact()),true);step('Old artifact replay remains valid after completion request',save(one));
  step('Incomplete criterion coverage prevents completion',complete(research,asked),true);
  fixture('Additional verified coverage fixture',(db,map)=>{const row=db.prepare('SELECT data FROM research_artifacts WHERE id=?').get(map(corrected.id))!;const v=JSON.parse(String(row.data));v.coverage.push({criterionId:1,section:'Risks'});db.prepare('UPDATE research_artifacts SET data=? WHERE id=?').run(JSON.stringify(v),map(corrected.id));});
  step('Current coverage still requires stopped process ownership',complete(research,asked),true);
  step('Begin research pause',{method:'beginPause',runId:research.run.id});
  for(const member of [research.coordinator,research.specialist!])step('Confirm stopped research attempt',{method:'recordOwnership',attemptId:member.attempt.id,stopped:true});
  step('Finish research pause',{method:'finishPause',runId:research.run.id});
  step('Wrong completion revision refused',complete(research,asked,{revision:1}),true);step('Wrong completion identity refused',complete(research,asked,{requestId:'other'}),true);
  step('Retain late steering before completion',{method:'steer',runId:research.run.id,input:{requestId:'late',body:'Retain this direction'}});
  const inserted=step('Pending inserted responsibility blocks completion',{method:'proposeOperatorSpecialist',runId:research.run.id,input:{requestId:randomUUID(),roleId:writer.id,input:'Explain risk',criteria:[1]},role:writer,model});
  step('Unreconciled insertion refuses completion',complete(research,asked),true);
  step('Unavailable insertion is explicitly rejected',{method:'rejectUnavailableTeamProposal',id:inserted.id,roles:[{...writer,enabled:false}]});
  const outcome=step('Current covered stopped research completes and closes pending steering',complete(research,asked));assert.equal(outcome.status,'completed');
  step('Completed outcome replay',complete(research,asked));step('Replay retains completed outcome even with old caller revision',complete(research,asked,{revision:1}));
  step('Terminal completion request remains replayable',request());
  step('Accept new research scope without changing past outcome',accept(research.task.id,2,{criteria:['New direction']}));
  step('Adopt newer scope reopens with historical artifacts',{method:'adoptRequirements',runId:research.run.id,input:{revision:3,requestId:randomUUID(),reason:'New direction'}});
  step('Past completion cannot complete adopted scope',complete(research,asked),true);
  const commit='b'.repeat(40);patch('runs',pinned.run.id,{researchSource:{commit}});
  step('Pinned artifact requires captured source',save(artifact(pinned)),true);
  const citation={path:'fixture.md',commit,startLine:1,endLine:1,excerpt:'Fixture',verified:false};
  const pinnedArtifact=artifact(pinned,{pinnedCommit:commit,coverage:[{criterionId:0,section:'Comparison'},{criterionId:1,section:'Risk'}],citations:[citation]});
  step('Store retains trusted artifact citation flags',save(pinnedArtifact));
  const pinnedAsked=step('Pinned completion request',request(pinned));
  step('Unverified citation prevents completion',complete(pinned,pinnedAsked),true);
  fixture('Verified citation fixture',(db,map)=>{const row=db.prepare('SELECT data FROM research_artifacts WHERE id=?').get(map(pinnedArtifact.id))!;const v=JSON.parse(String(row.data));v.citations[0].verified=true;db.prepare('UPDATE research_artifacts SET data=? WHERE id=?').run(JSON.stringify(v),map(pinnedArtifact.id));});
  patch('runs',pinned.run.id,{researchSource:{commit:'c'.repeat(40)}});step('Changed pinned source prevents completion',complete(pinned,pinnedAsked),true);patch('runs',pinned.run.id,{researchSource:{commit}});
  step('Begin pinned pause',{method:'beginPause',runId:pinned.run.id});for(const member of [pinned.coordinator,pinned.specialist!])step('Confirm pinned attempt stop',{method:'recordOwnership',attemptId:member.attempt.id,stopped:true});step('Finish pinned pause',{method:'finishPause',runId:pinned.run.id});
  step('Complete pinned research with fixture citation proof',complete(pinned,pinnedAsked));
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps including legacy wall-clock calls',reopen:'Rust process and store reopened for every operation; TypeScript reopened at seed and finish',claims:'Outcome records only; artifact hashes, citation verification, source pins and process identities are explicit fixtures'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
