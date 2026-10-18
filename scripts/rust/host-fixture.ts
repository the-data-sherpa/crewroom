import assert from 'node:assert/strict';
import { spawn, spawnSync, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { once } from 'node:events';
import { appendFileSync, existsSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { homedir,tmpdir } from 'node:os';
import { join,relative,resolve } from 'node:path';
import { createServer, type Server } from 'node:http';
import { setTimeout as sleep } from 'node:timers/promises';

import { ContainerCommand } from './container-adapter';

export async function until<T>(read:()=>T|Promise<T>,timeout=15000):Promise<Exclude<NonNullable<T>, false>> {
  const end=Date.now()+timeout;while(Date.now()<end){const value=await read();if(value)return value as Exclude<NonNullable<T>, false>;await sleep(10);}throw new Error('Fixture condition timed out');
}
export class NativeHost {
  root:string; config:any; proc:ChildProcessWithoutNullStreams; exited:Promise<any>; output='';error=''; origin='';
  constructor(backend:string,settings:Record<string,unknown>={},extra:Record<string,unknown>={}) {
    this.root=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-native-host-')));
    writeFileSync(join(this.root,'fixture-settings.json'),JSON.stringify(settings),{mode:0o600});
    const model={requested:{model:'fixture/model',effort:'low'},resolved:{provider:'fixture',id:'model',api:'openai-responses',selector:'fixture/model',effort:'low'}};
    this.config={root:this.root,helper:resolve('target/debug/crewroom-process'),token:'private-controller-token-123456',providerToken:'private-provider-token-654321',backend,attemptId:'fixture-attempt',model,upstreamProvider:{baseUrl:backend+'/v1',api:'openai-responses',apiKey:'upstream-fixture-secret',models:[{id:'model',contextWindow:1050000,maxTokens:128000}]},command:'/usr/bin/env',args:['python3','-u',resolve('scripts/rust/host-fixture-rpc.py')],env:{PATH:process.env.PATH,HOME:homedir(),PI_CONFIG_DIR:relative(homedir(),join(this.root,'omp-root'))},recipients:['Coordinator','Engineer'],secrets:['fixture-hidden'],...extra};
    writeFileSync(join(this.root,'host.json'),JSON.stringify(this.config),{mode:0o600});
    this.proc=settings.hostFileLimit
      ? spawn('/usr/bin/env',['python3','-c','import os,resource,signal,sys; resource.setrlimit(resource.RLIMIT_FSIZE,(int(sys.argv[1]),int(sys.argv[1]))); signal.signal(signal.SIGXFSZ,signal.SIG_IGN); os.execv(sys.argv[2],sys.argv[2:])',String(settings.hostFileLimit),resolve('target/debug/crewroom-host'),join(this.root,'host.json')],{stdio:'pipe'})
      : spawn('target/debug/crewroom-host',[join(this.root,'host.json')],{stdio:'pipe'});this.exited=once(this.proc,'exit');
    this.proc.stdout.on('data',chunk=>{this.output+=chunk;});this.proc.stderr.on('data',chunk=>{this.error+=chunk;});
  }
  async ready() {
    await until(()=>{if(this.proc.exitCode!==null)throw new Error(this.error+' '+this.events().map(v=>JSON.stringify(v)).join('\n'));return existsSync(join(this.root,'endpoint.json'));});
    this.origin='http://127.0.0.1:'+JSON.parse(readFileSync(join(this.root,'endpoint.json'),'utf8')).port;
    return this.value({op:'identity'});
  }
  async request(body:any,token=this.config.token) {const response=await fetch(this.origin,{method:'POST',headers:{Authorization:`Bearer ${token}`},body:JSON.stringify(body),signal:AbortSignal.timeout(15000)});return {status:response.status,value:await response.json() as any};}
  async value(body:any) {const r=await this.request(body);assert.equal(r.status,200,JSON.stringify(r.value));return r.value;}
  events():any[] {try{return readFileSync(join(this.root,'events.jsonl'),'utf8').trimEnd().split('\n').filter(Boolean).map(v=>JSON.parse(v));}catch{return [];}}
  requests():any[] {try{return readFileSync(join(this.root,'rpc-requests.jsonl'),'utf8').trimEnd().split('\n').filter(Boolean).map(v=>JSON.parse(v));}catch{return [];}}
  async terminal(id:string) {return until(()=>this.events().find(v=>v.event.type==='prompt_result'&&v.event.id===id)?.event);}
  input(id:string,action:any) {return this.value({op:'input',id,message:JSON.stringify(action)});}
  async close() {
    if(this.proc.exitCode===null&&this.proc.signalCode===null){try {if(this.origin)await this.request({op:'close'});}catch{} }
    if(this.proc.exitCode===null&&this.proc.signalCode===null){this.proc.kill('SIGTERM');const timer=new AbortController();await Promise.race([this.exited,sleep(5000,undefined,{signal:timer.signal}).catch(()=>{})]);timer.abort();}
    if(this.proc.exitCode===null&&this.proc.signalCode===null)this.proc.kill('SIGKILL');
    await this.exited;
    const ownership=spawnSync('target/debug/crewroom-process',['stop',join(this.root,'omp-process')],{encoding:'utf8',timeout:15000});
    assert.equal(ownership.status,0,ownership.stderr);assert.equal(JSON.parse(ownership.stdout).state,'stopped');
    if(existsSync(join(this.root,'tools'))){
      for(const name of readdirSync(join(this.root,'tools'))){
        const job=new ContainerCommand(join(this.root,'tools',name),this.config.commandEngine);
        assert.equal((await job.stop()).state,'stopped','Retaining unresolved fixture '+this.root);await job.remove();
      }
    }
    rmSync(this.root,{recursive:true,force:true});
  }
}
export async function backend():Promise<{server:Server,origin:string,requests:any[]}> {
  const requests:any[]=[];const server=createServer(async(req,res)=>{let bytes='';for await(const chunk of req)bytes+=chunk;requests.push({url:req.url,headers:req.headers,body:JSON.parse(bytes)});res.setHeader('Content-Type','application/json');res.end(JSON.stringify({stored:true}));});
  server.listen(0,'127.0.0.1');await once(server,'listening');return {server,origin:`http://127.0.0.1:${(server.address() as any).port}`,requests};
}
