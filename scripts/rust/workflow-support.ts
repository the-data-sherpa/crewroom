/** Full native workflow library through a disposable JSONL controller. Backend
 * HTTP forwarding and Mail are explicit loopback fixtures, not daemon parity. */
import assert from 'node:assert/strict';
import {spawn,execFileSync,type ChildProcessWithoutNullStreams} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {createServer} from 'node:http';
import {mkdtempSync,mkdirSync,realpathSync,readFileSync,writeFileSync,readdirSync,existsSync,rmSync} from 'node:fs';
import {homedir,tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {setTimeout as sleep} from 'node:timers/promises';
import type {TestContext} from 'node:test';
import {Workbench} from '../../server/workbench';
import {RepositoryWork} from '../../server/repository-work';
import {defaultRoles} from '../../server/roles';
import {seedRepository,saveRepositoryFixture} from '../../tests/fixtures/repository';
import {inspectCheckedOwned,inspectOwned,stopOwned} from './process-adapter';
import {until} from './host-fixture';
import {NativeDaemon} from './daemon-client';
export {until};
export class NativeWorkflow {
 proc:ChildProcessWithoutNullStreams;ready:Promise<void>;exit:Promise<any>;stderr='';
 pending=new Map<string,{resolve:(v:any)=>void,reject:(e:Error)=>void}>();
 constructor(config:any){
  this.proc=spawn('target/debug/examples/workflow-fixture',[],{stdio:'pipe'});this.exit=once(this.proc,'exit');
  this.proc.stderr.on('data',b=>{this.stderr+=b;});
  this.ready=new Promise((resolve,reject)=>{
   createInterface({input:this.proc.stdout}).on('line',line=>{const v=JSON.parse(line);if(v.ready){resolve();return;}const job=this.pending.get(v.id);if(!job)return;this.pending.delete(v.id);v.error?job.reject(new Error(v.error)):job.resolve(v.value);});
   this.proc.once('error',reject);this.proc.once('exit',()=>{const e=new Error(this.stderr||'Workflow fixture exited');reject(e);for(const p of this.pending.values())p.reject(e);this.pending.clear();});
  });this.proc.stdin.write(JSON.stringify(config)+'\n');
 }
 async command(op:string,args:Record<string,unknown>={},id:string=randomUUID()){await this.ready;return new Promise<any>((resolve,reject)=>{this.pending.set(id,{resolve,reject});this.proc.stdin.write(JSON.stringify({id,op,...args})+'\n',e=>{if(e){this.pending.delete(id);reject(e);}});});}
 async close(){if(this.proc.exitCode===null&&this.proc.signalCode===null){this.proc.stdin.end(JSON.stringify({op:'close'})+'\n');const timer=new AbortController();await Promise.race([this.exit,sleep(60000,undefined,{signal:timer.signal}).catch(()=>{})]);timer.abort();}if(this.proc.exitCode===null&&this.proc.signalCode===null)this.proc.kill('SIGKILL');await this.exit;}
}
export type FixtureCleanup = {passed:boolean, daemonExitCode:number|null, daemonSignal:string|null, fallback:boolean, processes:{attemptId:string,hostState:string,runnerState:string}[], errors:string[]};
export async function workflowFixture(t:TestContext,purpose:'research'|'coding'|'explore'|'grill'='research',specialists=1,pinned=false,options:{parent?:string,service?:boolean,jira?:{source:any,issue:any,apiUrl:string},cleanup?:(report:FixtureCleanup)=>void|Promise<void>,configure?:(context:{root:string,source:string,database:string,config:any,team:any[],roles:typeof defaultRoles})=>void|Promise<void>}={}){
 const root=realpathSync(mkdtempSync(join(options.parent??tmpdir(),'cr-wf-'))),runtime=join(root,'runtime'),database=join(root,options.service?'data':'','workflow.sqlite');mkdirSync(runtime,{mode:0o700});if(options.service)mkdirSync(join(root,'data'),{mode:0o700});
 const roles=defaultRoles.map(r=>({...r,model:'fixture/model',effort:'low' as const}));writeFileSync(join(root,'roles.json'),JSON.stringify(roles),{mode:0o600});
 const source=join(root,'source');mkdirSync(source);seedRepository(source);
 writeFileSync(join(source,'source.txt'),'before\n');execFileSync('git',['-C',source,'add','source.txt']);execFileSync('git',['-C',source,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','Seed source'],{stdio:'ignore'});
 let c:NativeWorkflow|NativeDaemon;let origin='';let mailOffline=false,mailGate:Promise<void>|undefined;let releaseMail:(()=>void)|undefined;
 const requests:any[]=[],sent=new Map<string,any>();
 const server=createServer(async(req,res)=>{try{
  let raw='';for await(const b of req)raw+=b;const body=JSON.parse(raw);res.setHeader('Content-Type','application/json');
  if(req.url==='/mcp/'){
   if(mailOffline){res.writeHead(503);res.end('{}');return;}
   if(body.method!=='tools/call'){res.end(body.id?JSON.stringify({id:body.id,result:{}}):'');return;}
   const {name,arguments:args}=body.params;requests.push({name,args});if(mailGate)await mailGate;
   let value:any={};
   if(name==='register_agent')value={name:args.name};
   else if(name==='send_message') {if(!sent.has(args.idempotency_key))sent.set(args.idempotency_key,{id:sent.size+1,subject:args.subject,body_md:args.body_md});value={count:1,deliveries:[{payload:{id:sent.get(args.idempotency_key).id}}]};}
   else if(name==='acknowledge_message')value={acknowledged:true};
   else if(name==='fetch_inbox_events')value={events:[],next_cursor:args.after,has_more:false};
   else if(name==='fetch_inbox')value=[...sent.values()];
   res.end(JSON.stringify({id:body.id,result:{structuredContent:value}}));return;
  }
  assert.equal(req.headers.origin,origin);assert.ok(req.url?.startsWith('/internal/'));
  const value=await c.command('tool',{route:req.url!.slice('/internal/'.length),authorization:req.headers.authorization,input:body});res.end(JSON.stringify(value));
 }catch(e){res.writeHead(409);res.end(JSON.stringify({error:e instanceof Error?e.message:String(e)}));}});
 server.listen(0,'127.0.0.1');await once(server,'listening');origin=`http://127.0.0.1:${(server.address() as any).port}`;
 const store=new Workbench(database);let run:any,task:any,repository:any;
 try{
  if(purpose==='coding'||pinned)repository=saveRepositoryFixture(store,{name:'Native workflow',path:source,targetBranch:'main'});
  task=options.jira?store.importJira({siteUrl:options.jira.source.siteUrl,issues:[options.jira.issue]})[0]:store.createTask({title:'Full native workflow',description:'',...(repository?{repositoryId:repository.id}:{})});
  if(options.jira&&repository)task=store.assignRepository(task.id,{revision:task.revision,repositoryId:repository.id});
  store.refine(task.id,{revision:task.revision,sourceRevision:task.sourceRevision,brief:'Preserve workflow boundaries',criteria:['Retain independently verified evidence']});
  const revision=store.getTask(task.id).revision;
  run=purpose==='coding'?store.queue(task.id,{revision}):purpose==='research'?store.queueResearch(task.id,{revision,requestId:randomUUID()},repository?await new RepositoryWork(store,join(root,'seed-work')).pinResearch(repository):undefined):store.queueRefinement(task.id,{revision,requestId:randomUUID(),purpose});
 }catch(error){server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));rmSync(root,{recursive:true,force:true});throw error;}finally{store.close();}
 const team=[{name:'Coordinator',roleId:'coordinator',coordinator:true,input:'Coordinate this isolated workflow.'},...Array.from({length:specialists},(_,i)=>({name:`Specialist${i+1}`,roleId:purpose==='coding'?(i===0?'implementer':'reviewer'):'tech-writer',coordinator:false,input:'Investigate the accepted criterion.',criteria:[{index:0,obligation:'Verify the accepted criterion',checkIds:[]}]}))];
 const config:any={authority:join(root,'data'),database,repositories:join(root,'repositories'),backend:origin,roles:join(root,'roles.json'),environment:{PATH:process.env.PATH,HOME:homedir()},runtime:{root:runtime,socket:join(runtime,'tmux.sock'),ompBinary:'/fixture/omp',launchPrefix:['/usr/bin/env','python3','-u',resolve('scripts/rust/workflow-fixture-rpc.py')],mail:{url:origin+'/mcp/',token:'synthetic-mail-token',projectRoot:join(root,'mail-projects'),retentionSeconds:86400,atcDisabled:true},catalog:[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],policy:{},models:{providers:{fixture:{baseUrl:origin+'/v1',api:'openai-responses',apiKey:'synthetic-upstream-key',models:[{id:'model',contextWindow:1050000,maxTokens:128000}]}}},commandEngine:{socket:process.platform==='darwin'?join(homedir(),'.docker/run/docker.sock'):'/var/run/docker.sock'}}};
 if(purpose==='coding')config.runtime.launchPrefix.push('--unique-sessions');
 if(options.jira)config.jira={source:options.jira.source,apiUrl:options.jira.apiUrl};
 try{await options.configure?.({root,source,database,config,team,roles});
 c=options.service?new NativeDaemon(config):new NativeWorkflow(config);await c.ready;
 // Explicit operator acceptance through the same public plan gate used by the native workbench.
 if(purpose==='coding'){
  const current=await c.command('task',{target:task.id});
  const source=await c.command('plan_source',{target:task.id,input:{revision:current.revision}});
  const plan=await c.command('plan_save',{target:task.id,input:{requestId:randomUUID(),revision:current.revision,sourceId:source.id,content:{schemaVersion:1,goal:current.brief,scope:[current.brief],exclusions:[],assumptions:[],decisions:[],workPackages:[{id:'fixture',name:'Fixture work',dependsOn:[],outputs:['A checked change'],criteria:current.criteria.map((_:unknown,i:number)=>i),validation:['The surrounding workflow contract']} ]}}});
  await c.command('plan_decide',{target:plan.id,input:{requestId:randomUUID(),revision:current.revision,sourceId:source.id,decision:'accept'}});
  run=await c.command('run',{target:run.id});
 }
 }catch(error){server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));throw error;}
 t.after(async()=>{
  const report:FixtureCleanup={passed:false,daemonExitCode:null,daemonSignal:null,fallback:false,processes:[],errors:[]};
  releaseMail?.();
  try{await c.close();}catch(error){report.errors.push(String(error));}
  server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));
  for(const name of readdirSync(runtime)){
   const dir=join(runtime,name);if(!existsSync(join(dir,'host-intent.json')))continue;
   try{
    const descriptor=JSON.parse(readFileSync(join(dir,'host-intent.json'),'utf8')).descriptor;
    const hostState=(await inspectCheckedOwned(descriptor)).state;
    if(hostState!=='stopped'){report.fallback=true;await stopOwned(descriptor.root);}
    const launch=JSON.parse(readFileSync(join(dir,'host.json'),'utf8'));const adapter=launch.model?.resolved?.adapter;const processDir=adapter==='codex'?'codex-process':adapter==='claude-code'?'claude-process':'omp-process';
    const runnerState=(await inspectOwned(join(dir,processDir))).state;
    if(runnerState!=='stopped'){report.fallback=true;await stopOwned(join(dir,processDir));}
    report.processes.push({attemptId:name,hostState,runnerState});
   }catch(error){report.errors.push(String(error));}
  }
  report.daemonExitCode=c.proc.exitCode;report.daemonSignal=c.proc.signalCode;
  report.passed=!report.fallback&&report.errors.length===0&&report.daemonExitCode===0;
  if(options.cleanup){writeFileSync(join(root,'feature-cleanup.json'),JSON.stringify(report,null,2)+'\n',{mode:0o600});await options.cleanup(report);}
  assert.ok(report.passed,`${JSON.stringify(report)}; ${c.stderr}; retained ${root}`);
  if(process.env.CREWROOM_RETAIN_FIXTURE)t.diagnostic('Retained disposable fixture: '+root);else rmSync(root,{recursive:true,force:true});
 });
 const host=(attempt:string)=>JSON.parse(readFileSync(join(runtime,attempt,'host.json'),'utf8'));
 return {root,runtime,database,roles,team,config,run,task,repository,source,requests,sent,host,get c(){return c;},
  start(){return c.command('start',{target:run.id,input:{team},roles});},
  snapshot(){return c.command('snapshot',{target:run.id});},
  async idle(){return until(async()=>{const v=await this.snapshot();return !(await c.command('busy',{target:run.id}))&&v.assignments.length&&v.assignments.filter((a:any)=>!a.retiredAt).every((a:any)=>v.attempts.find((p:any)=>p.id===a.currentAttemptId)?.state==='idle')?v:false;});},
  async input(attempt:string,action:any,id:string=randomUUID()){if(action.kind==='tool'&&!action.id)action={...action,id:'workflow-tool-'+id};const h=host(attempt);const endpoint=JSON.parse(readFileSync(join(runtime,attempt,'endpoint.json'),'utf8'));const response=await fetch(`http://127.0.0.1:${endpoint.port}`,{method:'POST',headers:{Authorization:`Bearer ${h.token}`},body:JSON.stringify({op:'input',id,message:JSON.stringify(action)}),signal:AbortSignal.timeout(15000)});assert.ok(response.ok,await response.clone().text());return response.json();},
  tool(attempt:string,route:string,args:any,key:string=randomUUID()){return c.command('tool',{route,authorization:`Bearer ${host(attempt).token}`,input:{attemptId:attempt,toolCallId:key,arguments:args}});},
  pause(){return c.command('pause',{target:run.id});},resume(){return c.command('resume',{target:run.id});},
  offline(value:boolean){mailOffline=value;},
  holdMail(){mailGate=new Promise(r=>{releaseMail=r;});},releaseMail(){releaseMail?.();mailGate=undefined;},
  async kill(){c.proc.kill('SIGKILL');await c.exit;},
  async reopen(){assert.ok(c.proc.exitCode!==null||c.proc.signalCode!==null);c=options.service?new NativeDaemon(config):new NativeWorkflow(config);await c.ready;return c.command('initialize');}
 };
}

/** Operator acceptance for additional queued fixture tasks; no execution authority. */
export async function acceptFixturePlan(c:NativeDaemon,taskId:string){
 const task=await c.command('task',{target:taskId});
 const source=await c.command('plan_source',{target:taskId,input:{revision:task.revision}});
 const prior=(await c.request('/api/workspace')).intake[taskId].plans.at(-1);
 const content=prior?.content??{schemaVersion:1,goal:task.brief,scope:[task.brief],exclusions:[],assumptions:[],decisions:[],workPackages:[{id:'fixture',name:'Fixture work',dependsOn:[],outputs:['Checked behavior'],criteria:task.criteria.map((_:unknown,i:number)=>i),validation:['Required fixture check']}]};
 const plan=await c.command('plan_save',{target:taskId,input:{requestId:randomUUID(),revision:task.revision,sourceId:source.id,...(prior?{predecessorId:prior.id}:{}),content}});
 await c.command('plan_decide',{target:plan.id,input:{requestId:randomUUID(),revision:task.revision,sourceId:source.id,decision:'accept'}});
 return plan;
}
