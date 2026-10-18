/** N2.7b serialized replay and repository preflight oracle. Each native operation reopens the database.
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
import { repositorySchema, type ValidatedRepository } from '../../server/repository-configuration';
import { seedRepository, saveRepositoryFixture } from '../../tests/fixtures/repository';

const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-n27b-')));
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
    case 'createRepository': return saveRepositoryFixture(s,op.input);
    case 'saveRepository': return s.saveRepository(repositorySchema.parse(op.input) as ValidatedRepository);
    case 'repositoryReplay': return s.repositoryReplay(op.input);
    case 'repositoryUpdateReplay': return s.repositoryUpdateReplay(op.id,op.input);
    case 'retireRepository': return s.retireRepository(op.id,op.input);
    case 'queue': return s.queue(op.taskId,op.input,op.roles);
    case 'queueResearch': return s.queueResearch(op.taskId,op.input);
    case 'queueRefinement': return s.queueRefinement(op.taskId,op.input);
    case 'setQueueLaunch': return s.setQueueLaunch(op.runId,op.input,op.roles);
    case 'proposeOperatorSpecialist': return s.proposeOperatorSpecialist(op.runId,op.input,op.role,op.model);
    case 'setDependencies': return s.setDependencies(op.taskId,op.input);
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
  const checkout=join(root,'repo');seedRepository(checkout);const repo=saveRepositoryFixture(legacy,{name:'Replay oracle',path:checkout,targetBranch:'main'});
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  function task(name:string,repositoryId?:string) {const t=legacy.createTask({title:name,description:'',repositoryId});return legacy.refine(t.id,{revision:1,sourceRevision:1,brief:'Accepted',criteria:['Verify']});}
  const coding=task('Coding',repo.id),legacyTask=task('Legacy',repo.id),empty=task('Empty',''),emptyResearch=task('Empty research',''),plain=task('Plain'),plainResearch=task('Plain research');
  const queued=legacy.queue(task('Launch',repo.id).id,{revision:2});
  const insertRun=legacy.queue(task('Insertion',repo.id).id,{revision:2});
  const initial=legacy.queue(legacyTask.id,{revision:2,requestId:randomUUID()});
  legacy.close();copyFileSync(legacyPath,nativePath);legacy=new Workbench(legacyPath,()=>now);protect(snapshot(legacyPath));compare('seed');
  const reverse=(v:any):any=>Array.isArray(v)?v.map(reverse):v && typeof v==='object'?Object.fromEntries(Object.entries(v).reverse().map(([k,x])=>[k,reverse(x)])):v;
  const head='a'.repeat(40),launch={team:[{name:'Coordinator',roleId:role.id,coordinator:true,input:'Coordinate',criteria:[{index:0,obligation:'Verify',checkIds:[]}],dependsOn:[]}],repository:{mode:'continue',branch:'crewroom/replay',expectedHead:head}};
  const queue={method:'queue',taskId:coding.id,roles:[role],input:{revision:2,requestId:randomUUID(),launch}};
  const run=step('Queue normalizes launch object field order',{...queue,input:reverse(queue.input)});
  step('Queue reordered replay after reopen',queue);
  step('Queue lexical integer replay',{...queue,input:reverse(queue.input)},false,{revision:'2e0',index:'-0'});
  step('Queue changed launch refuses receipt',{...queue,input:{...queue.input,launch:{...launch,repository:{...launch.repository,expectedHead:'b'.repeat(40)}}}},true);
  fixture('Receipt payload order remains significant',(db)=>{const row=db.prepare('SELECT payload FROM queue_requests WHERE request_id=?').get(queue.input.requestId)!;const payload=JSON.parse(String(row.payload));db.prepare('UPDATE queue_requests SET payload=? WHERE request_id=?').run(JSON.stringify(reverse(payload)),queue.input.requestId);});
  step('Reordered stored payload is not structural replay',queue,true);
  fixture('Restore normalized queue receipt',(db,map)=>{db.prepare('UPDATE queue_requests SET payload=? WHERE request_id=?').run(JSON.stringify(map({taskId:coding.id,revision:2,activeLimitMs:3600000,launch})),queue.input.requestId);});
  step('Normalized queue receipt remains usable',queue);
  fixture('Legacy queued run predates receipts',(db)=>{db.prepare('DELETE FROM queue_requests WHERE request_id=?').run(initial.requestId!);});
  step('Legacy run without queue receipt replays',{method:'queue',taskId:legacyTask.id,input:{revision:2,requestId:initial.requestId}});
  patch('runs',initial.id,{launch:null});
  step('Legacy null launch differs from missing',{method:'queue',taskId:legacyTask.id,input:{revision:2,requestId:initial.requestId}},true);
  patch('runs',initial.id,{launch:reverse(launch)});
  step('Legacy nested launch field order differs',{method:'queue',taskId:legacyTask.id,roles:[role],input:{revision:2,requestId:initial.requestId,launch}},true);
  patch('runs',initial.id,{launch});
  step('Legacy normalized launch replays',{method:'queue',taskId:legacyTask.id,roles:[role],input:{revision:2,requestId:initial.requestId,launch}});
  const edit={method:'setQueueLaunch',runId:queued.id,roles:[role],input:{requestId:randomUUID(),launch}};
  step('Launch edit normalizes request', {...edit,input:reverse(edit.input)});
  step('Launch edit replays reordered input',edit,false,{index:'0.0'});
  step('Cross-kind identity collision refuses queue',{...queue,input:{...queue.input,requestId:edit.input.requestId}},true);
  step('Empty selected repository coding refusal',{method:'queue',taskId:empty.id,input:{revision:2}},true);
  step('Empty selected repository research refusal',{method:'queueResearch',taskId:emptyResearch.id,input:{revision:2,requestId:randomUUID()}},true);
  const refinement={method:'queueRefinement',taskId:empty.id,input:{revision:2,purpose:'explore',requestId:randomUUID()}};
  step('Empty selected repository still permits refinement',refinement);step('Refinement replay after reopen',refinement,false,{revision:'2.0'});
  const research={method:'queueResearch',taskId:plainResearch.id,input:{revision:2,requestId:randomUUID()}};
  step('Repository-free research queues',research);step('Repository-free research replay',{...research,input:reverse(research.input)},false,{revision:'2.0'});
  step('Research changed limit refuses',{...research,input:{...research.input,activeLimitMs:1000}},true);
  const insertion={method:'proposeOperatorSpecialist',runId:insertRun.id,role,model,input:{requestId:randomUUID(),roleId:role.id,input:'Inspect',criteria:[0]}};
  step('Insertion normalizes numeric criterion',insertion,false,{criteria:'0.0'});
  step('Insertion reordered input replay',{...insertion,input:reverse(insertion.input)});
  const other=join(root,'other');seedRepository(other);const alias=join(root,'alias');symlinkSync(other,alias);
  const check={id:'unit',image:'sha256:'+'a'.repeat(64),command:'/bin/true',args:[],timeoutMs:100,memoryMiB:128,tmpExecutable:false,required:true};
  const creation={method:'createRepository',input:{name:'Other',path:other,targetBranch:'main',checks:[check],requestId:randomUUID()}};
  const saved=step('Repository normalized reversed fields',{...creation,input:reverse(creation.input)},false,{timeoutMs:'100.0',memoryMiB:'128e0'});
  step('Repository creation exact replay',creation);step('Repository existing alias canonical replay',{...creation,input:{...creation.input,path:alias}});
  step('Repository changed content refuses',{...creation,input:{...creation.input,name:'Changed'}},true);
  step('Repository cross-kind identity refuses',{method:'setDependencies',taskId:plain.id,input:{requestId:creation.input.requestId,dependencyRevision:0,dependsOn:[]}},true);
  step('Duplicate canonical repository refuses',{...creation,input:{...creation.input,path:alias,requestId:randomUUID()}},true);
  const retirement={method:'retireRepository',id:saved.id,input:{revision:1,requestId:randomUUID()}};
  step('Repository retirement recorded',retirement);step('Retirement numeric replay',retirement,false,{revision:'1.0'});
  step('Creation receipt returns original saved result after retirement',creation);
  fixture('Legacy configuration without queue receipt',(db)=>{db.prepare('DELETE FROM queue_requests WHERE request_id=?').run(creation.input.requestId);});
  step('Legacy creation replay returns current retired row',creation);
  step('Legacy creation changed content refusal',{...creation,input:{...creation.input,name:'Changed'}},true);
  step('Legacy save rechecks creation receipt',{method:'saveRepository',input:reverse(creation.input)},false,{timeoutMs:'100e0',memoryMiB:'128.0'});
  renameSync(other,join(root,'moved'));
  step('Removed checkout canonical legacy replay',creation);
  step('Removed checkout old alias cannot match canonical receipt',{method:'repositoryReplay',input:{...creation.input,path:alias}},true);
  step('Removed checkout replay rejects changed field',{method:'repositoryReplay',input:{...creation.input,targetBranch:'changed'}},true);
  // A historical configured path may be a symlink. Duplicate detection canonicalizes it at commit.
  patch('repositories',repo.id,{path:join(root,'seed-alias')});symlinkSync(checkout,join(root,'seed-alias'));
  step('Canonical duplicate against historical alias refuses',{method:'createRepository',input:{name:'Duplicate',path:checkout,targetBranch:'main',requestId:randomUUID()}},true);
  const owned=join(root,'owned');seedRepository(owned);
  fixture('Managed worktree ownership fixture only',(db,map)=>{db.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(map(run.id),'fixture-common','fixture-branch',JSON.stringify(map({runId:run.id,path:owned})));});
  step('Commit refuses Crewroom-owned path',{method:'saveRepository',input:{name:'Owned',path:owned,targetBranch:'main'}},true);
  legacy.close();legacy=new Workbench(legacyPath,()=>now);compare('Final reopen');
  console.log(JSON.stringify({status:'passed',operations:cases.length,cases,fixtureBoundaries:fixtures,tables:Object.keys(snapshot(legacyPath)),claims:'Durable replay parity and disposable Git validation only; no agent or provider delivery. Private preflight tokens, concurrency and response latency are checked by Rust integration tests.'},null,2));
} finally {legacy.close();globalThis.Date=RealDate;rmSync(root,{recursive:true,force:true});}
