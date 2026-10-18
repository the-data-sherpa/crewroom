import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {randomUUID} from 'node:crypto';
export interface Command {file:string;args:string[];timeout:number;maxBuffer:number}
export class NativeProviderConfig {
 private proc=spawn('target/debug/examples/provider-config-fixture',[],{stdio:'pipe'});private exit=once(this.proc,'exit');private stderr='';
 private pending=new Map<string,{resolve:(v:any)=>void,reject:(e:Error)=>void}>();readonly ready:Promise<any>;
 constructor(config:Record<string,unknown>,command:(spec:Command)=>Promise<string>){
  this.proc.stderr.on('data',c=>this.stderr+=c);this.ready=new Promise((resolve,reject)=>{
   createInterface({input:this.proc.stdout}).on('line',line=>{const v=JSON.parse(line);
    if(v.callback){void command(v.command).then(stdout=>this.write({callback:v.callback,stdout}),()=>this.write({callback:v.callback,error:true}));return;}
    const error=v.error?Object.assign(new Error(v.error),{kind:v.kind,requestSent:v.requestSent,status:v.status??undefined}):undefined;
    if(v.ready){if(error)reject(error);else resolve(v.metadata);return;}
    const job=this.pending.get(v.id);if(!job)return;this.pending.delete(v.id);if(error)job.reject(error);else job.resolve(v.value);
   });this.proc.once('error',reject);this.proc.once('exit',()=>{const error=new Error(this.stderr||'Native configuration fixture exited');reject(error);for(const job of this.pending.values())job.reject(error);this.pending.clear();});
  });this.write(config);
 }
 private write(v:unknown){if(!this.proc.stdin.destroyed)this.proc.stdin.write(JSON.stringify(v)+'\n',()=>{});}
 async call(op='probe',args:Record<string,unknown>={}){await this.ready;const id=randomUUID();return new Promise<any>((resolve,reject)=>{this.pending.set(id,{resolve,reject});this.write({id,op,...args});});}
 async close(){this.write({op:'close'});this.proc.stdin.end();await this.exit;}
}
