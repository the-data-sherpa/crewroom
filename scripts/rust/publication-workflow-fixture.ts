/** Disposable native publication workflow + local Bitbucket provider. Agent identities are seeded records, not live OMP proof. */
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
import {checkImage,dockerSocket} from './docker-engine';
import {inspectCheckedOwned,stopOwned} from './process-adapter';
export const providerRepository={workspace:'fixture',slug:'repository',uuid:'{00000000-0000-4000-8000-000000000011}'};
export const source={siteUrl:'https://fixture.atlassian.net',cloudId:'00000000-0000-4000-8000-000000000022',filterId:'12345',assigneeAccountId:'operator-1',name:'Assigned work',inProgressStatusId:'3',inProgressTransitionId:'6',reviewStatusId:'10004',reviewTransitionId:'8',doneStatusId:'10003',doneTransitionId:'2'};
export const issue=(id='100',title='Fix scrolling',status='1',updated='2026-09-28T00:00:00.000Z')=>({id,key:`SIM-${id}`,fields:{summary:title,description:'Keep buttons visible',status:{id:status,name:status==='1'?'To Do':'In Progress'},updated}});
export async function body(req:IncomingMessage){const chunks:Buffer[]=[];for await(const chunk of req)chunks.push(chunk);return JSON.parse(Buffer.concat(chunks).toString());}
export async function waitFor(test:()=>boolean,description='fixture condition'){const until=Date.now()+10000;while(!test()){assert.ok(Date.now()<until,`Timed out waiting for ${description}`);await delay(5);}}
export async function publicationFixture(t:TestContext,handler:(req:IncomingMessage,res:ServerResponse)=>void|Promise<void>,options:{seed?:boolean,activate?:boolean,source?:any,evidence?:boolean,research?:boolean}={}){
 const provider=createServer((req,res)=>{assert.equal(req.headers.authorization,'Bearer synthetic-bitbucket-token');void handler(req,res);});
 await new Promise<void>(resolve=>provider.listen(0,'127.0.0.1',resolve));
 const root=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-publication-'))),operator=join(root,'operator'),database=join(root,'workflow.sqlite'),credentialRoot=join(root,'credentials');
 mkdirSync(credentialRoot,{mode:0o700});writeFileSync(join(credentialRoot,'rules.json'),'{}');seedRepository(operator);
 const gitAt=(path:string,...args:string[])=>execFileSync('git',['-C',path,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid',...args],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trimEnd();
 writeFileSync(join(operator,'source.txt'),'before\n');gitAt(operator,'add','source.txt');gitAt(operator,'commit','-m','Fixture source');
 const store=new Workbench(database);let seed:any;
 try{
  const repository=saveRepositoryFixture(store,{name:'Jira fixture',path:operator,targetBranch:'main'});
  if(options.seed!==false){
   const task=store.importJira({siteUrl:source.siteUrl,issues:[issue()]})[0];store.assignRepository(task.id,{revision:1,repositoryId:repository.id});store.refine(task.id,{revision:2,sourceRevision:1,brief:'Constrain the modal',criteria:['Actions stay accessible']});const run=store.queue(task.id,{revision:3});store.claimNextCodingSlot();
   const admit=(id:string,name:string,coordinator=false)=>{const role={...defaultRoles.find(r=>r.id===id)!,model:'fixture/model',effort:'low' as const};assert.ok(role.id,`missing role ${id}`);const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});return store.admitAssignment(run.id,{name,role,model,coordinator,input:'Fixture assignment.'});};
   seed={repository,task,run,coordinator:admit('coordinator','Coordinator',true),writer:admit('implementer','Writer'),reviewer:admit('reviewer','Reviewer'),reporter:admit('delivery-reporter','Reporter')};
  }
 }finally{store.close();}
 const remote=join(root,'remote.git');gitAt(operator,'clone','--bare',operator,remote);
 const connection={repositoryPath:operator,repository:providerRepository,policy:{statusKeys:['build'],pipelineSteps:['Tests'],optionalSteps:[]},fixtureGitPath:remote,apiUrl:`http://127.0.0.1:${(provider.address() as {port:number}).port}`};
 const config={database,root:join(root,'owned'),authority:join(root,'data'),socket:dockerSocket(),publicationRemote:{connections:[connection],credentialRoot}};
 let c=new NativeRepository(config);await c.ready;
 if(seed && options.activate===true)await c.command('activate',{target:seed.run.id});
 const releaseAll=()=>{for(const name of readdirSync(credentialRoot).filter(n=>n.startsWith('entered-')))writeFileSync(join(credentialRoot,name.replace('entered-','release-')),'release');};
 t.after(async()=>{releaseAll();await c.close();provider.closeAllConnections();await new Promise<void>(resolve=>provider.close(()=>resolve()));const runtime=join(config.authority,'runtime');if(existsSync(runtime))for(const entry of readdirSync(runtime,{recursive:true,withFileTypes:true}).filter(e=>e.name==='container.json')){const job=new ContainerCommand(entry.parentPath,{socket:config.socket});const saved=JSON.parse(readFileSync(join(entry.parentPath,'container.json'),'utf8'));if(!saved.removed){await job.stop();await job.receipt();await job.remove();}assert.equal((await job.inspect()).state,'stopped');assert.equal(saved.removed,true,'Fixture retained an unremoved check container');}const operations=join(config.root,'.git-operations');if(existsSync(operations))for(const run of readdirSync(operations))for(const name of readdirSync(join(operations,run)).filter(n=>n.endsWith('.json'))){const d=JSON.parse(readFileSync(join(operations,run,name),'utf8')).descriptor;const state=await inspectCheckedOwned(d);if(state.state!=='stopped')await stopOwned(d.root);assert.equal(state.state,'stopped','Fixture left a live/uncertain Git child');}const publication=join(config.authority,'publication');if(existsSync(publication))for(const entry of readdirSync(publication,{recursive:true,withFileTypes:true}).filter(e=>e.isFile() && e.name.endsWith('.json'))){const item=JSON.parse(readFileSync(join(entry.parentPath,entry.name),'utf8'));if(!item.descriptor)continue;const state=await inspectCheckedOwned(item.descriptor);if(state.state!=='stopped')await stopOwned(item.descriptor.root);assert.equal(state.state,'stopped','Publication left an unverified Git child');}rmSync(root,{recursive:true,force:true});});
 return {root,operator,database,credentialRoot,seed,config,connection,remote,gitAt,get c(){return c;},rules(value:any){writeFileSync(join(credentialRoot,'rules.json'),JSON.stringify(value));},entered(call:number){return waitFor(()=>existsSync(join(credentialRoot,`entered-${call}`)),`credentials ${call}`);},release(call:number){writeFileSync(join(credentialRoot,`release-${call}`),'release');},async reopen(overrides:Record<string,unknown>={}){releaseAll();await c.close();for(const name of readdirSync(credentialRoot).filter(n=>/^(entered|release)-/.test(n)))rmSync(join(credentialRoot,name));writeFileSync(join(credentialRoot,'rules.json'),'{}');c=new NativeRepository({...config,...overrides});await c.ready;},prepare(input:any,attempt=seed.coordinator.attempt.id){return c.command('publication_prepare',{target:attempt,input});},record(run=seed.run.id){return c.command('publication',{target:run});},flush(id?:string){return c.command('publication_flush',{},id);},snapshot(){return c.command('snapshot');}};
}

