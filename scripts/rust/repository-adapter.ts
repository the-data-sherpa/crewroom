/** Disposable JSONL adapter to the native repository service and disposable schema-26 store. */
import {spawn,type ChildProcessWithoutNullStreams} from 'node:child_process';
import {randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {setTimeout as sleep} from 'node:timers/promises';
import {dockerSocket} from './docker-engine';

export class NativeRepository {
  proc:ChildProcessWithoutNullStreams;ready:Promise<void>;exit:Promise<any>;stderr='';
  pending=new Map<string,{resolve:(v:any)=>void,reject:(e:Error)=>void}>();
  constructor(config:Record<string,unknown>,env:NodeJS.ProcessEnv=process.env) {
    this.proc=spawn('target/debug/examples/repository-fixture',[],{stdio:'pipe',env});this.exit=once(this.proc,'exit');
    this.proc.stderr.on('data',chunk=>{this.stderr+=chunk;});
    this.ready=new Promise((resolve,reject)=>{
      createInterface({input:this.proc.stdout}).on('line',line=>{
        const value=JSON.parse(line);if(value.ready){resolve();return;}
        const job=this.pending.get(value.id);if(!job)return;this.pending.delete(value.id);
        if(value.error){const error=value.kind==='timeout'?new DOMException(value.error,'TimeoutError'):new Error(value.error);job.reject(error);}else job.resolve(value.value);
      });
      this.proc.once('error',reject);this.proc.once('exit',()=>{const error=new Error(this.stderr||'Repository fixture exited');reject(error);for(const job of this.pending.values())job.reject(error);this.pending.clear();});
    });
    // The native fixture has no socket default of its own; checks use the shared resolver unless a test names a socket.
    this.proc.stdin.write(JSON.stringify({...config,socket:config.socket??dockerSocket(env)})+'\n');
  }
  async command(op:string,args:Record<string,unknown>={},id:string=randomUUID()) {await this.ready;return new Promise<any>((resolve,reject)=>{this.pending.set(id,{resolve,reject});this.proc.stdin.write(JSON.stringify({id,op,...args})+'\n',error=>{if(error){this.pending.delete(id);reject(error);}});});}
  call(name:string,args:Record<string,unknown>) {return this.command('call',{name,args});}
  async close() {
    if(this.proc.exitCode===null&&this.proc.signalCode===null){this.proc.stdin.end(JSON.stringify({op:'close'})+'\n');const timer=new AbortController();await Promise.race([this.exit,sleep(15000,undefined,{signal:timer.signal}).catch(()=>{})]);timer.abort();}
    if(this.proc.exitCode===null&&this.proc.signalCode===null)this.proc.kill('SIGKILL');await this.exit;
  }
}
