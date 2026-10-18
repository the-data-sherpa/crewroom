// Historical DDL is executed to the requested version before representative rows
// are inserted. This never creates schema 26 and relabels it as an old database.
import { strict as assert } from 'node:assert';
import { DatabaseSync } from 'node:sqlite';
import { build } from 'esbuild';
import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, readFileSync, realpathSync, symlinkSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import { Workbench } from '../../server/workbench';
const root=resolve(process.argv[2] ?? `.crewroom/test-results/native-upgrades/parity-${Date.now()}`);
mkdirSync(resolve(root,'..'),{recursive:true,mode:0o700});
mkdirSync(root,{mode:0o700});
const NOW=1791000123456;
const AT='2026-10-01T00:00:00.000Z';
const source=readFileSync('server/workbench.ts','utf8');
const binary=resolve(process.env.CREWROOM_UPGRADE_BINARY ?? 'target/debug/examples/upgrade-store');
const report:any={kind:'Actual historical-schema migration differential',sourceSha256:createHash('sha256').update(source).digest('hex'),clock:NOW,cases:[],limitations:['Scripted durable records; no live provider or process ownership is inferred.']};
async function historical(version:number,path:string) {
  const capped=source.replace(/if \(version < (\d+)\)/g,(_,v)=>`if (version < ${v} && ${v} <= ${version})`);
  const result=await build({stdin:{contents:capped,resolveDir:resolve('server'),sourcefile:'workbench.ts',loader:'ts'},bundle:true,platform:'node',format:'esm',packages:'external',write:false});
  const modulePath=join(root,`historical-${version}.mjs`);writeFileSync(modulePath,result.outputFiles[0].text);
  const {Workbench:Historical}=await import(pathToFileURL(modulePath).href);
  new Historical(path,()=>NOW).close();
  const db=new DatabaseSync(path);assert.equal(db.prepare('PRAGMA user_version').get()!.user_version,version);
  return db;
}
function tables(db:DatabaseSync) {return new Set(db.prepare("SELECT name FROM sqlite_master WHERE type='table'").all().map(r=>String(r.name)));}
function insert(db:DatabaseSync,table:string,values:any[]) {const q=db.prepare(`INSERT INTO ${table} VALUES(${values.map(()=>'?').join(',')})`);q.run(...values.map(v=>typeof v==='object' && v!==null?JSON.stringify(v):v));}
function seed(db:DatabaseSync,version:number) {
 const names=tables(db);
 const ticket=(n:number,extra:any={})=>({id:`ticket-${n}`,sourceRevision:7,updatedAt:AT,title:`Ticket ${n}`,description:'Retained source',...extra});
 const refinement=(n:number)=>({revision:3,brief:`Retained brief ${n}`,criteria:['preserve changes'],needsReacceptance:n===3||n===8,...(n===2?{acceptedSourceRevision:4}:{})});
 for(let n=1;n<=9;n++)insert(db,'tickets',[`ticket-${n}`,null,null,ticket(n),refinement(n)]);
 const run=(n:number)=>({id:`run-${n}`,taskId:`ticket-${n}`,status:n===5?'cancelled':n===3||n===4?'awaiting_runner':'running',desiredState:n===5?'cancelled':'running',createdAt:n===4?'2026-09-30T00:00:00.000Z':AT,taskSnapshot:{sourceRevision:5},activeMs:12,activeSince:NOW-100,...(version>=20&&(n===3||n===4)?{position:n===4?0:1}:{}),...(version>=21&&n===1?{schedule:{id:'old-schedule',at:AT},launchCompletedAt:AT}:{})});
 if(names.has('repositories'))insert(db,'repositories',['repo',{id:'repo',path:'/disposable/repository',guidance:[]}]);
 if(names.has('runs'))for(let n=1;n<=7;n++)insert(db,'runs',[`run-${n}`,`ticket-${n}`,run(n)]);
 if(names.has('notes'))insert(db,'notes',['note','ticket-1',{id:'note',body:'Retained operator decision'}]);
 if(names.has('steering'))for(const [id,n,delivery] of [['pending',5,'pending'],['dispatched',5,'dispatched'],['received',5,'received'],['active',1,'pending']] as const)insert(db,'steering',[id,`run-${n}`,`key-${id}`,{id,runId:`run-${n}`,body:'Saved guidance',delivery,createdAt:AT}]);
 if(names.has('assignments'))for(const n of [1,2,6,7]) {
  insert(db,'assignments',[`assignment-${n}`,`run-${n}`,'Coordinator',{id:`assignment-${n}`,runId:`run-${n}`,currentAttemptId:`attempt-${n}`,coordinator:true}]);
  insert(db,'attempts',[`attempt-${n}`,`assignment-${n}`,{id:`attempt-${n}`,runId:`run-${n}`,assignmentId:`assignment-${n}`,state:n===6?'admitted':'idle',ownership:n===6?'stopped':'held'}]);
 }
 if(names.has('peer_messages'))insert(db,'peer_messages',['peer','run-1','attempt-1','peer-tool',{id:'peer',delivery:'received',body:'Retained Mail receipt'}]);
 if(names.has('observer_cursors')) {insert(db,'observer_cursors',['assignment-1',41]);insert(db,'observations',['assignment-1',41,{cursor:41,event:{type:'state'}}]);}
 if(names.has('agent_events')) {
  const events:any[]=[
   [2,1,{type:'message_update',messageId:'done',assistantMessageEvent:{type:'text_delta',contentIndex:0,partial:'heavy'}}],
   [5,5,{type:'message_end',messageId:'done',message:{id:'done',role:'assistant',content:[{type:'text',text:'Final answer'}]}}],
   [8,8,{type:'message_update',messageId:'ongoing',assistantMessageEvent:{type:'text_delta',contentIndex:1,partial:'retained progress'}}],
   [11,9,{type:'message_update',messageId:'mismatch',assistantMessageEvent:{type:'text_delta'}}],
   [17,10,{type:'message_end',messageId:'mismatch',message:{id:'another',content:[]}}],
   [90,4,{type:'message_update',messageId:'done',assistantMessageEvent:{type:'text_delta'}}],
  ];
  for(const [cursor,sequence,event] of events)db.prepare('INSERT INTO agent_events(rowid,attempt_id,sequence,data) VALUES(?,?,?,?)').run(cursor,'attempt-1',sequence,JSON.stringify(event));
 }
 if(names.has('team_proposals'))insert(db,'team_proposals',['team','run-1','attempt-1','team-tool',{id:'team',status:'pending'}]);
 if(names.has('repository_work'))for(const n of [1,2,6,7])insert(db,'repository_work',[`run-${n}`,'/disposable/repository/.git',`work-${n}`,{runId:`run-${n}`,path:`/disposable/work-${n}`,branch:`work-${n}`,refreshes:[]}]);
 if(names.has('coding'))insert(db,'coding',['run-1',{runId:'run-1',claims:[],candidates:[],completions:[],checks:[],reviews:[]}]);
 if(names.has('jira_sync'))insert(db,'jira_sync',[1,{state:'unavailable',pending:['retained']}]);
 if(names.has('jira_updates'))insert(db,'jira_updates',['update','run-1','milestone',{id:'update',state:'pending'}]);
 if(names.has('publications'))insert(db,'publications',['run-7',{runId:'run-7',merged:{commit:'merged-head',observedAt:AT}}]);
 if(names.has('delivery_notices'))insert(db,'delivery_notices',['notice','run-1',{id:'notice',state:'received'}]);
 if(names.has('learning_proposals'))insert(db,'learning_proposals',['learning','run-1','attempt-1','learn-tool',{id:'learning',state:'proposed'}]);
 if(names.has('refinement_proposals'))insert(db,'refinement_proposals',['refinement','run-1','attempt-1','refine-tool',{id:'refinement',state:'pending'}]);
 if(names.has('research_artifacts'))insert(db,'research_artifacts',['artifact','run-1','attempt-1','artifact-tool',{id:'artifact',sha256:'retained'}]);
 if(names.has('queue_requests'))insert(db,'queue_requests',['queue-request','reorder',{order:['run-4','run-3']},{revision:2}]);
 if(names.has('coding_slot'))db.prepare('UPDATE coding_slot SET run_id=? WHERE id=1').run('run-1');
 if(version>=25)insert(db,'team_proposals',['operator','run-1',null,'operator-request',{id:'operator',origin:'operator'}]);
 if(version>=26)insert(db,'repository_work',['run-3','/disposable/repository/.git','work-1',{runId:'run-3',transferredTo:'run-1',retained:'historical'}]);
}
function snapshot(path:string) {
 const db=new DatabaseSync(path); const names=[...tables(db)].sort(); const output:any={version:db.prepare('PRAGMA user_version').get()!.user_version,tables:{},indexes:[]};
 for(const table of names) {
   const columns=db.prepare(`PRAGMA table_xinfo(${table})`).all();
   const rows=db.prepare(`SELECT rowid AS _rowid,* FROM ${table} ORDER BY rowid`).all().map(row=>Object.fromEntries(Object.entries(row).map(([key,value])=> {
    if(['data','imported','refinement','payload','result'].includes(key) && typeof value==='string')value=JSON.parse(value);
    return [key,value];
   })));
   const indexes=db.prepare(`PRAGMA index_list(${table})`).all().map(index=>({...index,columns:db.prepare(`PRAGMA index_xinfo(${String(index.name)})`).all()}));
   output.tables[table]={columns,foreignKeys:db.prepare(`PRAGMA foreign_key_list(${table})`).all(),indexes,rows};
 }
 output.indexes=db.prepare("SELECT name,tbl_name,sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL ORDER BY name").all().map(r=>({...r,sql:String(r.sql).replace(/\s+/g,' ').trim()}));
 assert.deepEqual(db.prepare('PRAGMA foreign_key_check').all(),[]);db.close();return output;
}
function normalize(value:any):any {
 if(Array.isArray(value))return value.map(normalize);
 if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).map(([k,v])=>[k,k==='schedule'&&v&&typeof v==='object'?{...normalize(v),id:(v as any).id==='old-schedule'?'old-schedule':'<generated-v4>'}:normalize(v)]));
 return value;
}
async function check(name:string,version:number,edit?:(db:DatabaseSync)=>void,fail=false) {
 const original=join(root,`${name}-original.sqlite`); const db=await historical(version,original);seed(db,version);edit?.(db);db.close();
 const rust=join(root,`${name}-rust.sqlite`),ts=join(root,`${name}-ts.sqlite`);copyFileSync(original,rust);copyFileSync(original,ts);
 let tsError:string|undefined;try{new Workbench(ts,()=>NOW).close();}catch(e){tsError=String((e as Error).message);}
 const result=spawnSync(binary,[rust,String(NOW)],{encoding:'utf8'}); const receipt=JSON.parse(result.stdout.trim());
 if(fail){assert.equal(receipt.ok,false);assert.ok(tsError);assert.deepEqual(snapshot(rust),snapshot(original));assert.deepEqual(snapshot(ts),snapshot(original));}
 else {
  assert.equal(receipt.ok,true,`${name}: ${receipt.error}`);assert.equal(tsError,undefined);
  const a=snapshot(rust),b=snapshot(ts);
  for(const current of [a,b])for(const row of current.tables.runs.rows) {const generated=row.data.schedule?.id;if(generated&&generated!=='old-schedule')assert.match(generated,/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);}
  assert.deepEqual(normalize(a),normalize(b),name);
  const reopened=spawnSync(binary,[rust,String(NOW+100000)],{encoding:'utf8'});assert.equal(reopened.status,0);assert.deepEqual(snapshot(rust),a);
  // The deleted highest historical rowid remains reserved by AUTOINCREMENT.
  if (a.tables.attempts.rows.length) {const append=new DatabaseSync(rust);const before=Number(append.prepare("SELECT seq FROM sqlite_sequence WHERE name='agent_events'").get()?.seq ?? 0);append.prepare('INSERT INTO agent_events(attempt_id,sequence,data) VALUES(?,?,?)').run('attempt-1',1000,'{"type":"fixture_append"}');const cursor=Number(append.prepare('SELECT max(cursor) AS n FROM agent_events').get()!.n);assert.ok(cursor>before);append.close();}
  writeFileSync(join(root,`${name}-snapshot.json`),JSON.stringify(a,null,2));
 }
 report.cases.push({name,version,passed:true,atomicFailure:fail,tsError,rustError:receipt.error});console.log(`${name}: PASS`);
}
function work(db:DatabaseSync,n:number,refreshes:any[]) {const row=JSON.parse(String(db.prepare('SELECT data FROM repository_work WHERE run_id=?').get(`run-${n}`)!.data));row.refreshes=refreshes;db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(row),`run-${n}`);}
function retries(prefix:string) {return [{requestId:`${prefix}-old`,state:'conflicted',supersededBy:{requestId:`${prefix}-retry`,reason:'retry after inspect',at:AT}},{requestId:`${prefix}-retry`,state:'interrupted',at:AT,supersedes:{requestId:`${prefix}-old`,reason:'retry after inspect'}}];}
function promotion(db:DatabaseSync,kind:string) {
 const branch='work-1',requestId='promotion',operationId='completion',token='11111111-1111-4111-8111-111111111111';
 const directory=join(root,`promotion-${kind}`);mkdirSync(directory);const processRoot=join(realpathSync(directory),`${operationId}-${token}`);mkdirSync(processRoot);
 const env={GIT_CONFIG_GLOBAL:'/disposable/none',GIT_CONFIG_NOSYSTEM:'1',GIT_SSH_COMMAND:'/usr/bin/false',GIT_TERMINAL_PROMPT:'0',HOME:'/disposable/home',LANG:'C',PATH:'/usr/bin:/bin',SSH_AUTH_SOCK:'/disposable/socket'};
 const spec={version:1,token,command:'/usr/bin/git',args:['reset','--keep','--no-recurse-submodules','resolved-head'],cwd:'/disposable/work-1',env,epoch:'fixture-epoch',controllerRoot:'/disposable/controller'};
 const raw=JSON.stringify(spec);const specSha256=createHash('sha256').update(raw).digest('hex');
 const process={...spec,root:processRoot,specSha256};
 if(kind!=='missing-manifest')writeFileSync(join(processRoot,'launch.json'),kind==='bad-manifest'?'{}':raw);
 writeFileSync(join(processRoot,'seal.json'),JSON.stringify({token,specSha256,...(kind==='extra-seal-key'?{other:true}:{})}));
 if(kind==='symlink-root'){const link=join(directory,'alias');symlinkSync(processRoot,link);process.root=link;}
 const refresh:any={requestId,state:'conflicted',fromHead:'original-head',fromBase:'original-base',toBase:'target',sourceCandidateId:'source',resolutionWorkspace:{state:'operator_required',operationId,toHead:'resolved-head',branch:`${branch}-refresh-${requestId}`,promotionProcess:process}};
 const completion:any={id:operationId,attemptId:'attempt-1',conflictRefreshId:requestId,state:'rejected',head:'original-head',rejection:{code:'promotion_interrupted',expectedTarget:'target',at:AT,reason:'Retained checked interruption'}};
 const record:any={runId:'run-1',candidates:[{id:'source',revision:{base:'original-base',head:'original-head'}}],claims:[{writerAttemptId:'attempt-1',targetRefreshId:requestId}],completions:[completion]};
 if(kind==='missing-source')delete refresh.sourceCandidateId;
 if(kind==='split-candidate')record.candidates[0].revision.head='other-head';
 if(kind==='split-claim')record.claims[0].targetRefreshId='other-refresh';
 if(kind==='already-promoted')completion.candidateId='published';
 if(kind==='bad-command')process.args[0]='clean';
 if(kind==='bad-rejection')completion.rejection.expectedTarget='foreign';
 work(db,1,[refresh]);db.prepare('UPDATE coding SET data=? WHERE run_id=?').run(JSON.stringify(record),'run-1');
}
try {
 for(let version=1;version<=26;version++)await check(`schema-${version}`,version);
 await check('retry-hold',21,db=>work(db,1,retries('one')));
 await check('retry-late-rollback',21,db=>{work(db,1,retries('one'));const malformed=retries('two');malformed[1].supersedes!.reason='different';work(db,2,malformed);},true);
 await check('legacy-workspace-hold',22,db=>work(db,1,[{requestId:'legacy',state:'conflicted',resolutionWorkspace:{state:'conflicted',path:'/disposable/retained-workspace'}}]));
 await check('legacy-rebasing-hold',22,db=>work(db,1,[{requestId:'legacy',state:'rebasing'}]));
 for(const kind of ['sealed','missing-manifest','bad-manifest','extra-seal-key','symlink-root','missing-source','split-candidate','split-claim','already-promoted','bad-command','bad-rejection']) await check(`promotion-${kind}`,22,db=>promotion(db,kind));
 report.passed=true;
} finally {writeFileSync(join(root,'report.json'),JSON.stringify(report,null,2)+'\n');}
