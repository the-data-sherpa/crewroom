/** N2.7c final store closure oracle. Each native operation reopens the database.
 * Legacy receipts and managed ownership below are explicit fixtures; no agents or providers run. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { closeSync, copyFileSync, mkdtempSync, openSync, realpathSync, renameSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench, Conflict, NotFound, RepositoryConfigurationChanged } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { ZodError } from 'zod';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n27c-')));
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
  :typeof value==='string'?convertString(value,strings):value;
// Launch instructions serialize earlier generated run IDs inside JSON. Map only those UUIDs.
function convertString(value:string,strings:(s:string)=>string):string {
  if(value.startsWith('{')) {try {return JSON.stringify(transform(JSON.parse(value),strings));}catch {/* Ordinary prose remains exact. */}}
  return strings(value);
}
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
    assert.deepEqual(Object.keys(fromNative(actual)).sort(),Object.keys(expected).sort());
    for(const [key,value] of Object.entries(expected))remember(value,actual[ids.get(key)??key]);
  } else assert.deepEqual(fromNative(actual),expected);
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
    case 'importJira': return s.importJira(op.input);
    case 'savePublicationFeedback': return s.savePublicationFeedback(op.runId,op.prId,op.read,op.observedAt);
    case 'noticeDelivery': return s.noticeDelivery(op.runId,op.payload);
    case 'agentEventsOfType': return s.agentEventsOfType(op.runId,op.types);
    case 'configuredCheckIds': return s.configuredCheckIds(op.runId);
    case 'isOwnedRepositoryWorktree': return s.isOwnedRepositoryWorktree(op.path);
    case 'validateInsertionTeam': return s.validateInsertionTeam(op.runId,op.team);
    case 'claimCodingSlot': return s.claimCodingSlot(op.runId,op.manual);
    case 'deferDependencyLaunch': return s.deferDependencyLaunch(op.runId,op.reason);
    case 'queueFeedbackResume': return s.queueFeedbackResume(op.runId);
    case 'getRun': return s.getRun(op.runId);
    case 'runtime': return s.runtime(op.runId);
    case 'repositoryReplay': return s.repositoryReplay(op.input);
    case 'assertCodingSlotAvailable': return s.assertCodingSlotAvailable(op.runId);
    case 'proposeLearning': return s.proposeLearning(op.input);
    case 'requirementRevisions': return s.requirementRevisions(op.taskId);
    default: throw new Error(`Unknown method ${op.method}`);
  }
}

