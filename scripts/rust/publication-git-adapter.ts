/** Actual native checked Git, restricted by this fixture to a disposable local remote. */
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {randomUUID} from 'node:crypto';
import {dirname,join,resolve} from 'node:path';
export class NativePublicationGit {
 readonly proc;private exit:Promise<any>;private stderr='';private closedError?:string;
 private pending=new Map<string,{resolve:(v:any)=>void,reject:(e:Error)=>void}>();readonly ready:Promise<void>;
 constructor(readonly root:string,remote:{fixturePath:string},protectedBranches=['main','master'],options:{authority?:string;helper?:string;env?:NodeJS.ProcessEnv}={}){
  this.proc=spawn(resolve('target/debug/examples/publication-git-fixture'),[],{stdio:'pipe',env:options.env??process.env});this.exit=once(this.proc,'exit');
  this.proc.stderr.on('data',c=>this.stderr+=c);this.ready=new Promise((resolve,reject)=>{
   createInterface({input:this.proc.stdout}).on('line',line=>{const value=JSON.parse(line);if(value.closed){this.closedError=value.error??undefined;return;}const error=value.error?new Error(value.error):undefined;if(value.ready){if(error)reject(error);else resolve();return;}const job=this.pending.get(value.id);if(!job)return;this.pending.delete(value.id);if(error)job.reject(error);else job.resolve(value.value);});
   this.proc.once('error',reject);this.proc.once('exit',()=>{const error=new Error(this.stderr||'Native publication fixture exited');reject(error);for(const job of this.pending.values())job.reject(error);this.pending.clear();});
  });this.write({root,remote:remote.fixturePath,protected:protectedBranches,authority:options.authority??join(dirname(root),'authority'),helper:options.helper??resolve('target/debug/crewroom-process')});
 }
 private write(value:unknown){if(!this.proc.stdin.destroyed)this.proc.stdin.write(JSON.stringify(value)+'\n',()=>{});}
 async call(op:string,input:Record<string,unknown>={},id:string=randomUUID()){await this.ready;return new Promise<any>((resolve,reject)=>{this.pending.set(id,{resolve,reject});this.write({id,op,...input});});}
 async ref(name:string):Promise<string|undefined>{return (await this.call('ref',{name}))??undefined;}
 async push(input:{directory:string;head:string;branch:string;expectedHead:string|null},id?:string){await this.call('push',input,id);}
 async fetchTarget(input:{directory:string;branch:string;destination:string}){await this.call('fetch',input);}
 async cancel(id:string){await this.call('cancel',{target:id});this.pending.get(id)?.reject(new Error('Fixture waiter cancelled'));this.pending.delete(id);}
 async close(){this.write({op:'shutdown'});this.proc.stdin.end();await this.exit;if(this.closedError)throw new Error(this.closedError);}
 async kill(){this.proc.kill('SIGKILL');await this.exit;}
}
