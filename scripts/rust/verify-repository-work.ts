/** N2.6 repository ownership and target recovery oracle. Every native operation opens the database in a new process.
 * Process identities, publication observations and unported control states below
 * are explicit fixtures: this suite never starts agents or calls a provider. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID, createHash } from 'node:crypto';
import { copyFileSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench, Conflict, NotFound, RepositoryConfigurationChanged } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { requirementsHash } from '../../shared/requirements';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n26-')));
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
    case 'reserveRepositoryWork':return s.reserveRepositoryWork(op.record);
    case 'confirmRepositoryWork':return s.confirmRepositoryWork(op.runId,op.gitFile,op.instructions,op.checkpoint);
    case 'claimRepositoryWriter':return s.claimRepositoryWriter(op.attemptId,op.checkpoint,op.claim);
    case 'releaseRepositoryWriter':return s.releaseRepositoryWriter(op.attemptId,op.checkpoint);
    case 'checkpointRepository':return s.checkpointRepository(op.runId,op.checkpoint);
    case 'preflightTargetRefresh':return s.preflightTargetRefresh(op.runId,op.supersedes);
    case 'beginTargetRefresh':return s.beginTargetRefresh(op.runId,op.refresh);
    case 'recordTargetRefreshWorkspace':return s.recordTargetRefreshWorkspace(op.runId,op.requestId,op.workspace,op.conflicts);
    case 'beginTargetConflictContinuation':return s.beginTargetConflictContinuation(op.runId,op.requestId,op.identity);
    case 'finishTargetConflictContinuation':return s.finishTargetConflictContinuation(op.runId,op.requestId,op.identity,op.result);
    case 'beginTargetPromotion':return s.beginTargetPromotion(op.runId,op.requestId,op.operationId,op.toHead,op.process);
    case 'rejectUnpromotedConflict':return s.rejectUnpromotedConflict(op.runId,op.requestId,op.operationId,op.proof);
    case 'interruptUnpromotedConflict':return s.interruptUnpromotedConflict(op.runId,op.requestId,op.operationId,op.proof);
    case 'recordTargetRebaseRecovery':return s.recordTargetRebaseRecovery(op.runId,op.requestId,op.stage);
    case 'finishTargetRefresh':return s.finishTargetRefresh(op.runId,op.requestId,op.outcome,op.checkpoint);
    case 'repositoryWork':return s.repositoryWork(op.runId);
    case 'grantRepairCycle':return s.grantRepairCycle(op.runId,op.assignmentId,op.input);
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
  assert.equal(actual.repositoryConfigurationChanged??false,error instanceof RepositoryConfigurationChanged,`${label}: retryable configuration disposition`);
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
  const checkout=join(root,'repo');seedRepository(checkout);const repo=saveRepositoryFixture(legacy,{name:'Ownership oracle',path:checkout,targetBranch:'main',checks:[]});
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  function seed(name:string) {
    const task=legacy.createTask({title:name,description:'',repositoryId:repo.id});legacy.refine(task.id,{revision:1,sourceRevision:1,brief:'Keep exact history',criteria:['Prove work']});const run=legacy.queue(task.id,{revision:2});
    const revision={base:'a'.repeat(40),head:'b'.repeat(40),tree:'c'.repeat(40),worktreeHash:'d'.repeat(64),dirty:false,requirementsHash:requirementsHash(legacy.getTask(task.id)),instructionsHash:createHash('sha256').update('[]').digest('hex')};
    const checkpoint={id:randomUUID(),at:new Date(now).toISOString(),reason:'prepared',revision};
    const work={runId:run.id,repository:repo,commonDir:join(checkout,'.git'),path:join(root,name),branch:'crewroom/'+name,targetRef:'refs/heads/main',mode:'new',base:revision.base,startHead:revision.head,requirementsHash:revision.requirementsHash,state:'preparing',instructions:[],checkpoints:[]};
    const candidate={id:randomUUID(),runId:run.id,attemptId:'fixture-author',revision,createdAt:new Date(now).toISOString()};
    legacy.updateCodingRecord(run.id,()=>({runId:run.id,requiredChecks:[],candidates:[candidate],checks:[],reviews:[],completions:[],claims:[]}) as any);
    const members=['Coordinator','Implementer','Peer'].map((name,i)=>legacy.admitAssignment(run.id,{name,coordinator:i===0,role,model,input:'Fixture responsibility'}));
    return {run,task,revision,checkpoint,work,candidate,members,coordinator:members[0],writer:members[1],peer:members[2]};
  }
  const main=seed('main'),other=seed('other'),target=seed('target');
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  const reserve=(record:any)=>({method:'reserveRepositoryWork',record});
  const confirm=(f:any,extra:any={})=>({method:'confirmRepositoryWork',runId:f.run.id,gitFile:join(checkout,'.git/worktrees',f.run.id),instructions:[],checkpoint:f.checkpoint,...extra});
  const active=(f:any,status='running',desiredState='running')=>{patch('runs',f.run.id,{status,desiredState,position:undefined});for(const member of f.members)patch('attempts',member.attempt.id,{state:status==='running'?'idle':'exited',ownership:status==='running'?'held':'stopped'});};
  const claim=(member=main.writer,extra:any={})=>({id:randomUUID(),coordinatorAttemptId:([main,other,target].find(f=>f.members.some(m=>m.attempt.id===member.attempt.id))??main).coordinator.attempt.id,toolCallId:randomUUID(),writerAttemptId:member.attempt.id,instruction:'Implement accepted work',automatic:false,...extra});
  const writer=(f=main,member=f.writer,c:any=undefined,checkpoint=f.checkpoint)=>({method:'claimRepositoryWriter',attemptId:member.attempt.id,checkpoint,...(c?{claim:c}:{})});
  const release=(f=main,member=f.writer)=>({method:'releaseRepositoryWriter',attemptId:member.attempt.id,checkpoint:{...f.checkpoint,id:randomUUID(),reason:'interrupted'}});
  step('Missing run cannot reserve a branch',reserve({...main.work,runId:'missing'}),true);
  step('Foreign selected repository refused',reserve({...main.work,repository:other.work}),true);
  patch('runs',main.run.id,{purpose:'research'});step('Only coding run can reserve ownership',reserve(main.work),true);patch('runs',main.run.id,{purpose:'coding'});
  for(const desiredState of ['paused','cancelled']) {patch('runs',main.run.id,{desiredState});step('Newer control refuses repository reservation',reserve(main.work),true);}patch('runs',main.run.id,{desiredState:undefined});
  patch('attempts',main.writer.attempt.id,{state:'starting'});step('Attempt execution supersedes repository reservation',reserve(main.work),true);patch('attempts',main.writer.attempt.id,{state:'admitted'});
  step('Stale repository produces retryable configuration disposition',reserve({...main.work,repository:{...repo,revision:0}}),true);
  patch('repositories',repo.id,{retiredAt:'fixture'});step('Retired repository produces retryable configuration disposition',reserve(main.work),true);patch('repositories',repo.id,{retiredAt:undefined});
  step('Reserve owned branch',reserve(main.work));step('Same run cannot reserve twice',reserve(main.work),true);step('Another run cannot take current branch',reserve({...other.work,branch:main.work.branch}),true);
  step('Unreserved work cannot be confirmed',confirm(other),true);step('Confirm instructions and first checkpoint',confirm(main));step('Confirmation replay keeps original snapshot',confirm(main,{gitFile:'changed'}));
  step('Reserve independent target branch',reserve(target.work));step('Confirm target workspace',confirm(target));step('Reserve independent other branch',reserve(other.work));step('Confirm other workspace',confirm(other));
  step('Unstarted specialist cannot become writer',writer(),true);active(main);
  step('Coordinator cannot become writer',writer(main,main.coordinator),true);
  patch('assignments',main.writer.assignment.id,{rebriefRequired:true});step('Changed accepted scope blocks writer',writer(),true);patch('assignments',main.writer.assignment.id,{rebriefRequired:undefined});
  patch('assignments',main.writer.assignment.id,{retiredAt:'fixture'});step('Retired specialist cannot become writer',writer(),true);patch('assignments',main.writer.assignment.id,{retiredAt:undefined});
  const proposalId=randomUUID();fixture('Pending retirement fixture',(db,map)=>db.prepare('INSERT INTO team_proposals VALUES(?,?,NULL,?,?)').run(proposalId,map(main.run.id),'retire-fixture',JSON.stringify(map({id:proposalId,runId:main.run.id,status:'pending',retirements:[{assignmentId:main.writer.assignment.id,reason:'Retire'}]}))));
  step('Pending retirement blocks writer',writer(),true);fixture('Resolve pending retirement fixture',db=>db.prepare('DELETE FROM team_proposals WHERE id=?').run(proposalId));
  step('Wrong implementation claim owner refused',writer(main,main.writer,claim(main.peer)),true);
  const grant=claim(main.writer,{automatic:true,candidateId:main.candidate.id});step('Writer grant atomically records repair counter claim and checkpoint',writer(main,main.writer,grant));step('Same writer replay consumes no extra repair cycle',writer(main,main.writer,grant));
  step('Competing writer refused',writer(main,main.peer),true);step('Foreign writer cannot release ownership',release(main,main.peer),true);step('Held process cannot release writer',release(),true);
  step('Live writer cannot checkpoint completion',{method:'checkpointRepository',runId:main.run.id,checkpoint:main.checkpoint},true);
  patch('attempts',main.writer.attempt.id,{state:'exited',ownership:'stopped'});step('Stopped writer releases and retains interrupted checkpoint',release());step('Repeated writer release is not a second checkpoint',release(),true);
  patch('attempts',main.writer.attempt.id,{state:'idle',ownership:'held'});step('Second automatic repair grant',writer(main,main.writer,claim(main.writer,{automatic:true})));
  patch('attempts',main.writer.attempt.id,{state:'exited',ownership:'stopped'});step('Second stopped release',release());patch('attempts',main.writer.attempt.id,{state:'idle',ownership:'held'});
  step('Two automatic cycles exhausted',writer(main,main.writer,claim(main.writer,{automatic:true})),true);
  step('Explicit allowance extends repair budget',{method:'grantRepairCycle',runId:main.run.id,assignmentId:main.writer.assignment.id,input:{requestId:randomUUID(),reason:'One more attempt'}});
  step('Explicit extra repair allowance can be consumed',writer(main,main.writer,claim(main.writer,{automatic:true})));
  patch('attempts',main.writer.attempt.id,{state:'exited',ownership:'stopped'});step('Release third cycle',release());patch('attempts',main.writer.attempt.id,{state:'idle',ownership:'held'});step('Extended repair budget exhaustion stays explicit',writer(main,main.writer,claim(main.writer,{automatic:true})),true);
  step('Manual writer claim does not consume automatic budget',writer(main,main.writer,claim()));patch('attempts',main.writer.attempt.id,{state:'exited',ownership:'stopped'});step('Release manual writer',release());active(main,'paused','paused');
  step('All stopped attempts permit completed checkpoint',{method:'checkpointRepository',runId:main.run.id,checkpoint:{...main.checkpoint,reason:'completed'}});
  const publication={id:randomUUID(),runId:main.run.id,candidateId:main.candidate.id,repositoryPath:repo.path,repository:{uuid:'fixture-repo'},branch:main.work.branch,push:{state:'confirmed'},pr:{id:42,state:'confirmed',providerState:'OPEN'},ready:false};
  fixture('Publication ownership fixture',(db,map)=>db.prepare('INSERT INTO publications VALUES(?,?)').run(map(main.run.id),JSON.stringify(map(publication))));active(main);
  step('Publication prevents an unscoped new writer',writer(),true);
  const feedback={id:randomUUID(),state:'starting',sourceCandidateId:main.candidate.id,sourceHead:main.revision.head,publicationId:publication.id,prId:42,repository:{uuid:'fixture-repo'}};patch('runs',main.run.id,{feedbackContinuations:[feedback]});
  const feedbackClaim=claim(main.writer,{candidateId:main.candidate.id,feedbackContinuationId:feedback.id});
  step('Changed feedback checkpoint cannot claim writer',writer(main,main.writer,feedbackClaim,{...main.checkpoint,revision:{...main.revision,tree:'different'}}),true);
  for(const changes of [{push:{state:'uncertain'}},{pr:{...publication.pr,state:'blocked'}},{merged:{commit:'fixture'}},{repository:{uuid:'different'}},{candidateId:'different'},{repositoryPath:'different'},{branch:'different'}]) {patch('publications',main.run.id,changes);step('Unreconciled publication identity prevents feedback writer',writer(main,main.writer,feedbackClaim),true);patch('publications',main.run.id,{...publication,merged:undefined});}
  step('Feedback checkpoint serialization must match the captured revision',writer(main,main.writer,feedbackClaim,{...main.checkpoint,revision:Object.fromEntries(Object.entries(main.revision).reverse())}),true);
  step('Explicit matching PR feedback can claim writer',writer(main,main.writer,feedbackClaim));
  patch('attempts',main.writer.attempt.id,{state:'exited',ownership:'stopped'});step('Stopped feedback writer retains interrupted grant',release());
  // A replacement process identity is a fixture; the store checks its recorded predecessor.
  const replacementId=randomUUID();fixture('Stopped feedback writer replacement fixture',(db,map)=>{const prior=legacy.getAttempt(main.writer.attempt.id);const next=map({...prior,id:replacementId,previousAttemptId:prior.id,generation:2,state:'idle',ownership:'held'});db.prepare('INSERT INTO attempts VALUES(?,?,?)').run(next.id,next.assignmentId,JSON.stringify(next));const assignment=JSON.parse(String(db.prepare('SELECT data FROM assignments WHERE id=?').get(map(main.writer.assignment.id))!.data));assignment.currentAttemptId=replacementId;db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(assignment),assignment.id);});
  const replacement={attempt:{id:replacementId}} as any;const restored=claim(replacement,{candidateId:main.candidate.id,feedbackContinuationId:feedback.id,restoredFrom:main.writer.attempt.id});
  step('Changed restored instruction refuses writer',writer(main,replacement,{...restored,instruction:'Changed'}),true);step('Stopped same-assignment feedback grant can be restored',writer(main,replacement,restored));
  patch('attempts',replacementId,{state:'exited',ownership:'stopped'});step('Release restored writer',release(main,replacement));
  const child={...main.candidate,id:randomUUID(),revision:{...main.revision,head:'2'.repeat(40)},feedbackFrom:{continuationId:feedback.id}};
  patch('coding',main.run.id,{candidates:[main.candidate,child]});patch('runs',main.run.id,{feedbackContinuations:[{...feedback,state:'candidate',candidateId:child.id}]});patch('attempts',replacementId,{state:'idle',ownership:'held'});
  const childClaim=claim(replacement,{candidateId:child.id,feedbackContinuationId:feedback.id});const childCheckpoint={...main.checkpoint,revision:child.revision};
  patch('runs',main.run.id,{feedbackContinuations:[{...feedback,state:'starting',candidateId:child.id}]});step('Feedback handoff cannot use a different source candidate',writer(main,replacement,childClaim,childCheckpoint),true);
  patch('runs',main.run.id,{feedbackContinuations:[{...feedback,state:'candidate',candidateId:child.id}]});step('Linked successor feedback candidate may continue writing',writer(main,replacement,childClaim,childCheckpoint));patch('attempts',replacementId,{state:'exited',ownership:'stopped'});step('Release successor candidate writer',release(main,replacement));
  active(target,'paused','paused');
  const pre=(extra:any={})=>({method:'preflightTargetRefresh',runId:target.run.id,...extra});
  step('Stopped target preflight succeeds',pre());patch('runs',target.run.id,{status:'running'});step('Executing target refresh refused',pre(),true);patch('runs',target.run.id,{status:'paused'});
  patch('coding',target.run.id,{completions:[{state:'requested'}]});step('Pending completion blocks target preflight',pre(),true);patch('coding',target.run.id,{completions:[]});
  fixture('Uncertain target publication fixture',(db,map)=>db.prepare('INSERT INTO publications VALUES(?,?)').run(map(target.run.id),JSON.stringify(map({...publication,runId:target.run.id}))));
  for(const field of ['push','pr'])for(const state of ['sending','uncertain','blocked']) {patch('publications',target.run.id,{[field]:{...(publication as any)[field],state}});step('Uncertain provider write blocks target preflight',pre(),true);patch('publications',target.run.id,{[field]:(publication as any)[field]});}
  fixture('Remove target publication fixture',(db,map)=>db.prepare('DELETE FROM publications WHERE run_id=?').run(map(target.run.id)));
  step('Stale reasoned retry without hold refused',pre({supersedes:{requestId:'missing',reason:'Retry'}}),true);
  const refresh=(extra:any={})=>({requestId:randomUUID(),at:new Date(now).toISOString(),fromBase:target.revision.base,fromHead:target.revision.head,targetRef:'refs/crewroom/target',toBase:'e'.repeat(40),sourceCandidateId:target.candidate.id,state:'rebasing',...extra});
  const begin=(value:any)=>({method:'beginTargetRefresh',runId:target.run.id,refresh:value});
  step('Changed refresh source refused',begin(refresh({sourceCandidateId:'other'})),true);step('Incorrect unchanged-target receipt refused',begin(refresh({state:'completed',toBase:target.revision.base,toHead:target.revision.head,candidateId:'other'})),true);
  patch('repository_work',target.run.id,{continuationTargetRef:'refs/continuation-target'});const noop=refresh({state:'completed',toBase:target.revision.base,toHead:target.revision.head,candidateId:target.candidate.id});step('Verified unchanged target clears continuation pin',begin(noop));step('Repeated unchanged request refused',begin(noop),true);
  step('Completed refresh candidate association remains immutable',{method:'finishTargetRefresh',runId:target.run.id,requestId:noop.requestId,outcome:{state:'completed',candidateId:'different'}});
  const rebasing=refresh();step('Record target rebase intent',begin(rebasing));step('Concurrent target refresh refused',pre(),true);
  const recover=(stage:string)=>({method:'recordTargetRebaseRecovery',runId:target.run.id,requestId:rebasing.requestId,stage});
  const finish=(outcome:any,checkpoint:any=undefined)=>({method:'finishTargetRefresh',runId:target.run.id,requestId:rebasing.requestId,outcome,...(checkpoint?{checkpoint}:{})});
  step('Cannot undo a rebase without stopped receipt',recover('undo_started'),true);step('Cannot restore before undo',recover('restored'),true);step('Interrupted refresh cannot omit clean rollback proof',finish({state:'interrupted'}),true);
  step('Completed refresh cannot omit clean checkpoint',finish({state:'completed',toHead:'f'.repeat(40)}),true);
  step('Record stopped rebase child',recover('rebase_stopped'));step('Stopped receipt cannot be recorded twice',recover('rebase_stopped'),true);step('Record rollback dispatch',recover('undo_started'));step('Record completed rollback',recover('restored'));step('Safely rolled-back rebase becomes interrupted',finish({state:'interrupted'}));
  const completedRefresh=refresh();step('New rebase after reconciled interruption',begin(completedRefresh));step('Record stopped successful rebase',{...recover('rebase_stopped'),requestId:completedRefresh.requestId});
  const clean={...target.checkpoint,id:randomUUID(),reason:'refreshed',revision:{...target.revision,base:completedRefresh.toBase,head:'f'.repeat(40)}};const completed={method:'finishTargetRefresh',runId:target.run.id,requestId:completedRefresh.requestId,outcome:{state:'completed',toHead:clean.revision.head},checkpoint:clean};
  step('Dirty refreshed checkpoint refused',{...completed,checkpoint:{...clean,revision:{...clean.revision,dirty:true}}},true);
  patch('runs',target.run.id,{status:'attention_required'});step('Ordinary refresh completion requires paused run',completed,true);patch('runs',target.run.id,{status:'paused'});step('Clean stopped rebase updates base target and checkpoint',completed);step('Completed refresh replay does not add another checkpoint',completed);
  // Reset trusted candidate/work records for independent conflict scenarios; no Git is implied.
  patch('repository_work',target.run.id,{base:target.revision.base,targetRef:'refs/heads/main',refreshes:[],checkpoints:[target.checkpoint]});
  const conflicted=refresh();step('Admit conflict-producing rebase',begin(conflicted));
  const workspace={path:join(root,'conflict'),branch:'crewroom/conflict',state:'preparing',requirementsHash:target.revision.requirementsHash,instructionsHash:target.revision.instructionsHash};
  const workspaceOp=(value:any,extra:any={})=>({method:'recordTargetRefreshWorkspace',runId:target.run.id,requestId:conflicted.requestId,workspace:value,...extra});
  step('Record isolated conflict preparation',workspaceOp(workspace));step('Workspace path is immutable',workspaceOp({...workspace,path:'other'}),true);step('Workspace scope is immutable',workspaceOp({...workspace,requirementsHash:'other'}),true);
  step('Preparing workspace blocks another refresh',pre(),true);
  const readyConflict={...workspace,state:'conflicted',gitFile:'fixture-conflict-git'};step('Workspace records its conflict paths',workspaceOp(readyConflict,{conflicts:['answer']}));
  step('Preparing rebase becomes retained conflict',{method:'finishTargetRefresh',runId:target.run.id,requestId:conflicted.requestId,outcome:{state:'conflicted',conflicts:['answer']}});
  step('Workspace cannot move back to preparing',workspaceOp(workspace),true);step('Workspace update cannot invent continuation identity',workspaceOp({...readyConflict,continuation:{attemptId:'foreign'}}),true);
  step('Active unresolved conflict needs a reasoned approved retry',pre(),true);step('Reason cannot supersede an editable conflict',pre({supersedes:{requestId:conflicted.requestId,reason:'Retry'}}),true);
  active(target);step('Unscoped target conflict writer refused',writer(target),true);step('Wrong target conflict identity refused',writer(target,target.writer,claim(target.writer,{targetRefreshId:'other'})),true);
  const conflictGrant=claim(target.writer,{targetRefreshId:conflicted.requestId,candidateId:target.candidate.id});step('Explicit conflict scope permits one writer',writer(target,target.writer,conflictGrant));
  const identity={attemptId:target.writer.attempt.id,toolCallId:randomUUID(),requestId:conflicted.requestId,path:workspace.path,branch:workspace.branch,sourceHead:conflicted.fromHead,paths:['answer']};
  const continueOp=(value:any)=>({method:'beginTargetConflictContinuation',runId:target.run.id,requestId:conflicted.requestId,identity:value});
  for(const changes of [{paths:[]},{paths:['outside']},{branch:'other'},{requestId:'other'},{sourceHead:'other'},{attemptId:target.peer.attempt.id}])step('Conflict continuation identity refused',continueOp({...identity,...changes}),true);
  step('Record exact isolated Git continuation before mutation',continueOp(identity));step('Duplicate continuation cannot start twice',continueOp(identity),true);step('Continuing workspace blocks a writer even when held',writer(target,target.writer,conflictGrant),true);
  step('Workspace updates cannot settle a continuation',workspaceOp({...readyConflict,state:'ready',toHead:'f'.repeat(40)}),true);
  const settle=(result:any,selected=identity)=>({method:'finishTargetConflictContinuation',runId:target.run.id,requestId:conflicted.requestId,identity:selected,result});
  step('Ready continuation requires a validated head',settle({state:'ready',conflicts:[]}),true);step('Serialized continuation identity remains exact',settle({state:'ready',head:'f'.repeat(40),conflicts:[]},Object.fromEntries(Object.entries(identity).reverse())),true);
  const result={state:'ready',head:'f'.repeat(40),conflicts:[]};step('Returned Git continuation settles to ready with immutable receipt',settle(result));step('Continuation settlement does not replay after state advanced',settle(result),true);
  const ready=legacy.repositoryWork(target.run.id)!.refreshes!.at(-1)!.resolutionWorkspace!;
  step('Ready workspace cannot change promotion head',workspaceOp({...ready,toHead:'other'}),true);step('Ready workspace cannot erase continuation receipts',workspaceOp({...ready,continuationReceipts:[]}),true);
  patch('attempts',target.writer.attempt.id,{state:'exited',ownership:'stopped'});step('Release stopped scoped conflict writer',release(target));active(target,'paused','running');
  const operationId=randomUUID(),toHead=result.head;const process={version:1,token:'fixture-token',root:join(root,`${operationId}-fixture-token`),command:'/fixture/git',args:['reset','--hard',toHead],cwd:target.work.path,env:{PATH:'/fixture'}};
  const promotion=(extra:any={})=>({method:'beginTargetPromotion',runId:target.run.id,requestId:conflicted.requestId,operationId,toHead,process,...extra});
  step('Promotion requires a recorded requested completion',promotion(),true);
  const completion={id:operationId,attemptId:target.writer.attempt.id,toolCallId:'complete',summary:'Resolved',head:target.revision.head,worktreeHash:target.revision.worktreeHash,state:'requested',conflictRefreshId:conflicted.requestId};patch('coding',target.run.id,{completions:[completion]});
  for(const changes of [{version:2},{root:'other'},{cwd:'other'},{args:['reset','wrong']}])step('Checked promotion identity refused',promotion({process:{...process,...changes}}),true);
  patch('runs',target.run.id,{desiredState:'paused'});step('Promotion requires explicit running desire',promotion(),true);patch('runs',target.run.id,{desiredState:'running'});
  step('Record sealed promotion identity',promotion());step('Exact promotion replay',promotion());step('Promotion replay retains serialized process identity',promotion({process:Object.fromEntries(Object.entries(process).reverse())}),true);step('Promotion replay cannot change environment serialization',promotion({process:{...process,env:{OTHER:'changed'}}}),true);
  const proof={originalHead:target.revision.head,originalBase:target.revision.base,originalPath:target.work.path,originalBranch:target.work.branch,originalTargetRef:'refs/heads/main',targetRef:conflicted.targetRef,originalGitFile:legacy.repositoryWork(target.run.id)!.gitFile,workspaceGitFile:ready.gitFile,workspacePath:workspace.path,workspaceBranch:workspace.branch,toHead,worktreeHash:target.revision.worktreeHash,requirementsHash:target.revision.requirementsHash,instructionsHash:target.revision.instructionsHash,expectedTarget:conflicted.toBase,observedTarget:'0'.repeat(40)};
  const reject=(p=proof)=>({method:'rejectUnpromotedConflict',runId:target.run.id,requestId:conflicted.requestId,operationId,proof:p});
  for(const key of Object.keys(proof))step(`Unpromoted rejection requires matching ${key}`,reject({...proof,[key]:key==='observedTarget'?proof.expectedTarget:'changed'}),true);
  patch('runs',target.run.id,{codingTransition:{completionId:operationId,phase:'stopping'}});step('Target movement atomically rejects completion retains workspace clears transition and notifies',reject());step('Terminal rejection cannot execute twice',reject(),true);
  const operatorWorkspace=legacy.repositoryWork(target.run.id)!.refreshes!.at(-1)!.resolutionWorkspace!;step('Operator-held workspace cannot return to ready',workspaceOp({...operatorWorkspace,state:'ready'}),true);
  step('Blank retry reason refused',pre({supersedes:{requestId:conflicted.requestId,reason:' '}}),true);const supersedes={requestId:conflicted.requestId,reason:'Retry current target'};step('Explicit reason may supersede held conflict',pre({supersedes}));
  const retry=refresh({supersedes});step('Resolved completion permits reasoned retry intent',begin(retry));
  const retryRecovery=(stage:string)=>({...recover(stage),requestId:retry.requestId});step('Record stopped retry rebase',retryRecovery('rebase_stopped'));step('Record retry rollback',retryRecovery('undo_started'));step('Record restored retry source',retryRecovery('restored'));
  const retryFinish={method:'finishTargetRefresh',runId:target.run.id,requestId:retry.requestId,outcome:{state:'interrupted'}};step('Retry interruption cannot carry completion proof',{...retryFinish,checkpoint:target.checkpoint},true);step('Interrupted reasoned retry becomes a new explicit operator hold',retryFinish);step('Held retry replay remains unchanged',retryFinish);
  // Reuse the complete proof fixture to compare the other terminal outcome and successful promotion.
  const promotionRefresh={...conflicted,state:'conflicted',conflicts:[],resolutionWorkspace:{...ready,state:'promoting',operationId,toHead,promotionProcess:process}};
  patch('repository_work',target.run.id,{refreshes:[promotionRefresh]});patch('coding',target.run.id,{completions:[completion]});patch('runs',target.run.id,{codingTransition:{completionId:operationId,phase:'stopping'}});
  step('Stopped unapplied promotion becomes explicit interrupted outcome',{...reject(),method:'interruptUnpromotedConflict'});
  patch('repository_work',target.run.id,{refreshes:[promotionRefresh]});patch('coding',target.run.id,{completions:[completion]});
  const promoted={method:'finishTargetRefresh',runId:target.run.id,requestId:conflicted.requestId,outcome:{state:'completed',toHead,resolutionWorkspace:{...promotionRefresh.resolutionWorkspace,state:'promoted'}},checkpoint:{...target.checkpoint,id:randomUUID(),reason:'refreshed',revision:{...target.revision,base:conflicted.toBase,head:toHead}}};
  patch('runs',target.run.id,{desiredState:'cancelled'});step('Cancellation supersedes final promotion',promoted,true);patch('runs',target.run.id,{desiredState:'running',status:'attention_required'});step('Stopped conflict promotion can complete from attention hold',promoted);step('Promotion outcome replay preserves one checkpoint',promoted);
  step('Final repository ownership read',{method:'repositoryWork',runId:target.run.id});
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final TypeScript reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),clock:'Exact controlled timestamps including legacy wall-clock calls',reopen:'Rust process/store reopened for every operation; TypeScript reopened at seed and finish',claims:'Durable ownership/recovery transactions only. Managed worktrees, process identities, candidate/checkpoint hashes, checked process manifests and remote observations are supplied fixtures; no native execution, Git mutation or provider delivery is claimed.'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
