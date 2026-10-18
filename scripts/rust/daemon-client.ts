/** Disposable production daemon driver. All transitions cross its authenticated HTTP API. */
import assert from 'node:assert/strict';
import {type ChildProcessWithoutNullStreams} from 'node:child_process';
import {once} from 'node:events';
import {readFileSync,writeFileSync,existsSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {setTimeout as sleep} from 'node:timers/promises';
import {until} from './host-fixture';
import {spawnFixtureProcess} from './feature-owned-process';
export class NativeDaemon {
 proc:ChildProcessWithoutNullStreams;ready:Promise<void>;exit:Promise<any>;stderr='';info:any;
 constructor(readonly config:any){
  const path=join(config.authority,'native-runtime.json');
  const environment:NodeJS.ProcessEnv={...process.env,CREWROOM_TMUX:process.env.CREWROOM_TMUX??(process.platform==='darwin'?'/opt/homebrew/bin/tmux':'/usr/bin/tmux')};
  if(config.runtime){writeFileSync(path,JSON.stringify(config.runtime),{mode:0o600});environment.CREWROOM_RUNTIME_CONFIG=path;}
  else delete environment.CREWROOM_RUNTIME_CONFIG;
  const binaryDirectory=resolve(process.env.CREWROOM_NATIVE_BINARY_DIRECTORY??'target/debug');
  const providerFixture=!!(config.jira||config.publicationRemote);const fixturePath=join(config.authority,'provider-fixture.json');if(providerFixture)writeFileSync(fixturePath,JSON.stringify({...config,binaryDirectory}),{mode:0o600});
  this.proc=spawnFixtureProcess('daemon',providerFixture?resolve(config.providerFixtureBinary??join(binaryDirectory,'examples/provider-daemon')):join(binaryDirectory,'crewroomd'),providerFixture?[fixturePath]:['--data-dir',config.authority,'--roles',config.roles,'--port',String(config.nativePort??0)],environment);
  this.exit=once(this.proc,'exit');this.proc.stderr.on('data',b=>this.stderr+=b);this.proc.stdout.resume();
  this.ready=until(async()=>{
   if(this.proc.exitCode!==null||this.proc.signalCode!==null)throw new Error('Native daemon exited: '+this.stderr);
   const descriptor=join(config.authority,'native-connection.json');if(!existsSync(descriptor))return false;
   let info:any;try{info=JSON.parse(readFileSync(descriptor,'utf8'));}catch{return false;}
   if(info.pid!==this.proc.pid)return false;
   try{const response=await fetch(info.origin+'/api/health',{headers:{Authorization:`Bearer ${info.token}`},signal:AbortSignal.timeout(1000)});if(!response.ok)return false;}catch{return false;}
   this.info=info;config.nativePort=Number(new URL(info.origin).port);return true;
  },30000).then(()=>{});
 }
 async request(path:string,body?:any,token?:string,signal?:AbortSignal){
  await this.ready;const response=await fetch(this.info.origin+path,{method:body===undefined?'GET':'POST',headers:{Authorization:`Bearer ${token??this.info.token}`,Origin:this.info.origin,'Content-Type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:signal??AbortSignal.timeout(60000)});
  const value=await response.json();if(!response.ok)throw new Error(`${response.status}: ${value.error??JSON.stringify(value)}`);return value;
 }
 async command(op:string,args:Record<string,any>={},_requestId?:string){
  const run=`/api/runs/${args.target}`;
  switch(op){
   case 'task':return (await this.request('/api/workspace')).tasks.find((t:any)=>t.id===args.target);
   case 'plan_source':return this.request(`/api/tasks/${args.target}/plan-source`,args.input);
   case 'plan_save':return this.request(`/api/tasks/${args.target}/implementation-plans`,args.input);
   case 'plan_decide':return this.request(`/api/implementation-plans/${args.target}/decision`,args.input);
   case 'start':return this.request(run+'/start',args.input);
   case 'refinement_start':return this.request(run+'/start',{});
   case 'pause':case 'resume':case 'cancel':return this.request(run+'/'+op,{});
   case 'snapshot':case 'runtime':return this.request(run+'/runtime');
   case 'run':return (await this.request(run+'/runtime')).run;
   case 'busy':return (await this.request(run+'/runtime')).busy;
   case 'all':return this.request('/api/workspace');
   case 'steer':return this.request(run+'/steering',args.input);
   case 'steering':return (await this.request('/api/workspace')).messages.filter((s:any)=>s.runId===args.target);
   case 'events':{const events:any[]=[];let after=0;for(;;){const page=await this.request(run+'/events?limit=200&after='+after);events.push(...page.events);after=page.after;if(!page.hasMore)return events;}}
   case 'prepare':return this.request(run+'/repository',args.input);
   case 'evidence':return (await this.request(run+'/coding')).assessment;
   case 'record':return (await this.request(run+'/coding')).record;
   case 'publication_flush':return this.request(run+'/publication/reconcile',{});
   case 'publication':return (await this.request(run+'/result')).publication.record;
   case 'jira_flush':return this.request('/api/jira/reconcile',{});
   case 'jira_updates':return (await this.request('/api/workspace')).jiraUpdates.filter((u:any)=>u.runId===args.target);
   case 'research':return this.request(run+'/research');
   case 'limit':return this.request(run+'/budget',args.input);
   case 'allow':return this.request(run+`/assignments/${args.assignment}/repair-allowance`,args.input);
   case 'adopt':return this.request(run+'/requirements/adopt',args.input);
   case 'insert':return this.request(run+'/specialists',args.input);
   case 'tool':return this.request('/internal/'+args.route,args.input,String(args.authorization).replace(/^Bearer /,''));
   case 'initialize':return this.ready.then(()=>null);
   default:throw new Error('Unsupported daemon driver command: '+op);
  }
 }
 async close(){
  if(this.proc.exitCode===null&&this.proc.signalCode===null){this.proc.kill('SIGTERM');const abort=new AbortController();const ended=await Promise.race([this.exit.then(()=>true),sleep(60000,false,{signal:abort.signal}).catch(()=>true)]);abort.abort();if(!ended){this.proc.kill('SIGKILL');await this.exit;assert.fail('Daemon shutdown exceeded deadline: '+this.stderr);}}
  await this.exit;
 }
}
