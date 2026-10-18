/** N2.5c continuation, repository edit and snapshot oracle. Every native operation opens the database in a new process.
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
import { requirementsHash } from '../../shared/requirements';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n25c-')));
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
    case 'snapshot':return s.snapshot();
    case 'repositoryWork':return s.repositoryWork(op.runId);
    case 'continuationReplay':return s.continuationReplay(op.runId,op.input);
    case 'continuationPublication':return s.continuationPublication(op.runId);
    case 'publicationSuperseded':return s.publicationSuperseded(op.runId,op.publicationId);
    case 'cancelledSource':return s.cancelledSource(op.runId,op.successorId);
    case 'continuationView':return s.continuationView(op.runId);
    case 'queueCancelledContinuation':return s.queueCancelledContinuation(op.runId,op.input,op.roles,op.expectedWork);
    case 'transferCancelledWork':return s.transferCancelledWork(op.runId,op.expectedWork,op.repository,op.checkpoint,op.continuationTargetRef);
    case 'repositoryUpdateReplay':return s.repositoryUpdateReplay(op.id,op.input);
    case 'updateRepository':return s.updateRepository(op.id,op.validated,op.input);
    case 'retireRepository':return s.retireRepository(op.id,op.input);
    case 'getRun':return s.getRun(op.runId);
    case 'cancelQueued':return s.cancelQueued(op.runId,op.input);
    case 'setQueueLaunch':return s.setQueueLaunch(op.runId,op.input,op.roles);
    case 'savePublication':return s.savePublication(op.record);
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
    if(op.method==='snapshot') {
      for(const field of ['activity','capabilities','connections'])delete actual.value[field];
      for(const repo of actual.value.repositories)delete repo.publicationAvailable;
    }
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
  const checkout=join(root,'repo');seedRepository(checkout);execFileSync('git',['-C',checkout,'branch','other']);
  const secondCheckout=join(root,'second');seedRepository(secondCheckout);execFileSync('git',['-C',secondCheckout,'branch','other']);
  const check=(id:string)=>({id,image:'sha256:'+'a'.repeat(64),command:'/bin/true',args:[],timeoutMs:1000,required:true});
  const repo=saveRepositoryFixture(legacy,{name:'Continuation fixture',path:checkout,targetBranch:'main',requestId:randomUUID(),checks:[check('unit'),check('unused')]});
  const other=saveRepositoryFixture(legacy,{name:'Other',path:secondCheckout,targetBranch:'main',checks:[]});
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const roles=defaultRoles;
  function seed(title:string,owned=true) {
    const task=legacy.createTask({title,description:'Keep history',repositoryId:repo.id});legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Continue accepted scope',criteria:['Preserve history']});
    const run=legacy.queue(task.id,{revision:2});
    const revision={base:'a'.repeat(40),head:'b'.repeat(40),tree:'c'.repeat(40),worktreeHash:'d'.repeat(64),dirty:true,requirementsHash:requirementsHash(legacy.getTask(task.id)),instructionsHash:'e'.repeat(64)};
    const checkpoint={id:randomUUID(),at:new Date(now).toISOString(),reason:'interrupted',revision};
    const work={runId:run.id,repository:repo,commonDir:join(checkout,'.git'),path:join(root,title),branch:'crewroom/'+title,targetRef:'refs/heads/main',mode:'new',base:revision.base,startHead:revision.head,requirementsHash:revision.requirementsHash,state:'ready',gitFile:join(checkout,'.git/worktrees',title),instructions:[],checkpoints:[checkpoint]};
    if(owned)legacy.reserveRepositoryWork(work as any);
    return {run,task,work,checkpoint};
  }
  const plain=seed('plain'),published=seed('published'),guard=seed('guard'),empty=seed('empty',false),queued=seed('queued',false),held=seed('held',false);
  const attempt=legacy.admitAssignment(guard.run.id,{name:'Coordinator',coordinator:true,role,model,input:'Fixture'});
  legacy.setQueueLaunch(queued.run.id,{requestId:randomUUID(),launch:{team:[{name:'Coordinator',roleId:'coordinator',coordinator:true,input:'Keep unit',criteria:[{index:0,obligation:'Keep the configured check',checkIds:['unit']}]}],repository:{mode:'new'}}},roles);
  const coding={runId:plain.run.id,checks:[],reviews:[],completions:[{id:'completed',state:'completed',summary:'Historical completed work'},{id:'rejected',state:'rejected',summary:'Rejected work'}]};legacy.updateCodingRecord(plain.run.id,()=>coding as any);legacy.updateCodingRecord(guard.run.id,()=>({...coding,runId:guard.run.id}) as any);
  const publication={id:randomUUID(),runId:published.run.id,candidateId:'fixture',revision:published.checkpoint.revision,createdAt:new Date(now).toISOString(),push:{state:'confirmed'},pr:{state:'confirmed',id:42,url:'https://bitbucket.org/fixture/repo/pull-requests/42',providerState:'OPEN'},ready:false,blockers:[]};legacy.savePublication(publication as any);
  // Set terminal and ownership observations explicitly; this suite does not claim process or Git proof.
  for(const f of [plain,published,guard,empty]) {
    const r=legacy.getRun(f.run.id);r.status='cancelled';r.desiredState='cancelled';delete r.position;
    legacy.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(r),r.id);
  }
  const holding=legacy.getRun(held.run.id);Object.assign(holding,{status:'paused',desiredState:'paused',schedule:{id:randomUUID(),at:new Date(now).toISOString()},slotRelease:{reason:'review',at:new Date(now).toISOString(),candidateId:'old'},sourceConflict:true});delete holding.position;
  legacy.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(holding),holding.id);
  const q=legacy.getRun(queued.run.id);q.position=0;legacy.db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(q),q.id);
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  const source=(id:string,refused=false)=>step('Inspect cancelled source',{method:'cancelledSource',runId:id},refused);
  const request=(f=plain,extra:any={})=>({requestId:randomUUID(),revision:legacy.getTask(f.task.id).revision,roleId:'coordinator',...extra});
  const queue=(f:any,input:any,work=legacy.repositoryWork(f.run.id),selected=roles)=>({method:'queueCancelledContinuation',runId:f.run.id,input,expectedWork:work,roles:selected});
  const view=(id:string)=>step('Continuation availability reflects durable facts',{method:'continuationView',runId:id});
  step('Missing repository work run rejected',{method:'repositoryWork',runId:'missing'},true);step('No reserved work remains absent',{method:'repositoryWork',runId:empty.run.id});
  view(empty.run.id);source(empty.run.id,true);source(queued.run.id,true);source(plain.run.id);source(published.run.id);source(guard.run.id);
  step('Initial full snapshot projects histories, pending holds and continuation reasons',{method:'snapshot'});
  patch('runs',held.run.id,{sourceConflict:false});step('False source-conflict flag does not project a readiness hold',{method:'snapshot'});patch('runs',held.run.id,{sourceConflict:true});
  fixture('Fixture slot ownership for readiness hold',(db,map)=>db.prepare('UPDATE coding_slot SET run_id=? WHERE id=1').run(map(held.run.id)));
  step('Full snapshot distinguishes held readiness from pending re-hold',{method:'snapshot'});
  fixture('Release fixture slot',(db)=>db.prepare('UPDATE coding_slot SET run_id=NULL WHERE id=1').run());
  for(const changes of [{status:'paused'},{purpose:'research'},{desiredState:'paused'},{continuedBy:'other-successor'}]) {patch('runs',guard.run.id,changes);source(guard.run.id,true);view(guard.run.id);patch('runs',guard.run.id,{status:'cancelled',purpose:'coding',desiredState:'cancelled',continuedBy:undefined});}
  for(const changes of [{state:'preparing'},{checkpoints:[]},{transferredTo:'other-owner'},{writer:{attemptId:attempt.attempt.id}}]) {patch('repository_work',guard.run.id,changes);source(guard.run.id,true);patch('repository_work',guard.run.id,{state:'ready',checkpoints:guard.work.checkpoints,transferredTo:undefined,writer:undefined});}
  patch('attempts',attempt.attempt.id,{state:'claimed',ownership:'uncertain'});source(guard.run.id,true);patch('attempts',attempt.attempt.id,{state:'exited',ownership:'stopped'});source(guard.run.id);
  for(const refresh of [{state:'rebasing'},{state:'conflicted'},{state:'completed',resolutionWorkspace:{legacyOwnershipUnknown:true}},...['preparing','continuing','promoting'].map(state=>({state:'completed',resolutionWorkspace:{state}}))]) {patch('repository_work',guard.run.id,{refreshes:[refresh]});source(guard.run.id,true);}
  patch('repository_work',guard.run.id,{refreshes:[{state:'conflicted',supersededBy:{requestId:'later'},resolutionWorkspace:{state:'operator_required'}}]});source(guard.run.id);patch('repository_work',guard.run.id,{refreshes:undefined});
  for(const changes of [{edits:[{state:'pending'}]},{completions:[{state:'requested'}]},{checkOperations:[{state:'running'}]}]) {patch('coding',guard.run.id,changes);source(guard.run.id,true);patch('coding',guard.run.id,{edits:[],completions:[],checkOperations:[]});}
  patch('coding',guard.run.id,{checkOperations:[{state:'recorded'},{state:'rejected'}]});source(guard.run.id);
  for(const changes of [{merged:{commit:'x'}},{pr:{...publication.pr,providerState:'DECLINED'}},{pr:{...publication.pr,state:'uncertain'}},{pr:{...publication.pr,id:undefined}},{push:{state:'pending'}}]) {patch('publications',published.run.id,changes);source(published.run.id,true);view(published.run.id);patch('publications',published.run.id,{merged:undefined,pr:publication.pr,push:publication.push});}
  patch('repositories',repo.id,{retiredAt:'fixture'});source(plain.run.id,true);patch('repositories',repo.id,{retiredAt:undefined});
  patch('repository_work',guard.run.id,{repository:{...repo,targetBranch:'other'}});source(guard.run.id,true);patch('repository_work',guard.run.id,{repository:repo});
  const edit=(extra:any={},revision=legacy.snapshot().repositories.find(r=>r.id===repo.id)!.revision)=>{const input={name:' Edited ',path:checkout,targetBranch:'main',checks:[check('unit'),check('unused')],revision,requestId:randomUUID(),...extra};const {revision:_revision,...validated}=input;return {method:'updateRepository',id:repo.id,input,validated};};
  const savedEdit=edit({branchPrefix:'next/'});step('Repository edit preserves original identity and permits open-work metadata changes',savedEdit);
  step('Repository edit keyed replay',savedEdit);step('Repository edit replay endpoint',{method:'repositoryUpdateReplay',id:repo.id,input:savedEdit.input});
  step('Changed repository replay refused',{...savedEdit,input:{...savedEdit.input,name:'Changed'}},true);
  step('Stale repository edit refused',edit({},1),true);
  step('Referenced target change refused',edit({targetBranch:'other'}),true);step('Referenced path change refused',edit({path:secondCheckout,targetBranch:'other'},2),true);
  step('Referenced check removal refused',edit({checks:[check('unused')]}),true);
  step('Unused configured check can be removed',edit({checks:[check('unit')]}));
  step('Duplicate active repository target refused',edit({path:secondCheckout,checks:[]}),true);
  step('Repository retirement refuses open referenced issue keys',{method:'retireRepository',id:repo.id,input:{requestId:randomUUID(),revision:3}},true);
  for(const extra of [{unknown:true},{revision:0},{revision:Number.MAX_SAFE_INTEGER+1},{name:' '},{name:'🙂'.repeat(121)},{requestId:'{'+randomUUID()+'}'},{checks:[check('unit'),check('unit')]}]) step('Repository edit schema rejects invalid input',edit(extra),true);
  const normalized=edit({name:' '.repeat(200)+'🙂'.repeat(120)+'\uFEFF',checks:[Object.fromEntries(Object.entries(check('unit')).reverse())]});step('Repository schema trims before Unicode limits and orders check fields',normalized);
  step('Owned snapshots stay at original configuration revision',{method:'snapshot'});
  for(const bad of [{unknown:true},{revision:0},{revision:Number.MAX_SAFE_INTEGER+1},{roleId:''},{requestId:'bad'}])step('Continuation request schema refused',queue(plain,request(plain,bad)),true);
  step('Continuation expected-work change refused',queue(plain,request(),{...plain.work,state:'preparing'}),true);
  step('Continuation expected-work serialized ordering remains identity',queue(plain,request(),Object.fromEntries(Object.entries(plain.work).reverse())),true);
  step('Continuation stale accepted revision refused',queue(plain,request(plain,{revision:1})),true);
  step('Continuation role must remain available',queue(plain,request(plain,{roleId:'missing'})),true);
  const input=request();const next=step('Queue successor preserves cancelled source and completed summary',queue(plain,input));
  assert.equal(next.status,'awaiting_runner');assert.equal(legacy.repositoryWork(next.id),undefined);assert.deepEqual(JSON.parse(next.launch.team[0].input).previousCompletions,['Historical completed work']);
  step('Continuation queue replay after process reopen',queue(plain,input));step('Continuation replay lookup',{method:'continuationReplay',runId:plain.run.id,input});
  step('Continuation replay cannot change selected role',queue(plain,{...input,roleId:'reviewer'}),true);step('Another successor refused',queue(plain,request()),true);view(plain.run.id);
  step('Cancel successor before transfer releases predecessor link',{method:'cancelQueued',runId:next.id,input:{requestId:randomUUID()}});
  step('Old continuation receipt remains historical after cancellation',queue(plain,input));source(plain.run.id);
  const successor=step('New successor may replace cancelled unadmitted successor',queue(plain,request()));
  const currentRepo=legacy.snapshot().repositories.find(r=>r.id===repo.id)!;
  const transfer=(id:string,expectedWork:any,repository=currentRepo,extra:any={})=>({method:'transferCancelledWork',runId:id,expectedWork,repository,checkpoint:{...plain.checkpoint,id:randomUUID(),at:new Date(now).toISOString(),reason:'prepared'},...extra});
  step('Cancelled successor cannot take branch ownership',transfer(next.id,plain.work),true);
  step('Transfer refuses changed captured repository',transfer(successor.id,plain.work,repo),true);
  step('Transfer refuses changed retained work',transfer(successor.id,{...plain.work,base:'changed'}),true);
  patch('runs',successor.id,{desiredState:'paused'});step('Pause supersedes queued transfer',transfer(successor.id,plain.work),true);patch('runs',successor.id,{desiredState:undefined});
  const moved=step('Atomic ownership transfer keeps history and pins old base before refresh',transfer(successor.id,plain.work,currentRepo,{continuationTargetRef:'refs/crewroom/new-target'}));
  assert.equal(moved.targetRef,plain.work.base);assert.equal(moved.requirementsHash,requirementsHash(successor.taskSnapshot));assert.equal(legacy.codingRecord(successor.id),undefined);
  step('Transfer replay with current predecessor receipt returns original ownership',transfer(successor.id,legacy.repositoryWork(plain.run.id)));
  step('Transfer replay with stale predecessor receipt refuses',transfer(successor.id,plain.work),true);
  step('Linked successor cannot change retained branch',{method:'setQueueLaunch',runId:successor.id,roles,input:{requestId:randomUUID(),launch:{...successor.launch,repository:{mode:'new'}}}},true);
  step('Full snapshot preserves cancelled and successor histories',{method:'snapshot'});
  const first=step('Published cancellation captures immutable PR lineage',queue(published,request(published)));
  step('Publication unavailable until verified ownership transfer',{method:'continuationPublication',runId:first.id},true);
  step('Published branch transfer',transfer(first.id,published.work));
  step('Transferred successor locates predecessor PR',{method:'continuationPublication',runId:first.id});
  step('Predecessor is not superseded before successor publication',{method:'publicationSuperseded',runId:published.run.id,publicationId:publication.id});
  patch('runs',first.id,{status:'cancelled',desiredState:'cancelled',position:undefined});
  const middle={run:{id:first.id},task:published.task};const last=step('Second cancellation retains original publication lineage',queue(middle,request(published)));
  step('Second transfer',transfer(last.id,legacy.repositoryWork(first.id)));
  step('Publication lookup traverses two verified handoffs',{method:'continuationPublication',runId:last.id});
  patch('runs',first.id,{continuedBy:'wrong'});step('Broken handoff refuses publication lineage',{method:'continuationPublication',runId:last.id},true);step('Broken handoff is not a superseded publication',{method:'publicationSuperseded',runId:published.run.id,publicationId:publication.id},true);patch('runs',first.id,{continuedBy:last.id});
  patch('publications',published.run.id,{id:'different'});step('Changed predecessor PR identity refused',{method:'continuationPublication',runId:last.id},true);patch('publications',published.run.id,{id:publication.id});
  step('Successor publishes same PR identity as a fixture observation',{method:'savePublication',record:{...publication,runId:last.id}});
  step('Own publication wins after current observation',{method:'continuationPublication',runId:last.id});step('Both predecessors see superseded publication',{method:'publicationSuperseded',runId:published.run.id,publicationId:publication.id});step('Intermediate publication superseded',{method:'publicationSuperseded',runId:first.id,publicationId:publication.id});
  const savedLink=legacy.getRun(successor.id).continuedFrom;
  patch('runs',successor.id,{continuedFrom:{...savedLink,publicationId:publication.id,prId:42}});
  step('Missing predecessor publication remains held',{method:'continuationPublication',runId:successor.id},true);
  patch('runs',plain.run.id,{continuedFrom:{runId:successor.id,publicationId:publication.id,prId:42}});
  patch('runs',successor.id,{continuedBy:plain.run.id});patch('repository_work',successor.id,{transferredTo:plain.run.id});
  step('Publication cycle is explicitly refused',{method:'continuationPublication',runId:successor.id},true);
  step('Superseded lookup terminates at a cycle',{method:'publicationSuperseded',runId:plain.run.id,publicationId:publication.id});
  patch('runs',plain.run.id,{continuedFrom:undefined});patch('runs',successor.id,{continuedFrom:savedLink,continuedBy:undefined});patch('repository_work',successor.id,{transferredTo:undefined});
  const retire={method:'retireRepository',id:other.id,input:{revision:1,requestId:randomUUID()}};step('Unreferenced repository retirement keeps history',retire);step('Retirement replay retains exact controlled timestamp',retire);
  step('Changed retirement replay refused',{...retire,input:{...retire.input,revision:2}},true);
  const otherEdit={method:'updateRepository',id:other.id,input:{name:'Retired',path:secondCheckout,targetBranch:'main',revision:2,requestId:randomUUID()},validated:{name:'Retired',path:secondCheckout,targetBranch:'main'}};step('Retired repository cannot be edited',otherEdit,true);
  step('Final complete projections',{method:'snapshot'});
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Durable continuation, repository edits and projections only; Git worktrees, checkpoints, process ownership and PR observations are explicit fixtures. Real disposable Git checkouts validate repository edit preflight. Native-only unavailable capability hints are excluded from snapshot comparisons.'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