function step(label:string,op:any,refused=false,spellings:Record<string,string>={}):any {
  protect(op);let expected:any, error:unknown;
  try {expected=invoke(structuredClone(op))??null;}catch(e){error=e;}
  assert.equal(!!error,refused,`${label}: TypeScript ${String(error)}`);
  let input=JSON.stringify(toNative({...op,now}));
  for(const [field,spelling] of Object.entries(spellings)) {
    if(field==='criteria') input=input.replace('"criteria":[0]','"criteria":['+spelling+']');
    else input=input.replace(new RegExp('"'+field+'":-?[0-9]+','g'),'"'+field+'":'+spelling);
  }
  const requestPath=join(root,'request.json');writeFileSync(requestPath,input);const fd=openSync(requestPath,'r');
  let actual:any;try {actual=JSON.parse(execFileSync(resolve('target/debug/examples/ledger-oracle'),[nativePath],{stdio:[fd,'pipe','pipe'],encoding:'utf8',timeout:30000}));}finally {closeSync(fd);}

  if(error instanceof ZodError) assert.equal(actual.kind,'invalid',label);
  assert.equal(actual.ok,!refused,`${label}: Rust ${JSON.stringify(actual)}`);
  if(error instanceof Conflict || error instanceof NotFound)assert.equal(actual.error,error.message,`${label}: refusal reason`);
  assert.equal(actual.repositoryConfigurationChanged??false,error instanceof RepositoryConfigurationChanged,`${label}: retryable configuration disposition`);
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
  fixture(`fixture ${table}: ${Object.keys(changes).join(', ')}`,(db,map)=>{
    const key=['repository_work','coding','publications'].includes(table)?'run_id':'id';
    const row=db.prepare(`SELECT data FROM ${table} WHERE ${key}=?`).get(map(id));
    const value=JSON.parse(String(row!.data));for(const [k,v] of Object.entries(map(changes))) {if(v===undefined)delete value[k];else value[k]=v;}
    db.prepare(`UPDATE ${table} SET data=? WHERE ${key}=?`).run(JSON.stringify(value),map(id));
  });
}
try {
  const checkout=join(root,'repo');seedRepository(checkout);
  const check={id:'unit',image:'sha256:'+'a'.repeat(64),command:'/bin/true',args:[],timeoutMs:100,required:true};
  const repoInput={name:'Closure fixture',path:checkout,targetBranch:'main',checks:[check],requestId:randomUUID()};
  const repo=saveRepositoryFixture(legacy,repoInput);
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  function task(name:string,repositoryId?:string) {const t=legacy.createTask({title:name,description:'',repositoryId});return legacy.refine(t.id,{revision:1,sourceRevision:1,brief:'Accepted',criteria:['Verify']});}
  const run=legacy.queue(task('Observed',repo.id).id,{revision:2});
  const scheduled=legacy.queue(task('Deferred',repo.id).id,{revision:2});
  const research=legacy.queueResearch(task('Research').id,{revision:2,requestId:randomUUID()});
  const member=legacy.admitAssignment(run.id,{name:'Coordinator',coordinator:true,role,model,input:'Fixture'});
  legacy.recordAgentEvent(member.attempt.id,1,{type:'prompt_result',state:'success'});
  legacy.recordAgentEvent(member.attempt.id,2,{type:'tool_started',commandId:'fixture'});
  legacy.recordAgentEvent(member.attempt.id,3,{type:'prompt_result',state:'failed'});
  const learningEvidence=legacy.steer(run.id,{requestId:'closure-evidence',body:'Captured project learning evidence'});
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  step('Direct repository receipt replay',{method:'repositoryReplay',input:repoInput});
  const source=(id:string,date:string)=>({id,key:'SIM-'+id,fields:{summary:'Source '+id,status:{name:'Open'},updated:date}});
  const imported=(issues:any[])=>({method:'importJira',input:{siteUrl:'https://fixture.atlassian.net',issues}});
  const pairs=[['2026-02-31','2026-03-02','2026-03-03'],['2026-10-02T24:00:00Z','2026-10-02T23:59:59.999Z','2026-10-03T00:00Z'],['October 2 2026 12:00 GMT','Fri, 02 Oct 2026 11:59:59 GMT','2026-10-02T12:00:00.0000Z'],['+010000-01-01T00:00Z','9999-12-31T23:59:59Z','+010000-01-01T00:00:00Z'],['2026-03-08T03:30:00','2026-03-08T01:59:59','2026-03-08 03:30:00'],['2026-10','2026-09-30T23:59:59Z','2026-10-01']];
  for(const [i,[initial,older,same]] of pairs.entries()) {
    const id=String(i+1);step('Source date retained '+initial,imported([source(id,initial)]));
    step('Stale date refuses whole batch '+older,imported([source('99','2026-10-02'),source(id,older)]),true);
    step('Equivalent source instant accepted '+same,imported([source(id,same)]));
  }
  for(const date of ['2026-10-02T25:00','2026-13-01','2026-10-02T24:00:00.001Z','+275760-09-13T00:00:00.001Z'])step('Invalid date writes nothing '+date,imported([source('99',date)]),true);
  const comment=(id:number,created:string,updated=created,body='Current')=>({id,created_on:created,updated_on:updated,content:{raw:body},deleted:false,user:{display_name:'Fixture'},links:{html:{href:'https://fixture.invalid/comment/'+id}}});
  const first=comment(1,'October 2 2026 12:00 GMT');
  const publication={id:randomUUID(),runId:run.id,repository:{workspace:'fixture',repoSlug:'fixture'},repositoryPath:checkout,branch:'crewroom/fixture',candidateId:'fixture-candidate',revision:{head:'a'.repeat(40)},pr:{state:'confirmed',id:1,providerState:'OPEN'},feedback:{observedAt:'2026-10-02T00:00:00Z',state:'current',comments:[{...first,currentness:'known'}]}};
  fixture('Publication and held work are controller fixtures',(db,map)=>{
    db.prepare('INSERT INTO publications VALUES(?,?)').run(map(run.id),JSON.stringify(map(publication)));
    db.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(map(run.id),'fixture-common','fixture-branch',JSON.stringify(map({runId:run.id,path:join(root,'owned'),repository:repo,branch:publication.branch})));
  });
  const feedback=(comments:any[])=>({method:'savePublicationFeedback',runId:run.id,prId:1,read:{state:'current',comments},observedAt:new Date(now+1000).toISOString()});
  // Existing observation must be earlier than the controlled receipt clock.
  patch('publications',run.id,{feedback:{...publication.feedback,observedAt:new Date(0).toISOString()}});
  step('Legacy dates order comments by instant',feedback([first,comment(3,'2026-02-31'),comment(2,'2026-03-03')]));
  step('Older legacy comment preserves newer body',feedback([comment(1,first.created_on,'2026-10-02T11:59:59Z','Older')]));
  step('Equivalent non-ISO update rejects conflicting body',feedback([comment(1,first.created_on,'2026-10-02T12:00:00.0000Z','Conflict')]));
  step('Newer legacy update becomes current',feedback([comment(1,first.created_on,'October 2 2026 12:01 GMT','Newer')]));
  const notice={method:'noticeDelivery',runId:run.id,payload:{kind:'fixture',subjectId:'numeric',amount:1,details:{z:1e20,'10':10,'2':2,'01':1}}};
  step('Controller notice saves numeric payload',notice);step('Equivalent JSON decimal payload replays',notice,false,{amount:'1.0'});
  step('Equivalent controller payload after reopen',notice);
  step('Ordinary field order changes captured identity',{...notice,payload:{subjectId:'numeric',kind:'fixture',amount:1,details:notice.payload.details}});
  for(const types of [[],['prompt_result'],['tool_started','prompt_result'],['missing'],['prompt_result','prompt_result'],["x') OR 1=1 --"]])step('Filtered event read '+JSON.stringify(types),{method:'agentEventsOfType',runId:run.id,types});
  step('Missing run event projection is empty',{method:'agentEventsOfType',runId:'missing',types:['prompt_result']});
  for(const r of [run,research])step('Configured checks for '+r.purpose,{method:'configuredCheckIds',runId:r.id});
  step('Missing run configured checks refusal',{method:'configuredCheckIds',runId:'missing'},true);
  step('Captured repository checks survive config edit',{method:'configuredCheckIds',runId:run.id});
  patch('repositories',repo.id,{checks:[]});step('Owned snapshot remains authoritative for checks',{method:'configuredCheckIds',runId:run.id});step('Unowned plan uses current repository checks',{method:'configuredCheckIds',runId:scheduled.id});
  for(const path of [join(root,'owned'),checkout,join(root,'missing')])step('Exact owned-path lookup '+path,{method:'isOwnedRepositoryWorktree',path});
  step('Insertion helper accepts distinct bounded team',{method:'validateInsertionTeam',runId:scheduled.id,team:[{name:'Coordinator',coordinator:true},{name:'Worker',coordinator:false}]});
  step('Insertion helper refuses duplicate names',{method:'validateInsertionTeam',runId:scheduled.id,team:[{name:'Worker',coordinator:false},{name:'Worker',coordinator:false}]},true);
  step('Insertion helper refuses over-capacity team',{method:'validateInsertionTeam',runId:scheduled.id,team:[0,1,2,3].map(i=>({name:'Worker'+i,coordinator:false}))},true);
  step('Insertion helper retains capacity-before-missing-run refusal',{method:'validateInsertionTeam',runId:'missing',team:[0,1,2,3].map(i=>({name:'Worker'+i,coordinator:false}))},true);
  step('Insertion helper missing run with valid team',{method:'validateInsertionTeam',runId:'missing',team:[]},true);
  step('Coding slot availability before claim',{method:'assertCodingSlotAvailable',runId:scheduled.id});
  step('Claim slot before dependency deferral',{method:'claimCodingSlot',runId:scheduled.id,manual:true});
  step('Another slot holder is refused',{method:'assertCodingSlotAvailable',runId:run.id},true);
  step('Dependency helper releases slot and saves hold',{method:'deferDependencyLaunch',runId:scheduled.id,reason:'Dependency base unavailable'});
  step('Repeated dependency deferral refuses released slot',{method:'deferDependencyLaunch',runId:scheduled.id,reason:'Dependency base unavailable'},true);
  // Completed feedback has no active queue control. A reordered ordinary field
  // changes serialized identity even when the data is structurally equal.
  patch('runs',run.id,{status:'paused',desiredState:'paused',position:undefined});
  patch('publications',run.id,{feedback:publication.feedback});
  step('Capture feedback for Resume',{method:'queueFeedbackResume',runId:run.id});
  let captured=legacy.getRun(run.id).feedbackContinuations!;
  patch('runs',run.id,{resumeRequest:undefined,feedbackContinuations:captured.map(x=>({...x,state:'no_change'}))});
  step('Unchanged completed feedback reuses identity',{method:'queueFeedbackResume',runId:run.id});
  patch('runs',run.id,{resumeRequest:undefined});
  const reordered=Object.fromEntries(Object.entries({...first,currentness:'known'}).reverse());
  patch('publications',run.id,{feedback:{...publication.feedback,comments:[reordered]}});
  step('Reordered ordinary feedback fields create new capture',{method:'queueFeedbackResume',runId:run.id});
  const learning={method:'proposeLearning',input:{runId:run.id,attemptId:member.attempt.id,toolCallId:'closure-learning',target:{kind:'project',repositoryId:repo.id},change:'Preserve captured evidence',baseVersion:0,rationale:'Fixture validated by controller',evidence:[{kind:'steering',id:learningEvidence.id}]}};
  step('Trusted learning store boundary',learning);step('Trusted learning store replay',learning);
  step('Assignment and attempt projection',{method:'runtime',runId:run.id});
  step('Accepted requirement history',{method:'requirementRevisions',taskId:run.taskId});
  step('Final feedback history',{method:'getRun',runId:run.id});
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),claims:'Source-date ordering, captured JSON identities and final public-helper closure on schema 26. Controller work/publication/ownership records are fixtures; no native execution or provider delivery.'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
