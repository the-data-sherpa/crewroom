// Test-only bridge: Docker and durable command decisions are all made in Rust.
import { spawn,execFile } from 'node:child_process';
import { resolve } from 'node:path';
import type { CommandSpec,ContainerEngine } from '../../server/runtime/container-command';
export type {CommandSpec,ContainerEngine};
const binary=resolve('target/debug/examples/container-fixture');
export class ContainerCommand{
 constructor(readonly root:string,private engine:ContainerEngine){}
 private call(op:string,extra:object={}):Promise<any>{return new Promise((ok,no)=>{
  const child=execFile(binary,[],{timeout:120000,maxBuffer:32*1024*1024},(error,stdout,stderr)=>{if(error)no(new Error(stderr||error.message));else try{ok(JSON.parse(stdout))}catch(e){no(e)}});
  child.stdin!.end(JSON.stringify({root:this.root,...this.engine,op,...extra}));
 });}
 start(spec:CommandSpec){return this.call('start',{spec})}
 inspect(){return this.call('inspect')}
 stop(){return this.call('stop')}
 wait(){return this.call('wait')}
 logs():Promise<string>{return this.call('logs')}
 receipt(){return this.call('receipt')}
 remove(){return this.call('remove')}
 follow(onChunk:(bytes:Buffer)=>void,signal:AbortSignal):Promise<void>{return new Promise((ok,no)=>{
  const child=spawn(binary,[],{stdio:'pipe'});let errors='';child.stdout.on('data',onChunk);child.stderr.on('data',b=>errors+=b);child.on('error',no);
  const abort=()=>child.kill('SIGTERM');signal.addEventListener('abort',abort,{once:true});
  child.on('exit',code=>{signal.removeEventListener('abort',abort);if(!signal.aborted&&code!==0)no(new Error(errors));else ok();});
  child.stdin.end(JSON.stringify({root:this.root,...this.engine,op:'follow'}));if(signal.aborted)abort();
 });}
}
