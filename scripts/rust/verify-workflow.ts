/** N2.1 transaction oracle: identical schema-26 inputs, TypeScript vs Rust HTTP.
 * Provider/attempt/publication rows are fixture state, never execution evidence. */
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { setTimeout as delay } from 'node:timers/promises';
import { Workbench } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { saveRepositoryFixture, seedRepository } from '../../tests/fixtures/repository';
import type { Run, Task } from '../../shared/types';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'cr-n21-')));
const native = join(root, 'native'); mkdirSync(native);
const legacyPath = join(root, 'legacy.sqlite'); const nativePath = join(native, 'workflow.sqlite');
let legacy: Workbench | undefined; let child: ReturnType<typeof spawn> | undefined;
let info: { origin: string; token: string }; let diagnostic = '';
const start = async () => {
  child = spawn(resolve('target/debug/crewroomd'), ['--data-dir',native,'--roles',join(root,'roles.json'),'--port','0'], {stdio:['ignore','pipe','pipe']});
  child.stderr!.on('data', b => diagnostic += b);
  for (let i=0; i<100 && !existsSync(join(native,'native-connection.json')); i++) {
    if (child.exitCode !== null) throw new Error(diagnostic); await delay(50);
  }
  info = JSON.parse(readFileSync(join(native,'native-connection.json'),'utf8'));
};
const stop = async () => { if (child && child.exitCode === null) { const done=once(child,'exit'); child.kill('SIGTERM'); await done; } child=undefined; };
const tables = ['tickets','runs','assignments','attempts','notes','steering','requirement_revisions','queue_requests','queue_state','coding_slot','team_proposals','delivery_notices','repository_work','publications','research_artifacts'];
const rows = (path: string) => {
  const db = new DatabaseSync(path, {readOnly:true});
  try { return Object.fromEntries(tables.map(table => [table, db.prepare(`SELECT * FROM ${table} ORDER BY rowid`).all().map(row =>
    Object.fromEntries(Object.entries(row).map(([key,value]) => [key, ['data','imported','refinement','payload','result'].includes(key) ? JSON.parse(String(value)) : value])))])); }
  finally { db.close(); }
};
// UUID/time allocation is intentionally nondeterministic; all other fields,
// embedded instructions, requirements hashes, receipts and ordering are compared.
const fixedIds = new Set<string>();
const normalize = (value: unknown) => {
  const ids = new Map<string,string>();
  const sorted = (v: any): any => Array.isArray(v) ? v.map(sorted) : v && typeof v === 'object'
    ? Object.fromEntries(Object.keys(v).sort().map(k => [k,sorted(v[k])])) : v;
  return JSON.parse(JSON.stringify(sorted(value)).replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, id => {
    if (fixedIds.has(id)) return id;
    if (!ids.has(id)) ids.set(id,`UUID-${ids.size}`); return ids.get(id)!;
  }).replace(/\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z/g,'TIMESTAMP'));
};
let steps = 0; const cases: string[] = [];
const durable = (path: string) => {
  const snapshot = rows(path);
  // TypeScript may persist this derived view while compacting queue positions.
  // Native computes it on read and retains an explicit unavailable-runner hold.
  for (const row of snapshot.runs) delete row.data.holdReason;
  return snapshot;
};
const compare = (label: string) => {
  const actual = normalize(durable(nativePath)); const expected = normalize(durable(legacyPath));
  for (const table of tables) assert.deepEqual(actual[table],expected[table],`${label}: ${table}`);
};
const post = async (path: string, body: unknown) => {
  const response = await fetch(`${info.origin}/api${path}`,{method:'POST',headers:{Authorization:`Bearer ${info.token}`,'Content-Type':'application/json'},body:JSON.stringify(body)});
  return {ok:response.ok,status:response.status,body:await response.json()};
};
const step = async (label: string, path: string, body: unknown, oracle: () => unknown, expectError = false) => {
  if (body && typeof body === 'object' && 'requestId' in body) fixedIds.add(String(body.requestId));
  let expected: unknown; let failed = false;
  try { expected=oracle(); } catch (error) { failed=true; if (!expectError) throw error; }
  assert.equal(failed,expectError,`${label}: oracle disposition`);
  const result=await post(path,body);
  assert.equal(result.ok,!expectError,`${label}: ${JSON.stringify(result.body)}`);
  if (expectError) assert.ok([400,404,409].includes(result.status));
  else assert.deepEqual(normalize(result.body),normalize(expected),`${label}: result`);
  compare(label); steps++; cases.push(label); return result.body;
};
try {
  legacy = new Workbench(legacyPath);
  const repoPath = join(root,'repo'); seedRepository(repoPath);
  const repo = saveRepositoryFixture(legacy,{name:'Oracle',path:repoPath,targetBranch:'main',checks:[]});
  const makeTask = (title: string, repositoryId?: string) => legacy!.createTask({title,description:'',repositoryId});
  const accept = (t: Task, criteria=['Original']) => legacy!.refine(t.id,{revision:t.revision,sourceRevision:t.sourceRevision,brief:'Accepted 日本語 🦀',criteria});
  const a=makeTask('A'), b=makeTask('B'), c=makeTask('C'), explore=makeTask('Explore');
  const research=accept(makeTask('Standalone')); const pinned=accept(makeTask('Pinned',repo.id));
  const queuedTask=accept(makeTask('Queued',repo.id)); const queued=legacy.queue(queuedTask.id,{revision:2});
  const followingTask=accept(makeTask('Following',repo.id)); const following=legacy.queue(followingTask.id,{revision:2});
  const activeTask=accept(makeTask('Admitted',repo.id),['Old','Keep','Removed']); const active=legacy.queue(activeTask.id,{revision:2});
  const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};
  const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
  const coordinator=legacy.admitAssignment(active.id,{name:'Coordinator',role,model,coordinator:true,input:'Coordinate'});
  legacy.admitAssignment(active.id,{name:'Specialist',role,model,coordinator:false,input:'Inspect',brief:{criteria:[{index:0,criterion:'Old',obligation:'Check',checkIds:[]},{index:2,criterion:'Removed',obligation:'Check',checkIds:[]}],dependsOn:[]}});
  legacy.setDesiredState(active.id,'running'); // Admitted only; no actual runner or timing clock.
  const revisedActive=accept(legacy.getTask(activeTask.id),['New','Keep']);
  const proposal=legacy.proposeOperatorSpecialist(active.id,{requestId:randomUUID(),roleId:role.id,input:'Inspect',criteria:[],dependsOn:[]},role,model);
  // A proposal captured before the next edit must be rejected by adoption.
  const latestActive=accept(revisedActive,['Newest','Keep']);
  const jiraInput={siteUrl:'https://fixture.atlassian.net',issues:[{id:'1',key:'SIM-1',fields:{summary:'Source',description:'Original source',status:{name:'Open'}}}]};
  let jira=legacy.importJira(jiraInput)[0]; jira=legacy.assignRepository(jira.id,{repositoryId:repo.id,revision:1}); jira=accept(jira);
  const sourceRun=legacy.queue(jira.id,{revision:jira.revision});
  const finishedTask=accept(makeTask('Completed research')); const finished=legacy.queueResearch(finishedTask.id,{revision:2,requestId:randomUUID()});
  const finishedEdit=accept(legacy.getTask(finishedTask.id),['More research']);
  legacy.close(); legacy=undefined;
  const seed = new DatabaseSync(legacyPath);
  const work={runId:active.id,repository:repo,path:repoPath,branch:'fixture',requirementsHash:'prior-hash',checkpoints:[]};
  seed.prepare('INSERT INTO repository_work VALUES(?,?,?,?)').run(active.id,repoPath,'fixture',JSON.stringify(work));
  seed.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify({...finished,status:'completed',desiredState:'cancelled',researchCompletion:{requestId:'prior'},researchOutcome:{revision:2,requestId:'prior'}}),finished.id);
  seed.exec('PRAGMA wal_checkpoint(TRUNCATE)'); seed.close(); copyFileSync(legacyPath,nativePath);
  for (const id of JSON.stringify(rows(legacyPath)).matchAll(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g)) fixedIds.add(id[0]);
  legacy=new Workbench(legacyPath); await start(); compare('seed');

  const deps={requestId:randomUUID(),dependencyRevision:0,dependsOn:[b.id]};
  await step('dependencies persist',`/tasks/${a.id}/dependencies`,deps,()=>legacy!.setDependencies(a.id,deps));
  await step('dependency receipt replay',`/tasks/${a.id}/dependencies`,deps,()=>legacy!.setDependencies(a.id,deps));
  for (const [label,dependsOn,revision] of [['self',[a.id],1],['duplicate',[b.id,b.id],1],['missing',[randomUUID()],1],['stale',[],0]] as const) {
    const body={requestId:randomUUID(),dependencyRevision:revision,dependsOn};
    await step(`reject ${label} dependency`,`/tasks/${a.id}/dependencies`,body,()=>legacy!.setDependencies(a.id,body),true);
  }
  const back={requestId:randomUUID(),dependencyRevision:0,dependsOn:[a.id]};
  await step('reject dependency cycle',`/tasks/${b.id}/dependencies`,back,()=>legacy!.setDependencies(b.id,back),true);
  const exploreInput={requestId:randomUUID(),purpose:'explore',revision:1};
  let oracleExplore: Run;
  const rustExplore=await step('queue Explore',`/tasks/${explore.id}/refinement-runs`,exploreInput,()=>oracleExplore=legacy!.queueRefinement(explore.id,exploreInput));
  const grill={requestId:randomUUID(),purpose:'grill',revision:1};
  await step('open-run exclusion',`/tasks/${explore.id}/refinement-runs`,grill,()=>legacy!.queueRefinement(explore.id,grill),true);
  const message={requestId:'pending',body:'Keep this instruction as history.'};
  await step('pending refinement steering',`/runs/${rustExplore.id}/steering`,message,()=>legacy!.steer(oracleExplore!.id,message));
  await step('close and settle undelivered',`/runs/${rustExplore.id}/close`,{},()=>legacy!.closeRefinement(oracleExplore!.id));
  await step('closed refinement replay',`/tasks/${explore.id}/refinement-runs`,exploreInput,()=>legacy!.queueRefinement(explore.id,exploreInput));
  await step('queue Grill',`/tasks/${explore.id}/refinement-runs`,grill,()=>legacy!.queueRefinement(explore.id,grill));
  const researchInput={requestId:randomUUID(),revision:2};
  await step('queue standalone research',`/tasks/${research.id}/research-runs`,researchInput,()=>legacy!.queueResearch(research.id,researchInput));
  await step('research replay',`/tasks/${research.id}/research-runs`,researchInput,()=>legacy!.queueResearch(research.id,researchInput));
  const pinnedInput={requestId:randomUUID(),revision:2};
  const commit=execFileSync('git',['-C',repoPath,'rev-parse','refs/heads/main'],{encoding:'utf8'}).trim();
  await step('queue research pinned to target',`/tasks/${pinned.id}/research-runs`,pinnedInput,()=>legacy!.queueResearch(pinned.id,pinnedInput,{repositoryId:repo.id,repositoryRevision:1,path:repoPath,commit}));

  const edit={revision:2,sourceRevision:1,brief:'Revised',criteria:['Newest']};
  await step('edit leaves queued snapshot intact',`/tasks/${queuedTask.id}/refinement`,edit,()=>legacy!.refine(queuedTask.id,edit));
  const held=await fetch(`${info.origin}/api/workspace`,{headers:{Authorization:`Bearer ${info.token}`}}).then(r=>r.json());
  assert.equal(held.queue.plans.find((p: Run) => p.id === queued.id).holdReason,'Requirements changed');
  const adopt={requestId:randomUUID(),revision:3};
  await step('queued adoption',`/runs/${queued.id}/adopt-requirements`,adopt,()=>legacy!.adoptRequirements(queued.id,adopt));
  const activeInput={requestId:randomUUID(),revision:latestActive.revision};
  await step('active adoption requires reason',`/runs/${active.id}/adopt-requirements`,activeInput,()=>legacy!.adoptRequirements(active.id,activeInput),true);
  const reasoned={...activeInput,reason:'  Accepted investigation result.  '};
  await step('active adoption pauses, rebriefs, rejects insertion and hashes scope',`/runs/${active.id}/adopt-requirements`,reasoned,()=>legacy!.adoptRequirements(active.id,reasoned));
  await step('adoption replay does not duplicate steering',`/runs/${active.id}/adopt-requirements`,reasoned,()=>legacy!.adoptRequirements(active.id,reasoned));
  const changed={...reasoned,reason:'Different'};
  await step('reject changed adoption identity',`/runs/${active.id}/adopt-requirements`,changed,()=>legacy!.adoptRequirements(active.id,changed),true);
  const refreshed=structuredClone(jiraInput); refreshed.issues[0].fields.summary='Changed upstream';
  await step('source change retains queued position', '/import/jira',refreshed,()=>legacy!.importJira(refreshed));
  const staleAdopt={requestId:randomUUID(),revision:jira.revision};
  await step('reject unaccepted source adoption',`/runs/${sourceRun.id}/adopt-requirements`,staleAdopt,()=>legacy!.adoptRequirements(sourceRun.id,staleAdopt),true);
  const reaccept={revision:jira.revision,sourceRevision:2,brief:'Accepted source',criteria:['Keep']};
  await step('accept source independently',`/tasks/${jira.id}/refinement`,reaccept,()=>legacy!.refine(jira.id,reaccept));
  const adoptSource={requestId:randomUUID(),revision:jira.revision+1};
  await step('adopt source clears hold',`/runs/${sourceRun.id}/adopt-requirements`,adoptSource,()=>legacy!.adoptRequirements(sourceRun.id,adoptSource));
  const reopen={requestId:randomUUID(),revision:finishedEdit.revision};
  await step('explicit research reopening clears completion intent',`/runs/${finished.id}/adopt-requirements`,reopen,()=>legacy!.adoptRequirements(finished.id,reopen));
  const cancel={requestId:randomUUID()};
  await step('queued cancellation compacts positions',`/queue/${queued.id}/remove`,cancel,()=>legacy!.cancelQueued(queued.id,cancel));
  await stop(); legacy.close(); legacy=new Workbench(legacyPath); await start();
  await step('adoption replay after daemon restart',`/runs/${active.id}/adopt-requirements`,reasoned,()=>legacy!.adoptRequirements(active.id,reasoned));
  await step('dependency replay after daemon restart',`/tasks/${a.id}/dependencies`,deps,()=>legacy!.setDependencies(a.id,deps));
  await step('cancellation replay after daemon restart',`/queue/${queued.id}/remove`,cancel,()=>legacy!.cancelQueued(queued.id,cancel));
  const refused=await post(`/runs/${active.id}/resume`,{}); assert.equal(refused.status,409); compare('execution remains fenced');
  const snapshot=await fetch(`${info.origin}/api/workspace`,{headers:{Authorization:`Bearer ${info.token}`}}).then(r=>r.json());
  assert.equal(snapshot.capabilities.execution,false);
  assert.deepEqual(snapshot.dependencyStates,legacy.snapshot().dependencyStates);
  await stop();
  const reopened=new Workbench(nativePath); try { assert.equal(reopened.getRun(active.id).status,'pausing'); assert.equal(reopened.teamProposals(active.id).find(p=>p.id===proposal.id)?.status,'rejected'); } finally {reopened.close();}
  console.log(JSON.stringify({passed:true,scope:'N2.1 durable transactions; no runner or provider execution',steps,cases,reopen:'Both runtimes and daemon restart',comparedTables:tables},null,2));
} finally { legacy?.close(); await stop(); rmSync(root,{recursive:true,force:true}); }
