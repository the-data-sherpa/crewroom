/** Disposable native workflow + local provider. Agent identities are seeded records, not live OMP proof. */
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {existsSync,mkdirSync,mkdtempSync,readFileSync,readdirSync,realpathSync,rmSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {type TestContext} from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {Workbench} from '../../server/workbench';
import {defaultRoles} from '../../server/roles';
import {resolveAssignment} from '../../server/runtime/model-policy';
import {seedRepository,saveRepositoryFixture} from '../../tests/fixtures/repository';
import {NativeRepository} from './repository-adapter';
import {ContainerCommand} from './container-adapter';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
import {dockerSocket} from './docker-engine';
export const source={siteUrl:'https://fixture.atlassian.net',cloudId:'00000000-0000-4000-8000-000000000022',filterId:'12345',assigneeAccountId:'operator-1',name:'Assigned work',inProgressStatusId:'3',inProgressTransitionId:'6',reviewStatusId:'10004',reviewTransitionId:'8',doneStatusId:'10003',doneTransitionId:'2'};
export const issue=(id='100',title='Fix scrolling',status='1',updated='2026-09-28T00:00:00.000Z')=>({id,key:`SIM-${id}`,fields:{summary:title,description:'Keep buttons visible',status:{id:status,name:status==='1'?'To Do':'In Progress'},updated}});
export async function body(req:IncomingMessage){const chunks:Buffer[]=[];for await(const chunk of req)chunks.push(chunk);return JSON.parse(Buffer.concat(chunks).toString());}
export async function waitFor(test:()=>boolean,description='fixture condition'){const until=Date.now()+10000;while(!test()){assert.ok(Date.now()<until,`Timed out waiting for ${description}`);await delay(5);}}
export async function jiraFixture(t:TestContext,handler:(req:IncomingMessage,res:ServerResponse)=>void|Promise<void>,options:{seed?:boolean,activate?:boolean,source?:any,evidence?:boolean,research?:boolean}={}){
 const provider=createServer((req,res)=>{assert.equal(req.headers.authorization,'Bearer synthetic-jira-token');void handler(req,res);});
 await new Promise<void>(resolve=>provider.listen(0,'127.0.0.1',resolve));
 const root=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-jira-workflow-'))),operator=join(root,'operator'),database=join(root,'workflow.sqlite'),credentialRoot=join(root,'credentials');
 mkdirSync(credentialRoot,{mode:0o700});writeFileSync(join(credentialRoot,'rules.json'),'{}');seedRepository(operator);
 const gitAt=(path:string,...args:string[])=>execFileSync('git',['-C',path,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid',...args],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trimEnd();
 writeFileSync(join(operator,'source.txt'),'before\n');gitAt(operator,'add','source.txt');gitAt(operator,'commit','-m','Fixture source');
 const store=new Workbench(database);let seed:any,repositoryId='';
 try{
  const repository=saveRepositoryFixture(store,{name:'Jira fixture',path:operator,targetBranch:'main'});repositoryId=repository.id;
  if(options.seed!==false){
   const task=store.importJira({siteUrl:source.siteUrl,issues:[issue()]})[0];store.assignRepository(task.id,{revision:1,repositoryId:repository.id});store.refine(task.id,{revision:2,sourceRevision:1,brief:'Constrain the modal',criteria:['Actions stay accessible']});const run=store.queue(task.id,{revision:3});
   const admit=(id:string,name:string,coordinator=false)=>{const role={...defaultRoles.find(r=>r.id===id)!,model:'fixture/model',effort:'low' as const};assert.ok(role.id,`missing role ${id}`);const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});return store.admitAssignment(run.id,{name,role,model,coordinator,input:'Fixture assignment.'});};
   seed={repository,task,run,coordinator:admit('coordinator','Coordinator',true),writer:admit('implementer','Writer'),reviewer:admit('reviewer','Reviewer'),reporter:admit('delivery-reporter','Reporter')};
  }
 }finally{store.close();}
 const config={database,root:join(root,'owned'),authority:join(root,'data'),jira:{source:options.source??source,apiUrl:`http://127.0.0.1:${(provider.address() as {port:number}).port}`,credentialRoot,evidence:options.evidence??true,research:options.research??true}};
 let c=new NativeRepository(config);await c.ready;
 if(seed && options.activate!==false)await c.command('activate',{target:seed.run.id});
 const releaseAll=()=>{for(const name of readdirSync(credentialRoot).filter(n=>n.startsWith('entered-')))writeFileSync(join(credentialRoot,name.replace('entered-','release-')),'release');};
 t.after(async()=>{releaseAll();await c.close();provider.closeAllConnections();await new Promise<void>(resolve=>provider.close(()=>resolve()));const runtime=join(config.authority,'runtime');if(existsSync(runtime))for(const entry of readdirSync(runtime,{recursive:true,withFileTypes:true}).filter(e=>e.name==='container.json')){const job=new ContainerCommand(entry.parentPath,{socket:dockerSocket()});const saved=JSON.parse(readFileSync(join(entry.parentPath,'container.json'),'utf8'));if(!saved.removed){await job.stop();await job.receipt();await job.remove();}assert.equal((await job.inspect()).state,'stopped');assert.equal(saved.removed,true,'Fixture retained an unremoved check container');}const operations=join(config.root,'.git-operations');if(existsSync(operations))for(const run of readdirSync(operations))for(const name of readdirSync(join(operations,run)).filter(n=>n.endsWith('.json'))){const d=JSON.parse(readFileSync(join(operations,run,name),'utf8')).descriptor;const state=await inspectCheckedOwned(d);if(state.state!=='stopped')await stopOwned(d.root);assert.equal(state.state,'stopped','Fixture left a live/uncertain Git child');}rmSync(root,{recursive:true,force:true});});
 return {root,operator,database,credentialRoot,seed,config,gitAt,repositoryId,get c(){return c;},
  /** Jira refresh is per project (FOR-53): confirm the fixture site for the fixture repository; it adopts the connection's saved filter. */
  project(){return c.command('jira_project',{target:repositoryId});},rules(value:any){writeFileSync(join(credentialRoot,'rules.json'),JSON.stringify(value));},entered(call:number){return waitFor(()=>existsSync(join(credentialRoot,`entered-${call}`)),`credentials ${call}`);},release(call:number){writeFileSync(join(credentialRoot,`release-${call}`),'release');},async reopen(overrides:Record<string,unknown>={}){releaseAll();await c.close();for(const name of readdirSync(credentialRoot).filter(n=>/^(entered|release)-/.test(n)))rmSync(join(credentialRoot,name));writeFileSync(join(credentialRoot,'rules.json'),'{}');c=new NativeRepository({...config,...overrides});await c.ready;},report(input:any,attempt=seed.reporter.attempt.id){return c.command('jira_report',{target:attempt,input});},updates(){return c.command('jira_updates',{target:seed.run.id});},flush(id?:string){return c.command('jira_flush',{},id);},snapshot(){return c.command('snapshot');}};
}
