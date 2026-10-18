/** N2.7a: request schema and exact clock parity against the retained Workbench.
 * Each case starts from the same disposable schema-26 records. No agent/provider runs. */
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { closeSync, copyFileSync, mkdtempSync, openSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { ZodError } from 'zod';
import { Workbench, Conflict, NotFound } from '../../server/workbench';
import { RoleCatalog, defaultRoles, RoleConflict } from '../../server/roles';
import { Learning } from '../../server/learning';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n27a-')));
const seedPath=join(root,'seed.sqlite'), legacyPath=join(root,'legacy.sqlite'), nativePath=join(root,'native.sqlite');
const rolePath=join(root,'roles.json'), nativeRoles=join(root,'native-roles.json');
const now=1_790_982_123_456, RealDate=Date;
globalThis.Date=class extends RealDate {constructor(value?:string|number|Date){super(value===undefined?now:value);}static now(){return now;}} as DateConstructor;
let legacy=new Workbench(seedPath,()=>now);
const cases:string[]=[], ids=new Map<string,string>(), fixed=new Set<string>();
const uuid=/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
function transform(value:any, convert:(s:string)=>string):any {
  if(Array.isArray(value))return value.map(v=>transform(v,convert));
  if(value && typeof value==='object')return Object.fromEntries(Object.entries(value).map(([k,v])=>[convert(k),transform(v,convert)]));
  return typeof value==='string'?convert(value):value;
}
const fromNative=(value:any)=>transform(value,s=>{
  for(const [expected,actual] of ids){if(s===actual)return expected;if(s===`LOCAL-${actual.slice(0,8)}`)return `LOCAL-${expected.slice(0,8)}`;}
  return s;
});
function remember(expected:any,actual:any):void {
  if(typeof expected==='string' && uuid.test(expected) && expected!==actual && !fixed.has(expected)) {
    assert.ok(typeof actual==='string' && uuid.test(actual),'Generated UUID spelling');
    if(ids.has(expected))assert.equal(ids.get(expected),actual);else {assert.ok(![...ids.values()].includes(actual));ids.set(expected,actual);}
  } else if(Array.isArray(expected)) {assert.ok(Array.isArray(actual));assert.equal(actual.length,expected.length);expected.forEach((v,i)=>remember(v,actual[i]));}
  else if(expected && typeof expected==='object') {
    assert.ok(actual && typeof actual==='object');assert.deepEqual(Object.keys(actual).sort(),Object.keys(expected).sort());
    for(const [k,v] of Object.entries(expected))remember(v,actual[k]);
  } else assert.deepEqual(fromNative(actual),expected);
}
function snapshot(path:string) {
  const db=new DatabaseSync(path,{readOnly:true});
  try {return Object.fromEntries(db.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").all().map(({name})=>
    [String(name),db.prepare(`SELECT * FROM ${name} ORDER BY rowid`).all().map(row=>Object.fromEntries(Object.entries(row).map(([k,v])=>
      [k,['data','imported','refinement','payload','result'].includes(k)?JSON.parse(String(v)):v])))]));}
  finally {db.close();}
}
function invoke(op:any):any {
  const s=legacy, v=op.input;
  switch(op.method) {
    case 'createTask':return s.createTask(v);
    case 'assignRepository':return s.assignRepository(op.taskId,v);
    case 'addNote':return s.addNote(op.taskId,v);
    case 'refine':return s.refine(op.taskId,v);
    case 'setDependencies':return s.setDependencies(op.taskId,v);
    case 'queueRefinement':return s.queueRefinement(op.taskId,v);
    case 'queueResearch':return s.queueResearch(op.taskId,v);
    case 'queue':return s.queue(op.taskId,v,op.roles);
    case 'reorderQueue':return s.reorderQueue(v);
    case 'cancelQueued':return s.cancelQueued(op.runId,v);
    case 'setActiveLimit':return s.setActiveLimit(op.runId,v);
    case 'adoptRequirements':return s.adoptRequirements(op.runId,v);
    case 'closeRefinement':return s.closeRefinement(op.runId);
    case 'steer':return s.steer(op.runId,v);
    case 'prepareMessage':return s.prepareMessage(op.attemptId,op.toolCallId,v);
    case 'applySteering':return s.applySteering(op.attemptId,op.toolCallId,v);
    case 'admitAssignment':return s.admitAssignment(op.runId,v);
    case 'grantRepairCycle':return s.grantRepairCycle(op.runId,op.assignmentId,v);
    case 'setQueueLaunch':return s.setQueueLaunch(op.runId,v,op.roles);
    case 'proposeOperatorSpecialist':return s.proposeOperatorSpecialist(op.runId,v,op.role,op.model);
    case 'proposeRefinement':return s.proposeRefinement(op.attemptId,op.toolCallId,v);
    case 'decideRefinement':return s.decideRefinement(op.id,v);
    case 'learningPropose':return new Learning(s,new RoleCatalog(rolePath)).propose(op.attemptId,op.toolCallId,v);
    case 'learningDecide':return new Learning(s,new RoleCatalog(rolePath)).decide(op.id,v);
    case 'rolesCreate':return new RoleCatalog(rolePath).create(v);
    case 'rolesUpdate':return new RoleCatalog(rolePath).update(op.id,v);
    case 'repositoryUpdateReplay':return s.repositoryUpdateReplay(op.id,v);
    case 'importJira':return s.importJira(v);
    default:throw new Error(`Unknown method ${op.method}`);
  }
}
function check(label:string,op:any,raw?:string) {
  legacy.close();for(const path of [legacyPath,nativePath])copyFileSync(seedPath,path);
  for(const path of [rolePath,nativeRoles])writeFileSync(path,JSON.stringify(defaultRoles));
  legacy=new Workbench(legacyPath,()=>now);ids.clear();
  transform(op,s=>{if(uuid.test(s))fixed.add(s);return s;});
  let expected:any,error:unknown;try{expected=invoke(structuredClone(op))??null;}catch(e){error=e;}
  const input=raw??JSON.stringify({...op,rolePath:nativeRoles,now});
  // A file-backed stdin gives large Unicode fixtures a deterministic EOF; the
  // synchronous Node pipe writer can otherwise leave the child waiting for EOF.
  const requestFile=join(root,'request.json');writeFileSync(requestFile,input);const fd=openSync(requestFile,'r');
  let actual:any;try {actual=JSON.parse(execFileSync(resolve('target/debug/examples/ledger-oracle'),[nativePath],{stdio:[fd,'pipe','pipe'],encoding:'utf8',maxBuffer:10*1024*1024,timeout:30000}));}finally {closeSync(fd);}
  try {
    assert.equal(actual.ok,!error,`${label}: ${String(error)} / Rust ${JSON.stringify(actual).slice(0,600)}`);
    if(error instanceof ZodError)assert.equal(actual.kind,'invalid',label);
    else if(error instanceof Conflict || error instanceof NotFound || error instanceof RoleConflict) {
      assert.equal(actual.kind,error instanceof NotFound?'not_found':'conflict',label);
      assert.equal(actual.error,error.message,label);
    } else if(error)throw error;
    else {expected=JSON.parse(JSON.stringify(expected));remember(expected,actual.value);assert.deepEqual(fromNative(actual.value),expected,label);}
    const wanted=snapshot(legacyPath), saved=snapshot(nativePath);
    for(const state of [wanted,saved])for(const row of state.runs)delete row.data.holdReason;
    remember(wanted,saved);assert.deepEqual(fromNative(saved),wanted,`${label}: durable records`);
    cases.push(label);
  } catch(error) {console.error('Failed boundary:',label);throw error;}
}
function mutate(op:any,path:string,value:any):any {
  const copy=structuredClone(op);const keys=path.split('.');let cursor=copy;
  for(const key of keys.slice(0,-1))cursor=cursor[key];
  if(value===undefined)delete cursor[keys.at(-1)!];else cursor[keys.at(-1)!]=value;
  return copy;
}
function variants(label:string,op:any,path:string,values:[string,any][]) {for(const [kind,value] of values)check(`${label}: ${kind}`,mutate(op,path,value));}
const optional:[string,any][]=[['missing',undefined],['null',null],['number',7]];
function prose(label:string,op:any,path:string,max:number) {
  variants(label,op,path,[...optional,['empty',''],['spaces',' \t\r\n'],['BOM','\ufeff'],['NEL','\u0085'],['trim set','\u00a0\ufeff valid \u2029'],
    ['ASCII limit','a'.repeat(max)],['padding outside limit',' '.repeat(5)+'a'.repeat(max)+' '.repeat(5)],['ASCII overflow','a'.repeat(max+1)],
    ['UTF-16 limit','🦀'.repeat(Math.floor(max/2))+'a'.repeat(max%2)],['UTF-16 units may exceed code-point limit','🦀'.repeat(Math.floor(max/2))+'a'.repeat(max%2+1)],['code-point limit','🦀'.repeat(max)],['code-point overflow','🦀'.repeat(max+1)]]);
}
const numeric:[string,any][]=[...optional,['zero',0],['negative',-1],['fraction',1.25],['max safe',Number.MAX_SAFE_INTEGER],['unsafe',Number.MAX_SAFE_INTEGER+1],['string','1'],['boolean',true]];
const idsToCheck:[string,any][]=[...optional,['nil','00000000-0000-0000-0000-000000000000'],['max','ffffffff-ffff-ffff-ffff-ffffffffffff'],['uppercase max','FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF'],['v8','12345678-1234-8123-8123-123456789abc'],['upper v4','ABCDEFAB-ABCD-4ABC-ABCD-ABCDEFABCDEF'],['v0','12345678-1234-0123-8123-123456789abc'],['v9','12345678-1234-9123-8123-123456789abc'],['wrong variant','12345678-1234-4123-7123-123456789abc'],['compact','12345678123441238123123456789abc'],['braces','{12345678-1234-4123-8123-123456789abc}'],['URN','urn:uuid:12345678-1234-4123-8123-123456789abc']];
try {
  const checkout=join(root,'repo');seedRepository(checkout);const repo=saveRepositoryFixture(legacy,{name:'Boundary fixture',path:checkout,targetBranch:'main',checks:[]});
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  function task(title:string,repositoryId?:string) {const t=legacy.createTask({title,description:'',repositoryId});return legacy.refine(t.id,{revision:1,sourceRevision:1,brief:'Accepted',criteria:['Prove it']});}
  const plain=task('Plain'),coding=task('Coding',repo.id),dependency=task('Dependency'),queuedTask=task('Queued',repo.id), queued=legacy.queue(queuedTask.id,{revision:2});
  const assignmentTask=task('Assignment',repo.id), assignmentRun=legacy.queue(assignmentTask.id,{revision:2});
  const activeTask=task('Active',repo.id),active=legacy.queue(activeTask.id,{revision:2});
  const coordinator=legacy.admitAssignment(active.id,{name:'Coordinator',coordinator:true,role,model,input:'Fixture'});
  const peer=legacy.admitAssignment(active.id,{name:'Peer',coordinator:false,role,model,input:'Fixture'});
  legacy.claimAttempt(coordinator.attempt.id);legacy.recordAttemptIdentity(coordinator.attempt.id,{hostPid:1,ompPid:2,sessionId:'fixture',hostUrl:'http://fixture',pane:'%1'});
  legacy.setDesiredState(active.id,'running');legacy.beginPause(active.id);legacy.recordOwnership(coordinator.attempt.id,true);legacy.finishPause(active.id);
  const revised=legacy.refine(activeTask.id,{revision:2,sourceRevision:1,brief:'Revised',criteria:['Current']});
  const refineTask=task('Refinement'),refineRun=legacy.queueRefinement(refineTask.id,{revision:2,purpose:'explore'});
  const refiner=legacy.admitAssignment(refineRun.id,{name:'Coordinator',coordinator:true,role,model,input:'Fixture'});
  legacy.claimAttempt(refiner.attempt.id);legacy.recordAttemptIdentity(refiner.attempt.id,{hostPid:3,ompPid:4,sessionId:'fixture',hostUrl:'http://fixture',pane:'%2'});
  legacy.setDesiredState(refineRun.id,'running');legacy.startExecution(refineRun.id);
  const proposal=legacy.proposeRefinement(refiner.attempt.id,'seed',{brief:'Proposed',criteria:['New'],decisions:[],unresolvedQuestions:[]});
  const closeTask=task('Close'),closeRun=legacy.queueRefinement(closeTask.id,{revision:2,purpose:'grill'});
  const steering=legacy.steer(active.id,{requestId:'seed',body:'Recorded evidence'});
  const learningEvidence=legacy.steer(refineRun.id,{requestId:'learning-seed',body:'Recorded evidence'});
  const learn=legacy.proposeLearning({runId:active.id,attemptId:coordinator.attempt.id,toolCallId:'seed',target:{kind:'project',repositoryId:repo.id},change:'New project guidance',baseVersion:0,rationale:'Evidence',evidence:[{kind:'steering',id:steering.id}]});
  legacy.close();transform(snapshot(seedPath),s=>{if(uuid.test(s))fixed.add(s);return s;});legacy=new Workbench(legacyPath,()=>now);
  const key=randomUUID(),launch={team:[{name:'Coordinator',roleId:role.id,coordinator:true,input:'Coordinate',criteria:[],dependsOn:[]}],repository:{mode:'new'}};
  const create={method:'createTask',input:{title:'Valid',description:''}};
  prose('task title',create,'input.title',300);prose('task description',create,'input.description',40000);
  variants('optional repository',create,'input.repositoryId',[...optional,['empty',''],['valid',repo.id]]);
  check('task unknown field stripped',{...create,input:{...create.input,extra:true}});
  const note={method:'addNote',taskId:plain.id,input:{mode:'decision',body:'Valid'}};prose('note',note,'input.body',40000);
  variants('note mode',note,'input.mode',[...optional,['unknown','draft'],['grill','grill']]);check('note retains missing-task refusal order',{...note,taskId:'missing',input:{mode:'decision',body:null}});
  const refine={method:'refine',taskId:plain.id,input:{revision:2,sourceRevision:1,brief:'Valid',criteria:['Valid']}};
  prose('refinement brief',refine,'input.brief',40000);prose('criterion',refine,'input.criteria.0',2000);prose('decision note',refine,'input.decisionNote',4000);
  variants('criteria list',refine,'input.criteria',[...optional,['empty',[]],['limit',Array(100).fill('C')],['overflow',Array(101).fill('C')]]);
  variants('revision',refine,'input.revision',numeric);variants('source revision',refine,'input.sourceRevision',numeric);
  const assign={method:'assignRepository',taskId:plain.id,input:{repositoryId:repo.id,revision:2}};
  variants('assigned repository',assign,'input.repositoryId',[...optional,['empty',''],['long missing ID','a'.repeat(201)]]);
  const queue={method:'queue',taskId:coding.id,roles:[role],input:{revision:2,requestId:key}};
  variants('queue request UUID',queue,'input.requestId',idsToCheck);variants('queue active limit',queue,'input.activeLimitMs',[...numeric,['minimum',1000],['below minimum',999]]);
  const deps={method:'setDependencies',taskId:plain.id,input:{requestId:key,dependencyRevision:0,dependsOn:[dependency.id]}};
  variants('dependency UUID',deps,'input.dependsOn.0',idsToCheck.filter(([name])=>name!=='missing'));
  variants('dependency list',deps,'input.dependsOn',[...optional,['empty',[]],['limit',Array(12).fill(dependency.id)],['overflow',Array(13).fill(dependency.id)]]);
  variants('dependency revision',deps,'input.dependencyRevision',numeric);
  const order={method:'reorderQueue',input:{requestId:key,expectedRevision:legacy.codingQueue().revision,runIds:[queued.id,assignmentRun.id]}};
  // Use the source queue revision and complete order from the immutable seed.
  {const seed=new Workbench(seedPath,()=>now);order.input.expectedRevision=seed.codingQueue().revision;order.input.runIds=seed.codingQueue().plans.map(p=>p.id);seed.close();}
  variants('queue order UUID',order,'input.runIds.0',idsToCheck.filter(([name])=>name!=='missing'));check('queue order unchanged',order);check('queue order has no arbitrary array cap',mutate(order,'input.runIds',Array(10001).fill(queued.id)));
  const explore={method:'queueRefinement',taskId:plain.id,input:{revision:2,purpose:'explore',requestId:key}};
  variants('alternate UUID',explore,'input.requestId',idsToCheck);check('alternate exact clock',explore);check('close exact clock',{method:'closeRefinement',runId:closeRun.id});
  const research={method:'queueResearch',taskId:plain.id,input:{revision:2,requestId:key}};variants('research active limit',research,'input.activeLimitMs',[...numeric,['minimum',1000]]);
  const adoption={method:'adoptRequirements',runId:active.id,input:{requestId:key,revision:revised.revision,reason:'Valid'}};prose('adoption reason',adoption,'input.reason',4000);
  const repair={method:'grantRepairCycle',runId:active.id,assignmentId:peer.assignment.id,input:{requestId:key,reason:'Valid'}};prose('repair reason',repair,'input.reason',2000);
  const admission={method:'admitAssignment',runId:assignmentRun.id,input:{name:'Writer',coordinator:false,role,model,input:'Valid'}};
  prose('assignment name',admission,'input.name',100);prose('assignment raw input',admission,'input.input',100000);
  const steer={method:'steer',runId:active.id,input:{requestId:key,body:'Valid'}};prose('steering body',steer,'input.body',40000);prose('steering raw identity',steer,'input.requestId',200);
  const peerMessage={method:'prepareMessage',attemptId:refiner.attempt.id,toolCallId:'boundary',input:{to:'Coordinator',body:'Valid'}};
  prose('peer body',peerMessage,'input.body',40000);prose('tool call identity',peerMessage,'toolCallId',200);
  const launchOp={method:'setQueueLaunch',runId:queued.id,roles:[role],input:{requestId:key,launch}};
  prose('launch raw input',launchOp,'input.launch.team.0.input',40000);
  variants('launch optional criteria',launchOp,'input.launch.team.0.criteria',[...optional,['empty',[]]]);
  variants('launch optional dependencies',launchOp,'input.launch.team.0.dependsOn',[...optional,['empty',[]],['overflow',Array(11).fill('Coordinator')]]);
  const obligation=mutate(launchOp,'input.launch.team.0.criteria',[{index:0,obligation:'Verify',checkIds:[]}]);prose('launch obligation',obligation,'input.launch.team.0.criteria.0.obligation',2000);
  variants('criterion index',obligation,'input.launch.team.0.criteria.0.index',numeric);
  const insertion={method:'proposeOperatorSpecialist',runId:queued.id,role,model,input:{requestId:key,roleId:role.id,input:'Inspect'}};prose('insertion input',insertion,'input.input',40000);
  const refinement={method:'proposeRefinement',attemptId:refiner.attempt.id,toolCallId:'boundary',input:{brief:'Proposed',criteria:['Criterion'],decisions:['Decision'],unresolvedQuestions:['Question']}};
  for(const [field,max] of [['brief',40000],['criteria.0',2000],['decisions.0',4000],['unresolvedQuestions.0',4000]] as const)prose('proposal '+field,refinement,'input.'+field,max);
  const decision={method:'decideRefinement',id:proposal.id,input:{requestId:key,decision:'discard',reason:'Valid'}};prose('refinement decision',decision,'input.reason',4000);
  const learning={method:'learningPropose',attemptId:refiner.attempt.id,toolCallId:'boundary',input:{target:{kind:'role',roleId:role.id},change:'Changed',rationale:'Evidence',evidence:[{kind:'steering',id:learningEvidence.id}]}};
  prose('learning change',learning,'input.change',12000);prose('learning rationale',learning,'input.rationale',4000);
  const learningDecision={method:'learningDecide',id:learn.id,input:{requestId:key,decision:'reject',reason:'Valid'}};prose('learning decision',learningDecision,'input.reason',2000);
  const roleOp={method:'rolesCreate',input:{...role,id:'boundary'}};
  for(const [field,max] of [['name',100],['category',60],['purpose',1000],['instructions',12000],['model',200]] as const)prose('role '+field,roleOp,'input.'+field,max);
  variants('role optional revision',roleOp,'input.revision',numeric);
  const repository={method:'repositoryUpdateReplay',id:repo.id,input:{name:'Valid',path:checkout,targetBranch:'main',revision:1,requestId:key}};
  prose('repository name',repository,'input.name',120);prose('repository target',repository,'input.targetBranch',200);
  const jira={method:'importJira',input:{siteUrl:'https://fixture.atlassian.net',issues:[{id:'boundary',key:'SIM-1',fields:{summary:'Valid',status:{name:'Open'}}}]}};
  prose('Jira summary',jira,'input.issues.0.fields.summary',300);prose('Jira status',jira,'input.issues.0.fields.status.name',80);
  for(const [name,base,field,value] of [['revision',refine,'revision',2],['limit',queue,'activeLimitMs',1000],['zero',deps,'dependencyRevision',0]] as const) {
    for(const spelling of [String(value)+'.0',String(value)+'e0',value===0?'-0':'0.'+value+'e'+String(String(value).length)]) {
      const op=mutate(base,'input.'+field,value);let raw=JSON.stringify({...op,rolePath:nativeRoles,now});raw=raw.replace('"'+field+'":'+value,'"'+field+'":'+spelling);
      check(`JSON integer ${name}: ${spelling}`,op,raw);
    }
  }
  for(const [label,op] of Object.entries({create,note,refine,assign,queue,deps,order,explore,research,adoption,repair,steer,peerMessage,launchOp,insertion,refinement,decision,learning,learningDecision,roleOp,repository,jira})) {
    variants(label+' object',op,'input',[['missing',undefined],['null',null],['array',[]],['string','invalid'],['unknown field',{...op.input,unrecognized:'ignored only by non-strict schemas'}]]);
  }
  for(const spelling of ['1000.00000000000000001','9007199254740991.0','9007199254740990.9','9007199254740991.9']) {
    const raw=JSON.stringify({...queue,input:{...queue.input,activeLimitMs:'RAW'},now}).replace('"RAW"',spelling);
    check('JSON rounded integer: '+spelling,JSON.parse(raw),raw);
  }
  for(const [label,op,field] of [['launch index',obligation,'index'],['role revision',{...roleOp,input:{...roleOp.input,revision:1}},'revision']] as const) {
    const raw=JSON.stringify({...op,rolePath:nativeRoles,now}).replace('"'+field+'":'+(field==='index'?0:1),'"'+field+'":'+(field==='index'?'0.0':'1.0'));
    check('JSON integer nested '+label,JSON.parse(raw),raw);
  }
  console.log(JSON.stringify({passed:true,scope:'N2.7a input and exact clock boundaries; no execution',operations:cases.length,cases,tables:Object.keys(snapshot(nativePath)).length,clock:now},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
