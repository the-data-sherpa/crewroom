/** N2.5b provider record and Jira intake oracle. Every native operation opens the database in a new process.
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

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n25b-')));
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
    case 'codingRecord':return s.codingRecord(op.runId);
    case 'updateCodingRecord':return s.updateCodingRecord(op.runId,current=>{if(op.throw)throw new Conflict('Fixture update failed.');return op.merge?{...current??{runId:op.runId},...op.record}:op.record;});
    case 'publications':return s.publications();
    case 'publication':return s.publication(op.runId);
    case 'savePublication':return s.savePublication(op.record);
    case 'savePublicationFeedback':return s.savePublicationFeedback(op.runId,op.prId,op.read,op.observedAt);
    case 'jiraUpdates':return s.jiraUpdates(op.runId);
    case 'saveJiraUpdate':return s.saveJiraUpdate(op.update);
    case 'jiraState':return s.jiraState();
    case 'commitJiraSync':return s.commitJiraSync(op.source,op.input,op.retained);
    case 'failJiraSync':return s.failJiraSync(op.source,op.error);
    case 'importJira':return s.importJira(op.input);
    case 'getTask':return s.getTask(op.taskId);
    case 'acceptRequirements':return s.acceptRequirements(op.taskId,op.input,op.provenance);
    case 'adoptRequirements':return s.adoptRequirements(op.runId,op.input);
    case 'codingSlot':return s.codingSlot();
    case 'setDesiredState':return s.setDesiredState(op.runId,op.desiredState);
    case 'recordAgentEvent':return s.recordAgentEvent(op.attemptId,op.sequence,op.event);
    case 'accountTime':return s.accountTime(op.runId);
    case 'deliveryNotices':return s.deliveryNotices(op.runId);
    default:throw new Error(`Unknown method ${op.method}`);
  }
}
function step(label:string,op:any,refused=false):any {
  protect(op);let expected:any, error:unknown;
  try {expected=invoke(structuredClone(op))??null;}catch(e){error=e;}
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
try {
  const source={siteUrl:'https://fixture.atlassian.net',cloudId:'fixture-cloud',filterId:'1',assigneeAccountId:'operator',name:'Assigned work'};
  const issue=(id:string,title=`Issue ${id}`,extra:any={})=>({id,key:`FIX-${id}`,fields:{summary:title,description:'Source requirement',status:{id:'1',name:'To Do'},updated:'2026-09-28T00:00:00.000Z',...extra}});
  const initial=legacy.importJira({siteUrl:source.siteUrl,issues:[issue('1'),issue('2')]});
  legacy.refine(initial[0].id,{revision:1,sourceRevision:1,brief:'Retain local decisions',criteria:['Keep history']});
  const intakeRun=legacy.queueRefinement(initial[0].id,{revision:2,purpose:'explore'});
  const repoPath=join(root,'repo');seedRepository(repoPath);const repo=saveRepositoryFixture(legacy,{name:'Provider oracle',path:repoPath,targetBranch:'main',checks:[]});
  const task=legacy.createTask({title:'Published work',description:'',repositoryId:repo.id});legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Validated change',criteria:['Keep evidence']});const run=legacy.queue(task.id,{revision:2});legacy.claimCodingSlot(run.id,true);
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const member={name:'Coordinator',coordinator:true,role,model,input:'Keep records'};
  const coordinator=legacy.admitAssignment(run.id,member),intakeCoordinator=legacy.admitAssignment(intakeRun.id,member);
  for(const [r,a] of [[run,coordinator],[intakeRun,intakeCoordinator]] as const) {legacy.claimAttempt(a.attempt.id);legacy.recordAttemptIdentity(a.attempt.id,{hostPid:123,ompPid:124,sessionId:'fixture',hostUrl:'http://127.0.0.1:9',pane:'%1'});legacy.setDesiredState(r.id,'running');legacy.startExecution(r.id);}
  const idleTask=legacy.createTask({title:'Other saved records',description:'',repositoryId:repo.id});legacy.refine(idleTask.id,{revision:1,sourceRevision:1,brief:'Separate run',criteria:['Own history']});const idle=legacy.queue(idleTask.id,{revision:2});
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  step('No current coding record',{method:'codingRecord',runId:run.id});step('Missing coding run rejected',{method:'codingRecord',runId:'missing'},true);
  const record={runId:run.id,checks:[],reviews:[],completions:[{id:'fixture-completion',state:'completed',summary:'Historical result'}]};
  step('Persist coding record',{method:'updateCodingRecord',runId:run.id,record});
  step('Coding callback failure changes no record',{method:'updateCodingRecord',runId:run.id,throw:true},true);
  step('Coding record cannot change run identity',{method:'updateCodingRecord',runId:run.id,record:{...record,runId:idle.id}},true);
  step('Coding callback can retain history while extending the record',{method:'updateCodingRecord',runId:run.id,merge:true,record:{checks:[{id:'fixture-check',state:'passed'}]}});
  step('Read updated coding record',{method:'codingRecord',runId:run.id});
  step('Empty publication read',{method:'publication',runId:run.id});step('Unknown publication read remains absent',{method:'publication',runId:'missing'});
  step('Feedback for absent publication rejected',{method:'savePublicationFeedback',runId:run.id,prId:9,read:{state:'complete',comments:[]},observedAt:new Date(now).toISOString()},true);
  const head='a'.repeat(40),base='b'.repeat(40);
  const publication={id:randomUUID(),runId:run.id,attemptId:coordinator.attempt.id,candidateId:'fixture-candidate',repositoryPath:repoPath,repository:{workspace:'fixture',slug:'repo'},branch:'crewroom/fixture',target:'main',revision:{head,base,tree:'c'.repeat(40)},title:'Saved publication',description:'Fixture only',policy:{statusKeys:['unit'],pipelineSteps:[],optionalSteps:[]},createdAt:new Date(now).toISOString(),push:{state:'pending',expectedHead:null},pr:{state:'pending'},ready:false,blockers:['Not published']};
  step('Save publication intent and account active time',{method:'savePublication',record:publication});step('Read publication intent',{method:'publication',runId:run.id});
  step('Unconfirmed PR refuses feedback',{method:'savePublicationFeedback',runId:run.id,prId:9,read:{state:'complete',comments:[]},observedAt:new Date(now).toISOString()},true);
  const confirmed={...publication,push:{state:'confirmed',expectedHead:null},pr:{state:'confirmed',id:9,providerState:'OPEN'},remoteSourceHead:head,remoteTargetHead:base};
  step('Confirmed publication observation persists',{method:'savePublication',record:confirmed});
  const comment=(id:number,extra:any={})=>({id,created_on:'2026-09-28T00:00:00Z',updated_on:'2026-09-28T00:01:00Z',deleted:false,content:{raw:`Comment ${id}`},user:{display_name:'Reviewer'},...extra});
  const feedback=(comments:any[],extra:any={},observedAt=new Date(now).toISOString())=>({method:'savePublicationFeedback',runId:run.id,prId:9,read:{state:'complete',comments,...extra},observedAt});
  const comments=[comment(2),comment(1),comment(3,{created_on:'2026-09-27T23:59:00Z'})];
  const original=step('Feedback ordered by source creation and numeric ID',feedback(comments));assert.deepEqual(original.comments.map((c:any)=>c.id),[3,1,2]);
  step('Different PR identity refuses observation',{...feedback(comments),prId:10},true);
  step('Older feedback observation cannot replace current read',feedback([],{},'1970-01-01T00:00:01.000Z'));
  step('Same comment version may update metadata',feedback([comment(1,{user:{display_name:'Updated reviewer'}}),comments[0],comments[2]]));
  const missing=step('Missing comment remains known history with unknown currentness',feedback([comments[0]]));assert.equal(missing.comments.find((c:any)=>c.id===1).deleted,false);
  step('Earlier comment body cannot replace retained newer body',feedback([comment(1,{updated_on:'2026-09-28T00:00:59Z',content:{raw:'Old content'}})]));
  step('Same-version content conflict retains prior body',feedback([comment(1,{content:{raw:'Conflicting content'}})]));
  step('Same-version deletion conflict retains prior deletion flag',feedback([comment(1,{deleted:true})]));
  step('Unavailable comment read retains content',feedback([],{state:'unavailable',reason:'Provider unavailable'}));
  step('Newer confirmed deletion replaces the comment',feedback([comment(1,{updated_on:'2026-09-28T00:02:00+0000',deleted:true,content:undefined})]));
  step('Later comment revision may restore visible content',feedback([comment(1,{updated_on:'Mon, 28 Sep 2026 00:03:00 GMT',content:{raw:'Reopened'}})]));
  const latest=step('Capture current feedback',feedback([comment(1,{updated_on:'2026-09-29',content:{raw:'Current body'}})]));
  const without=step('Publication write without feedback retains latest observation',{method:'savePublication',record:confirmed});assert.deepEqual(without.feedback,latest);
  step('Publication write with stale feedback cannot overwrite later read',{method:'savePublication',record:{...confirmed,feedback:original}});
  const newer={...latest,observedAt:new Date(now+10000).toISOString(),reason:'Newer saved observation'};step('Newer feedback may accompany a publication update',{method:'savePublication',record:{...confirmed,feedback:newer}});
  step('Equal-time publication update preserves already-saved feedback',{method:'savePublication',record:{...confirmed,feedback:{...newer,comments:[]}}});
  step('Observe all publications',{method:'publications'});
  step('Agent session settles',{method:'recordAgentEvent',attemptId:coordinator.attempt.id,sequence:1,event:{type:'session_settled'}});
  const ci={...confirmed,observation:{pr:{id:9,state:'OPEN',source:{commit:{hash:head},branch:{name:confirmed.branch}},destination:{commit:{hash:base},branch:{name:'main'}}},statuses:[{key:'unit',state:'INPROGRESS',refname:confirmed.branch}],pipelines:[]}};
  step('Provider CI observation begins waiting only after session settles',{method:'savePublication',record:ci});
  assert.equal(legacy.getRun(run.id).activeSince,undefined);now+=100000;
  const failed={...ci,observation:{...ci.observation,statuses:[{key:'unit',state:'FAILED',refname:confirmed.branch}]}};
  step('Failed CI observation resumes accountable execution',{method:'savePublication',record:failed});
  assert.equal(legacy.getRun(run.id).activeSince,now-1000);
  const merged=step('Confirmed merge releases the coding slot atomically',{method:'savePublication',record:{...confirmed,merged:{commit:head,observedAt:new Date(now).toISOString()}}});
  step('Merged publication replay cannot re-release a slot',{method:'savePublication',record:merged});step('Coding slot is released',{method:'codingSlot'});
  step('No Jira update history yet',{method:'jiraUpdates'});
  const update={id:randomUUID(),runId:idle.id,attemptId:'fixture-attempt',siteUrl:source.siteUrl,issueId:'1',eventKey:'milestone',kind:'milestone',body:'Recorded progress',state:'pending',createdAt:new Date(now).toISOString()};
  step('Jira pending intent creates no delivery notice',{method:'saveJiraUpdate',update});step('Sending intent is not confirmed delivery',{method:'saveJiraUpdate',update:{...update,state:'sending'}});
  step('Unknown write records uncertain state and one notice',{method:'saveJiraUpdate',update:{...update,state:'uncertain'}});step('Uncertain replay deduplicates notice',{method:'saveJiraUpdate',update:{...update,state:'uncertain'}});
  step('Blocked Jira action remains explicit',{method:'saveJiraUpdate',update:{...update,state:'blocked',reason:'Provider refused'}});
  step('Confirmed Jira receipt captures external identity',{method:'saveJiraUpdate',update:{...update,state:'confirmed',externalId:'42'}});step('Confirmed receipt replay retains one notice',{method:'saveJiraUpdate',update:{...update,state:'confirmed',externalId:'42'}});
  step('Read notices for each retained disposition',{method:'deliveryNotices',runId:idle.id});step('Read update for its run',{method:'jiraUpdates',runId:idle.id});step('Other run has no Jira receipts',{method:'jiraUpdates',runId:run.id});
  step('No synchronization state before first attempt',{method:'jiraState'});
  const sync=(input:any[],retained:any[]=[],selected:any=source)=>({method:'commitJiraSync',source:selected,input,retained});
  const fail=(selected:any=source)=>({method:'failJiraSync',source:selected,error:'Fixture provider unavailable'});
  step('Initial unavailable sync records no implied imported source',fail());
  step('Read unavailable state',{method:'jiraState'});
  step('First complete sync retains explicit source order',sync([issue('2'),issue('1')]));
  step('Outage retains last complete source and sync time',fail());step('Recovery records a complete synchronization',sync([issue('1'),issue('2')]));
  step('Status-only source refresh does not change accepted requirements',sync([issue('1',undefined,{status:{id:'3',name:' In Progress '},updated:'2026-09-29T00:00:00.000+0000'}),issue('2')]));
  const statusTask=step('Read preserved local brief and new Jira status',{method:'getTask',taskId:initial[0].id});assert.equal(statusTask.brief,'Retain local decisions');assert.equal(statusTask.needsReacceptance,false);assert.equal(statusTask.jiraStatus,'In Progress');
  step('Changed source updates imports and pauses affected execution',sync([issue('1','Changed source',{updated:'Wed, 30 Sep 2026 00:00:00 GMT'}),issue('2')]));
  step('Retained issue refresh stays outside an empty filter',sync([], [issue('1','Changed outside filter',{updated:'2026-10-01'})]));
  const changed=step('Accepted brief survives leaving source filter',{method:'getTask',taskId:initial[0].id});assert.equal(changed.brief,'Retain local decisions');assert.equal(changed.needsReacceptance,true);
  step('Older retained issue cannot commit a sync',sync([], [issue('1','Stale',{updated:'2026-09-29T00:00:00Z'})]),true);
  step('Duplicate issues within filtered batch rejected',sync([issue('3'),issue('3')]),true);
  step('Overlap with retained issues rejected',sync([issue('3')],[issue('3')]),true);step('Duplicate retained issues rejected',sync([],[issue('3'),issue('3')]),true);
  for(const [label,extra] of [['empty title',{summary:' '}],['empty status',{status:{name:'\uFEFF'}}],['non-string timestamp',{updated:3}],['invalid timestamp',{updated:'bad-date'}],['invalid priority',{priority:{}}],['long priority',{priority:{name:'x'.repeat(81)}}],['null project',{project:null}],['long project',{project:{name:'x'.repeat(101)}}],['invalid status ID',{status:{name:'Open',id:3}}]] as const)step(`Jira refuses ${label}`,sync([issue('3',undefined,extra)]),true);
  step('Oversized sync batch rejected',sync(Array.from({length:1001},(_,i)=>issue(String(i+10)))),true);
  const padded=issue('3',' '.repeat(400)+'🙂'.repeat(300)+' ',{status:{name:'\uFEFF Open \uFEFF'},priority:null,project:{name:'  Project  '},description:{type:'doc',content:[{type:'paragraph',content:[{text:'Verified text'},{type:'hardBreak'},{text:'Next line'}]},{text:3,content:[{text:'Malformed parent must be discarded'}]}]}});
  step('Import normalizes Unicode limits and structured description correctly',sync([padded]));
  step('Same provider ID on another site creates distinct identity',sync([issue('3')],[],{...source,siteUrl:'https://other.atlassian.net'}));
  step('Changed sync source outage does not inherit old filter rows',fail(source));
  step('Save original source again',sync([issue('3')]));
  const reordered=Object.fromEntries(Object.entries(source).reverse());step('Source identity retains serialized-order contract',fail(reordered));
  for(const updated of ['2026-10-01T01:02:03.456Z','2026-10-01T01:02:03.456+0000','2026-10-01T03:02:03.456+02:00','Thu, 01 Oct 2026 01:02:03 GMT','2026-10-01'])step('Manual import accepts retained source date format',{method:'importJira',input:{siteUrl:source.siteUrl,issues:[issue(String(100+cases.length),undefined,{updated})]}});
  step('Manual empty import stays invalid',{method:'importJira',input:{siteUrl:source.siteUrl,issues:[]}},true);
  step('Manual source path remains invalid',{method:'importJira',input:{siteUrl:source.siteUrl+'/path',issues:[issue('999')]}},true);
  for(const suffix of ['?','#','?#'])step('Manual import normalizes empty URL delimiters '+suffix,{method:'importJira',input:{siteUrl:source.siteUrl+suffix,issues:[issue('3')]}});
  for(const suffix of ['?query=1','#fragment','?#fragment'])step('Manual import rejects nonempty URL components '+suffix,{method:'importJira',input:{siteUrl:source.siteUrl+suffix,issues:[issue('3')]}},true);
  step('Final sync read',{method:'jiraState'});
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps including legacy wall-clock calls',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Stored provider observations, coding receipts and Jira imports only; remote identities and process ownership are fixtures, with no provider calls'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
