import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {DatabaseSync} from 'node:sqlite';
import {test} from 'node:test';
import {issue,jiraFixture,source,waitFor} from './jira-workflow-fixture';

test('native complete paginated Jira refresh retains order, local refinements and source identity across reopen',async t=>{
 let refreshed=false;const requests:string[]=[];
 const f=await jiraFixture(t,(req,res)=>{const u=new URL(req.url!,'http://fixture');requests.push(u.pathname);assert.equal(u.pathname,'/rest/api/3/search/jql');assert.equal(u.searchParams.get('jql'),'filter = 12345 AND assignee = "operator-1" ORDER BY Rank ASC');res.end(JSON.stringify(u.searchParams.has('nextPageToken')?{issues:[issue('1','Second')],isLast:true}:{issues:[issue('2','First',refreshed?'3':'1')],isLast:false,nextPageToken:'two'}));},{seed:false});
 assert.equal((await f.c.command('jira_snapshot')).state,'not_synced');await f.c.command('jira_refresh');let state=await f.snapshot();assert.deepEqual(state.tasks.map((t:any)=>t.key),['SIM-2','SIM-1']);assert.equal(state.runs.length,0);
 const task=state.tasks[0];await f.c.command('refine',{target:task.id,input:{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Operator refinement',criteria:['Retained criterion']}});
 refreshed=true;await f.c.command('jira_refresh');await f.reopen();state=await f.snapshot();assert.equal(state.tasks[0].brief,'Operator refinement');assert.equal(state.tasks[0].sourceStatus,'In Progress');assert.deepEqual((await f.c.command('jira_snapshot')).taskIds,state.tasks.map((t:any)=>t.id));assert.equal(requests.length,4);assert.equal(JSON.stringify(state).includes('synthetic-jira-token'),false);
 await f.reopen({jira:{...f.config.jira,source:{...source,filterId:'99'}}});assert.deepEqual(await f.c.command('jira_snapshot'),{source:{...source,filterId:'99'},state:'not_synced',taskIds:[]});
});

test('native refresh retains active issues outside the filter and never commits incomplete or stale input',async t=>{
 let mode='initial',retainedReads=0;
 const f=await jiraFixture(t,(req,res)=>{
  const u=new URL(req.url!,'http://fixture');
  if(u.pathname.endsWith('/issue/100')){retainedReads++;res.end(JSON.stringify(issue(mode==='bad-retained'?'999':'100','Changed outside filter','3','2026-09-30T00:00:00Z')));return;}
  if(mode==='partial'){if(u.searchParams.has('nextPageToken')){res.statusCode=503;res.end('private source failure');return;}res.end(JSON.stringify({issues:[issue('100','Partial title')],isLast:false,nextPageToken:'broken'}));return;}
  const issues=mode==='empty'||mode==='bad-retained'?[]:[issue('100',mode==='stale'?'Stale title':'Fix scrolling','1',mode==='stale'?'2026-09-27T00:00:00Z':'2026-09-28T00:00:00Z')];res.end(JSON.stringify({issues,isLast:true}));
 });
 await f.c.command('jira_refresh');const saved=await f.c.command('jira_snapshot');mode='partial';await f.c.command('jira_refresh');let current=await f.c.command('jira_snapshot');assert.equal(current.state,'unavailable');assert.deepEqual(current.taskIds,saved.taskIds);assert.equal(current.lastSyncedAt,saved.lastSyncedAt);assert.equal(JSON.stringify(current).includes('private source failure'),false);assert.equal((await f.c.command('task',{target:f.seed.task.id})).title,'Fix scrolling');
 mode='stale';await f.c.command('jira_refresh');assert.equal((await f.c.command('task',{target:f.seed.task.id})).title,'Fix scrolling');
 mode='bad-retained';await f.c.command('jira_refresh');assert.equal((await f.c.command('jira_snapshot')).state,'unavailable');assert.equal((await f.c.command('task',{target:f.seed.task.id})).title,'Fix scrolling');
 mode='empty';await f.c.command('jira_refresh');current=await f.c.command('jira_snapshot');assert.equal(current.state,'connected');assert.deepEqual(current.taskIds,[]);const task=await f.c.command('task',{target:f.seed.task.id});assert.equal(task.title,'Changed outside filter');assert.equal(task.brief,'Constrain the modal');assert.equal(task.needsReacceptance,true);const run=await f.c.command('run',{target:f.seed.run.id});assert.equal(run.desiredState,'paused');assert.ok(run.sourceConflict);
 const before=retainedReads;await f.c.command('desired',{target:f.seed.run.id,input:'cancelled'});await f.c.command('jira_refresh');assert.equal(retainedReads,before);assert.equal((await f.snapshot()).tasks.length,1);
});

test('native refresh is driven and shared through cancelled waiters and close drains it',async t=>{
 let held:any,reads=0;const f=await jiraFixture(t,(_req,res)=>{reads++;held=res;},{seed:false});
 const id=randomUUID();const first=f.c.command('jira_refresh',{},id).then(v=>({value:v}),e=>({error:String(e)}));await waitFor(()=>!!held);
 const rest=Array.from({length:8},()=>f.c.command('jira_refresh'));await f.c.command('cancel_waiter',{requestId:id});assert.match((await first as any).error,/cancelled/);
 let closed=false;const closing=f.c.command('jira_sync_close').then(()=>{closed=true;});assert.equal((await f.c.command('jira_snapshot')).state,'not_synced');assert.equal(closed,false);
 held.end(JSON.stringify({issues:[issue('2','Shared complete observation')],isLast:true}));await Promise.all(rest);await closing;assert.equal(reads,1);assert.equal((await f.c.command('jira_snapshot')).state,'connected');await f.c.command('jira_refresh');assert.equal(reads,1);await f.c.command('jira_sync_close');
});

test('native sync rolls back all imported fields when the final SQLite transaction fails',async t=>{
 let changed=false;const f=await jiraFixture(t,(_req,res)=>{res.end(JSON.stringify({issues:[issue('2',changed?'Changed title':'Saved title','1',changed?'2026-09-30T00:00:00Z':'2026-09-28T00:00:00Z')],isLast:true}));},{seed:false});
 await f.c.command('jira_refresh');const previous=await f.c.command('jira_snapshot');const before=(await f.snapshot()).tasks[0];
 const db=new DatabaseSync(f.database);db.exec("CREATE TRIGGER refuse_import BEFORE UPDATE ON tickets BEGIN SELECT RAISE(ABORT,'fixture import failed'); END;");db.close();changed=true;await f.c.command('jira_refresh');
 const after=(await f.snapshot()).tasks[0],current=await f.c.command('jira_snapshot');assert.deepEqual(after,before);assert.equal(current.state,'unavailable');assert.equal(current.lastSyncedAt,previous.lastSyncedAt);assert.deepEqual(current.taskIds,previous.taskIds);assert.match(current.error,/No partial import/);
 const repair=new DatabaseSync(f.database);repair.exec('DROP TRIGGER refuse_import');repair.close();await f.c.command('jira_refresh');assert.equal((await f.snapshot()).tasks[0].title,'Changed title');
});
