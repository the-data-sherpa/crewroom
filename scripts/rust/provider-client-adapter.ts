/** Native JSONL fixture; only synthetic test credentials cross this pipe. */
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {randomUUID} from 'node:crypto';
import type {JiraClient,JiraConnection} from '../../server/jira-client';
import type {BitbucketClient,BitbucketConnection} from '../../server/bitbucket-client';
type Before=()=>void|Promise<void>;
class Client {
 private proc=spawn('target/debug/examples/provider-client-fixture',[],{stdio:'pipe'});
 private exit=once(this.proc,'exit');private stderr='';
 private pending=new Map<string,{resolve:(v:any)=>void,reject:(e:Error)=>void,before?:Before}>();
 readonly ready:Promise<void>;
 constructor(config:Record<string,unknown>,private credentials:()=>Promise<unknown>){
  this.proc.stderr.on('data',chunk=>this.stderr+=chunk);
  this.ready=new Promise((resolve,reject)=>{
   createInterface({input:this.proc.stdout}).on('line',line=>{
    const value=JSON.parse(line);
    if(value.callback){void (async()=>{try{const result=value.kind==='credentials'?await this.credentials():await this.pending.get(value.request)?.before?.();this.write({callback:value.callback,value:result??null});}catch{this.write({callback:value.callback,error:true});}})();return;}
    const error=value.error?Object.assign(new Error(value.error),{kind:value.kind,...(value.kind==='unavailable'?{requestSent:value.requestSent,status:value.status??undefined}:{})}):undefined;
    if(value.ready){if(error)reject(error);else resolve();return;}
    const job=this.pending.get(value.id);if(!job)return;this.pending.delete(value.id);if(error)job.reject(error);else job.resolve(value.value);
   });
   this.proc.once('error',reject);this.proc.once('exit',()=>{const error=new Error(this.stderr||'Native provider fixture exited');reject(error);for(const job of this.pending.values())job.reject(error);this.pending.clear();});
  });
  const {credentials:_,...safe}=config;this.write(safe);
 }
 private write(value:unknown){if(!this.proc.stdin.destroyed)this.proc.stdin.write(JSON.stringify(value)+'\n',()=>{});}
 async call(op:string,input:Record<string,unknown>={},before?:Before){await this.ready;const id=randomUUID();return new Promise<any>((resolve,reject)=>{this.pending.set(id,{resolve,reject,before});this.write({op,input,id,before:!!before});});}
 async close(){this.write({op:'close'});this.proc.stdin.end();await this.exit;}
}
export class NativeJiraClient extends Client {
 constructor(connection:JiraConnection){super({...connection,provider:'jira'},connection.credentials);}
 source(){return this.call('source');}
 search():ReturnType<JiraClient["search"]>{return this.call('search');}issue(id:string):ReturnType<JiraClient["issue"]>{return this.call('issue',{id});}comments(id:string):ReturnType<JiraClient["comments"]>{return this.call('comments',{id});}transitions(id:string):ReturnType<JiraClient["transitions"]>{return this.call('transitions',{id});}
 addComment(id:string,body:unknown,before?:Before):ReturnType<JiraClient["addComment"]>{return this.call('addComment',{id,body},before);}
 async transition(id:string,transitionId:string,before?:Before):ReturnType<JiraClient["transition"]>{await this.call('transition',{id,transitionId},before);}
 async request(path:string,method='GET',body?:unknown,before?:Before){const v=await this.call('request',{path,method,...(body===undefined?{}:{body})},before);return v.present?v.value:undefined;}
}
export class NativeBitbucketClient extends Client {
 constructor(connection:BitbucketConnection){super({...connection,provider:'bitbucket'},connection.credentials);}
 repository(){return this.call('repository');}verifyRepository():ReturnType<BitbucketClient["verifyRepository"]>{return this.call('verifyRepository');}pullRequest(id:number):ReturnType<BitbucketClient["pullRequest"]>{return this.call('pullRequest',{id});}pullRequests(branch:string,target:string):ReturnType<BitbucketClient["pullRequests"]>{return this.call('pullRequests',{branch,target});}comments(id:number):ReturnType<BitbucketClient["comments"]>{return this.call('comments',{id});}observe(id:number):ReturnType<BitbucketClient["observe"]>{return this.call('observe',{id});}
 createPullRequest(input:{title:string,description:string,branch:string,target:string,head:string},before?:Before):ReturnType<BitbucketClient["createPullRequest"]>{return this.call('createPullRequest',input,before);}
 updatePullRequest(id:number,input:{title:string,description:string},before?:Before):ReturnType<BitbucketClient["updatePullRequest"]>{return this.call('updatePullRequest',{id,...input},before);}
}
