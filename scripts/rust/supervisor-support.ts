/** Explicit seeded assignments and scripted RPC peer; real native lifecycle,
 * checked host/runner processes, tmux, SQLite and optional Docker ownership. */
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {existsSync,mkdirSync,mkdtempSync,readFileSync,readdirSync,realpathSync,rmSync,writeFileSync} from 'node:fs';
import {homedir,tmpdir} from 'node:os';
import {join,relative,resolve} from 'node:path';
import type {TestContext} from 'node:test';
import {Workbench} from '../../server/workbench';
import {defaultRoles} from '../../server/roles';
import {resolveAssignment} from '../../server/runtime/model-policy';
import {NativeSupervisor} from './supervisor-adapter';
import {backend,until} from './host-fixture';
import {inspectCheckedOwned,inspectOwned,sealCheckedOwned,stopOwned} from './process-adapter';
import {ContainerCommand} from './container-adapter';
import {dockerSocket} from './docker-engine';
export {until};
export const socket=dockerSocket();
export const image='sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285';
export async function supervisorFixture(t:TestContext,count=2,oneRun=false) {
 const root=realpathSync(mkdtempSync(join(tmpdir(),'cr-sup-'))),runtime=join(root,'runtime'),database=join(root,'workflow.sqlite');mkdirSync(runtime,{mode:0o700});
 const b=await backend(),store=new Workbench(database),seeds:any[]=[];
 const role={...defaultRoles[0],model:'fixture/model',effort:'low' as const};const model=resolveAssignment(role,[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['low'],tools:true}],{},{});
 try {for(let i=0;i<count;i++) {
  let task,run;if(oneRun&&i>0){task=seeds[0].task;run=seeds[0].run;}else{task=store.createTask({title:`Lifecycle ${i}`,description:''});store.refine(task.id,{revision:1,sourceRevision:1,brief:'Observe independent processes',criteria:['Preserve owned execution']});run=store.queueResearch(task.id,{revision:2,requestId:randomUUID()});run=store.setDesiredState(run.id,'running');}
  const admitted=store.admitAssignment(run.id,{name:oneRun&&i>0?'Engineer':'Coordinator',role,model,coordinator:!(oneRun&&i>0),input:'Fixture process assignment',brief:{criteria:[],dependsOn:[]}});seeds.push({task,run,...admitted});
 }}finally{store.close();}
 const config={database,root:runtime,authority:join(root,'data'),backend:b.origin,socket};let c=new NativeSupervisor(config);await c.ready;
 const attemptRoot=(i=0)=>join(runtime,seeds[i].attempt.id);
 const hostConfig=(i=0,settings:Record<string,unknown>={},extra:Record<string,unknown>={})=>{
  const dir=attemptRoot(i);mkdirSync(dir,{recursive:true,mode:0o700});writeFileSync(join(dir,'fixture-settings.json'),JSON.stringify(settings),{mode:0o600});
  return {providerToken:'synthetic-provider-token-private',model,command:'/usr/bin/env',args:['python3','-u',resolve('scripts/rust/host-fixture-rpc.py')],env:{PATH:process.env.PATH,HOME:homedir(),PI_CONFIG_DIR:relative(homedir(),join(dir,'omp-root')),TMPDIR:dir},upstreamProvider:{baseUrl:b.origin+'/v1',api:'openai-responses',apiKey:'synthetic-upstream-secret',models:[{id:'model',contextWindow:1050000,maxTokens:128000}]},recipients:['Coordinator','Engineer'],coordinator:true,...extra};
 };
 t.after(async()=>{
  await c.close();b.server.closeAllConnections();await new Promise<void>(r=>b.server.close(()=>r()));
  let unverified=false;
  for(const seed of seeds) {
   const dir=join(runtime,seed.attempt.id);if(!existsSync(dir))continue;
   const intent=join(dir,'host-intent.json');if(existsSync(intent)) {
    let d;try{d=JSON.parse(readFileSync(intent,'utf8')).descriptor;}catch{unverified=true;continue;}
    const before=await inspectCheckedOwned(d);if(before.state!=='stopped'){unverified=true;await stopOwned(d.root);}
   }
   const omp=await inspectOwned(join(dir,'omp-process'));if(omp.state!=='stopped'){unverified=true;await stopOwned(join(dir,'omp-process'));}
   if(existsSync(intent))assert.equal((await sealCheckedOwned(JSON.parse(readFileSync(intent,'utf8')).descriptor)).state,'stopped');
   if(existsSync(join(dir,'tools')))for(const name of readdirSync(join(dir,'tools'))) {
    const job=new ContainerCommand(join(dir,'tools',name),{socket});assert.equal((await job.inspect()).state,'stopped');
    const saved=JSON.parse(readFileSync(join(dir,'tools',name,'container.json'),'utf8'));assert.equal(saved.removed,true,'Native supervisor left a retained container unremoved');
   }
  }
  assert.equal(unverified,false,`Fixture required fallback cleanup; retained ${root}`);assert.equal(c.proc.exitCode,0,c.stderr);rmSync(root,{recursive:true,force:true});
 });
 return {root,runtime,database,seeds,config,b,attemptRoot,hostConfig,get c(){return c;},
  launch(i=0,settings:Record<string,unknown>={},extra:Record<string,unknown>={},requestId?:string){return c.command('launch',{target:seeds[i].attempt.id,revision:seeds[i].run.controlRevision,config:hostConfig(i,settings,extra)},requestId);},
  request(input:any,i=0){return c.command('request',{target:seeds[i].attempt.id,input});},
  async input(id:string,action:any,i=0){return this.request({op:'input',id,message:JSON.stringify(action)},i);},
  async observed(i=0,state='active'){return until(async()=>{const result=await c.command('observation',{target:seeds[i].run.id});return result.find((v:any)=>v.observation.state===state);});},
  events(i=0){return c.command('events',{target:seeds[i].run.id});},
  stop(i=0){return c.command('stop',{target:seeds[i].run.id});},
  tmux(...args:string[]){return execFileSync('/opt/homebrew/bin/tmux',['-S',join(runtime,'tmux.sock'),'-f','/dev/null',...args],{encoding:'utf8',timeout:10000});},
  async kill(){c.proc.kill('SIGKILL');await c.exit;},
  async reopen(extra:Record<string,unknown>={}) {assert.ok(c.proc.exitCode!==null||c.proc.signalCode!==null,'Kill/close controller before reopening');c=new NativeSupervisor({...config,...extra});await c.ready;},
 };
}