let image:string|undefined;
export async function validatedCandidate(f:Awaited<ReturnType<typeof publicationFixture>>){
 const run=f.seed.run.id;await f.c.command('prepare',{target:run,input:{mode:'new'}});
 image??=checkImage();const definition={id:'required',image,command:'/bin/sh',args:['-c','cat source.txt; test "$(head -n 1 source.txt)" = after'],timeoutMs:10000,memoryMiB:256,tmpExecutable:false,required:true};await f.c.command('configure',{target:run,input:[definition]});await f.c.command('activate',{target:run});
 await f.c.command('claim',{target:f.seed.writer.attempt.id});const read=await f.c.command('read',{target:run,path:'source.txt'});await f.c.command('edit',{target:f.seed.writer.attempt.id,key:'change',input:{path:'source.txt',expectedHash:read.sha256,content:'after\n'}});
 const completion=await f.c.command('request_completion',{target:f.seed.writer.attempt.id,key:'done',input:{summary:'Change fixture source'}});await f.c.command('pause',{target:run});for(const key of ['coordinator','writer','reviewer','reporter'])await f.c.command('ownership',{target:f.seed[key].attempt.id});await f.c.command('finish_pause',{target:run});await f.c.command('release',{target:f.seed.writer.attempt.id});const candidate=await f.c.command('complete',{target:run,key:completion.id});
 const agents:any={};for(const key of ['coordinator','writer','reviewer','reporter'])agents[key]=await f.c.command('replace',{target:f.seed[key].assignment.id,input:{automatic:false}});await f.c.command('activate',{target:run});
 const receipt=await f.c.command('execute_check',{target:agents.reviewer.id,key:'test',input:{checkId:'required'}});assert.equal(receipt.completed,true);assert.equal(receipt.exitCode,0);assert.equal(receipt.sourceIntact,true);
 await f.c.command('assign_review',{target:agents.coordinator.id,key:'review',input:{assignee:'Reviewer',candidateId:candidate.id,instruction:'Review exact source.'}});
 const review=await f.c.command('review',{target:agents.reviewer.id,input:{candidateId:candidate.id,verdict:'pass',summary:'The criterion is covered.',findings:[],coverage:[{criterion:0,assessment:'Checked exact source.',checkIds:['required']}]}});assert.equal((await f.c.command('assess',{target:run})).ready,true);
 return {candidate,agents,review,definition};
}
