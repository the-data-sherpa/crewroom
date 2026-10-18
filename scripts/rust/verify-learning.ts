/** N2.4c learning and role catalog oracle. Every native operation opens the database in a new process.
 * Process identities, publication observations and unported control states below
 * are explicit fixtures: this suite never starts agents or calls a provider. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { copyFileSync, existsSync, readFileSync, writeFileSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench, Conflict, NotFound } from '../../server/workbench';
import { defaultRoles, RoleCatalog, RoleConflict } from '../../server/roles';
import { Learning } from '../../server/learning';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n24c-')));
const legacyPath=join(root,'legacy.sqlite'), nativePath=join(root,'native.sqlite');
let now=1000, legacy=new Workbench(legacyPath,()=>now);
const legacyRolesPath=join(root,'legacy-roles.json'), nativeRolesPath=join(root,'native-roles.json');
const catalog=new RoleCatalog(legacyRolesPath);
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
  assert.equal(existsSync(nativeRolesPath),existsSync(legacyRolesPath),`${label}: catalog file existence`);
  if(existsSync(legacyRolesPath)){const expected=JSON.parse(readFileSync(legacyRolesPath,'utf8')),actual=JSON.parse(readFileSync(nativeRolesPath,'utf8'));remember(expected,actual);assert.deepEqual(fromNative(actual),expected,`${label}: persisted catalog`);}
  const expected=snapshot(legacyPath), native=snapshot(nativePath);
  // An existing derived-view difference from N2.1, never an ownership/timing field.
  for(const state of [expected,native])for(const row of state.runs)delete row.data.holdReason;
  remember(expected,native);const actual=fromNative(native);
  for(const table of Object.keys(expected))assert.deepEqual(actual[table],expected[table],`${label}: ${table}`);
}
function invoke(op:any):any {
  const s=legacy;
  switch(op.method) {
    case 'admitAssignment':return s.admitAssignment(op.runId,op.input);
    case 'learningProposals':return s.learningProposals();
    case 'getLearningProposal':return s.getLearningProposal(op.id);
    case 'proposeLearning':return s.proposeLearning(op.input);
    case 'decideLearning':return s.decideLearning(op.id,op.input);
    case 'completeLearningApproval':return s.completeLearningApproval(op.id,op.outcome);
    case 'learningPropose':return new Learning(s,catalog).propose(op.attemptId,op.toolCallId,op.input);
    case 'learningDecide':return new Learning(s,catalog).decide(op.id,op.input);
    case 'learningReconcile':return new Learning(s,catalog).reconcile();
    case 'rolesRead':return catalog.read();
    case 'rolesCreate':return catalog.create(op.input);
    case 'rolesUpdate':return catalog.update(op.id,op.input);
    default:throw new Error(`Unknown method ${op.method}`);
  }
}
function step(label:string,op:any,refused=false):any {
  protect(op);let expected:any, error:unknown;
  try {expected=invoke(op)??null;}catch(e){error=e;}
  assert.equal(!!error,refused,`${label}: TypeScript ${String(error)}`);
  const actual=JSON.parse(execFileSync(resolve('target/debug/examples/ledger-oracle'),[nativePath],{input:JSON.stringify(toNative({...op,now,rolePath:nativeRolesPath})),encoding:'utf8'}));
  assert.equal(actual.ok,!refused,`${label}: Rust ${JSON.stringify(actual)}`);
  if(error instanceof Conflict || error instanceof NotFound || error instanceof RoleConflict)assert.equal(actual.error,error.message,`${label}: refusal reason`);
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
  const repoPath=join(root,'repo');seedRepository(repoPath);const repo=saveRepositoryFixture(legacy,{name:'Learning oracle',path:repoPath,targetBranch:'main',checks:[]});
  const task=legacy.createTask({title:'Versioned guidance',description:'',repositoryId:repo.id});legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Keep evidence',criteria:['Retain decisions']});const run=legacy.queue(task.id,{revision:2});
  const freeTask=legacy.createTask({title:'No repository',description:''});const freeRun=legacy.queueRefinement(freeTask.id,{revision:1,purpose:'explore'});
  const role={...defaultRoles.find(r=>r.id==='coordinator')!,model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const member={name:'Coordinator',role,model,coordinator:true,input:'Inspect recorded evidence',brief:{criteria:[],dependsOn:[]}};
  const admitted=legacy.admitAssignment(run.id,member),freeAdmitted=legacy.admitAssignment(freeRun.id,member);
  const steering=legacy.steer(run.id,{requestId:'evidence',body:'Retain corrected check environment'}),freeSteering=legacy.steer(freeRun.id,{requestId:'evidence',body:'No repository'});
  const evidence=[{kind:'steering',id:steering.id}];
  const input=(change='Updated reviewer instructions',extra:any={})=>({target:{kind:'role',roleId:'reviewer'},change,rationale:'Recorded correction',evidence,...extra});
  const propose=(key:string,value=input(),attemptId=admitted.attempt.id)=>({method:'learningPropose',attemptId,toolCallId:key,input:value});
  const decide=(id:string,decision='approve',extra:any={})=>({method:'learningDecide',id,input:{decision,reason:' Evidence reviewed ',requestId:randomUUID(),...extra}});
  const roleUpdate=(id:string,changes:any={})=>({method:'rolesUpdate',id,input:{...catalog.read().find(r=>r.id===id)!,revision:catalog.read().find(r=>r.id===id)!.revision??1,...changes}});
  const byId=(method:string,id:string,extra:any={})=>({method,id,...extra});
  const roleFixture=(label:string,roles:any[])=>{for(const [path,map] of [[legacyRolesPath,(v:any)=>v],[nativeRolesPath,toNative]] as const)writeFileSync(path,JSON.stringify(map(roles)));compare(label);fixtures.push(label);};
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  step('Absent catalog returns defaults without writing',{method:'rolesRead'});
  for(const [label,changes] of [['foreign evidence',{evidence:[{kind:'steering',id:freeSteering.id}]}],['missing check',{evidence:[{kind:'check',id:'missing'}]}],['missing review',{evidence:[{kind:'review',id:'missing'}]}],['missing policy',{evidence:[{kind:'check_policy',id:'missing'}]}],['unknown mutation',{model:'forbidden'}],['empty evidence',{evidence:[]}],['invalid evidence kind',{evidence:[{kind:'model',id:'missing'}]}],['project with role ID',{target:{kind:'project',roleId:'reviewer'}}],['role without ID',{target:{kind:'role'}}],['missing role',{target:{kind:'role',roleId:'missing'}}],['unchanged instructions',{change:catalog.read().find(r=>r.id==='reviewer')!.instructions}]] as const)step(`Learning refuses ${label}`,propose(`invalid-${label}`,input(undefined,changes)),true);
  step('Project guidance requires configured repository',propose('no-repo',input('Project text',{target:{kind:'project'},evidence:[{kind:'steering',id:freeSteering.id}]}),freeAdmitted.attempt.id),true);
  const first=step('Capture proposed role revision with evidence',propose('first'));
  step('Proposal replay before decision',propose('first'));
  step('Same tool identity cannot change text',propose('first',input('Different')),true);
  const approval=decide(first.id);
  for(const requestId of ['invalid','550e8400e29b41d4a716446655440000','{550e8400-e29b-41d4-a716-446655440000}','urn:uuid:550e8400-e29b-41d4-a716-446655440000','550e8400-e29b-91d4-a716-446655440000','550e8400-e29b-41d4-c716-446655440000','FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF'])step('Decision refuses non-schema UUID spelling',{...approval,input:{...approval.input,requestId}},true);
  step('Unknown decision field refused',{...approval,input:{...approval.input,model:'other'}},true);
  step('Role approval writes instructions and provenance once',approval);
  assert.equal(catalog.read().find(r=>r.id==='reviewer')!.revision,2);
  assert.equal(legacy.runtime(run.id).assignments[0].role.instructions,role.instructions);
  step('Decision replay retains first reason and revision',{...approval,input:{...approval.input,reason:'Later reason'}});
  step('Different approval identity refused',decide(first.id),true);
  step('Rejecting approved change refused',decide(first.id,'reject'),true);
  step('Proposal replay after target changed',propose('first'));
  step('Explicit read of decided learning',byId('getLearningProposal',first.id));
  const stale=step('Capture next role proposal',propose('stale',input('Proposed stale text')));
  step('Operator edit creates new role revision',roleUpdate('reviewer',{instructions:'Operator edit',learningProposalId:stale.id}));
  assert.equal(catalog.read().find(r=>r.id==='reviewer')!.learningProposalId,undefined);
  step('Stale role approval refused',decide(stale.id),true);
  step('Stale role may be rejected',decide(stale.id,'reject',{requestId:'00000000-0000-0000-0000-000000000000'}));
  const interrupted=step('Proposal before interrupted role save',propose('interrupted',input('Approved before save')));
  step('Persist approval without pretending catalog is saved',byId('decideLearning',interrupted.id,{input:{requestId:randomUUID(),reason:'Reviewed',status:'approved',pendingRoleSave:true}}));
  step('Reconcile approved catalog write after restart',{method:'learningReconcile'});
  step('Reconcile replay does not increment role version',{method:'learningReconcile'});
  const afterSave=step('Proposal before lost completion receipt',propose('after-save',input('Already saved before restart')));
  step('Durable approval before file boundary',byId('decideLearning',afterSave.id,{input:{requestId:randomUUID(),reason:'Reviewed',status:'approved',pendingRoleSave:true}}));
  roleFixture('Approved catalog save fixture before SQLite completion',catalog.read().map(r=>r.id==='reviewer'?{...r,instructions:afterSave.change,revision:afterSave.baseVersion+1,learningProposalId:afterSave.id}:r));
  step('Reconcile recognizes proposal provenance',{method:'learningReconcile'});
  assert.equal(catalog.read().find(r=>r.id==='reviewer')!.revision,afterSave.baseVersion+1);
  const superseded=step('Proposal before competing write',propose('superseded',input('Do not overwrite later edit')));
  step('Approve before competing writer',byId('decideLearning',superseded.id,{input:{requestId:randomUUID(),reason:'Reviewed',status:'approved',pendingRoleSave:true}}));
  step('Competing operator role write',roleUpdate('reviewer',{instructions:'Retain competing edit'}));
  step('Reconcile supersedes changed role',{method:'learningReconcile'});
  step('Completed supersession cannot be rewritten',byId('completeLearningApproval',superseded.id,{outcome:{appliedVersion:99}}));
  const removed=step('Proposal targeting later removed custom role',propose('removed',input('Custom change',{target:{kind:'role',roleId:'tech-writer'}})));
  step('Approve before target removal',byId('decideLearning',removed.id,{input:{requestId:randomUUID(),reason:'Reviewed',status:'approved',pendingRoleSave:true}}));
  roleFixture('Remove proposed custom role fixture',catalog.read().filter(r=>r.id!=='tech-writer'));
  step('Reconcile missing role as not applied',{method:'learningReconcile'});
  const projectInput=input('Project guidance v1',{target:{kind:'project'}});
  const project=step('Project guidance proposal starts at version zero',propose('project',projectInput));
  const rival=step('Competing project proposal retains same base',propose('project-rival',input('Other project guidance',{target:{kind:'project'}})));
  const projectApproval=decide(project.id,'approve',{requestId:'ffffffff-ffff-ffff-ffff-ffffffffffff'});
  step('Project guidance and decision persist atomically',projectApproval);
  step('Project approval replay',projectApproval);
  step('Project proposal replay ignores later target version',propose('project',projectInput));
  step('Stale project approval cannot replace guidance',decide(rival.id),true);
  step('Project no-op proposal refused',propose('project-noop',projectInput),true);
  step('Reject stale project proposal',decide(rival.id,'reject',{requestId:'550E8400-E29B-41D4-A716-446655440000'}));
  step('Later assignment captures approved project guidance',{method:'admitAssignment',runId:run.id,input:{...member,name:'Reader',coordinator:false}});
  fixture('Recorded check review and policy evidence fixture',(db,map)=>db.prepare('INSERT INTO coding VALUES(?,?)').run(map(run.id),JSON.stringify(map({runId:run.id,checks:[{id:'recorded-check'}],reviews:[{id:'recorded-review'}],checkPolicyChanges:[{requestId:'recorded-policy'}]}))));
  const fullEvidence=[{kind:'check',id:'recorded-check'},{kind:'review',id:'recorded-review'},{kind:'check_policy',id:'recorded-policy'},...evidence];
  const allEvidence=step('Learning accepts recorded evidence from every supported kind',propose('all-evidence',input('Evidence-based text',{evidence:fullEvidence})));
  step('Changed evidence cannot reuse tool identity',propose('all-evidence',input('Evidence-based text',{evidence})),true);
  step('Lower-level proposal identity cannot change base', {method:'proposeLearning',input:{...allEvidence,baseVersion:99}},true);
  step('Read retained learning collection',{method:'learningProposals'});
  const savedRoles=catalog.read();
  roleFixture('Stored role normalization fixture',savedRoles.map(r=>({...r,name:`\uFEFF ${r.name} \uFEFF`,unknownField:'stripped only from reads'})));
  step('Stored catalog normalizes text and strips unknown fields',{method:'rolesRead'});
  step('Role update strips forged provenance and normalizes the catalog',roleUpdate('reviewer',{instructions:' Clean instructions ',learningProposalId:randomUUID(),extra:true}));
  step('Role edit cannot change ID',{method:'rolesUpdate',id:'reviewer',input:{...catalog.read().find(r=>r.id==='reviewer')!,id:'another'}},true);
  step('Role edit rejects stale revision',roleUpdate('reviewer',{revision:1}),true);
  step('Core role cannot be disabled',roleUpdate('reviewer',{enabled:false}),true);
  const custom={...defaultRoles[0],id:'custom-learning',name:' '.repeat(200)+'🙂'.repeat(100)+' '.repeat(200),model:' '.repeat(300),instructions:'\uFEFF Custom instructions \uFEFF',revision:99,learningProposalId:randomUUID(),extra:'ignore'};
  step('Create role trims before Unicode limits and ignores forged metadata',{method:'rolesCreate',input:custom});
  step('Duplicate custom role refused',{method:'rolesCreate',input:custom},true);
  step('Role preserves non-whitespace NEL',roleUpdate('custom-learning',{instructions:'\u0085Keep this\u0085'}));
  step('Unicode role field above code point limit refused',roleUpdate('custom-learning',{name:'🙂'.repeat(101)}),true);
  const validCatalog=catalog.read();
  roleFixture('Too-small saved catalog fixture',validCatalog.slice(0,2));step('Catalog minimum applies before default insertion',{method:'rolesRead'},true);
  roleFixture('Three-role saved catalog fixture',validCatalog.slice(0,3));step('Valid small catalog gains coordinator and reporter in memory',{method:'rolesRead'});
  roleFixture('Duplicate role fixture',[...validCatalog,validCatalog[0]]);step('Duplicate saved IDs refused without rewriting file',{method:'rolesRead'},true);
  roleFixture('Invalid provenance fixture',validCatalog.map(r=>r.id==='reviewer'?{...r,learningProposalId:'550e8400e29b41d4a716446655440000'}:r));step('Stored provenance uses strict UUID spelling',{method:'rolesRead'},true);
  roleFixture('Restore valid catalog fixture',validCatalog);step('Reopen valid catalog',{method:'rolesRead'});
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps including legacy wall-clock calls',reopen:'Rust process/store and role catalog reopened for every operation; TypeScript store reopened at seed and finish',claims:'Durable learning and real isolated role files; evidence records are explicit fixtures'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
