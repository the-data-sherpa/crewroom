// Test-only transport: every provider decision and upstream request uses the Rust gateway.
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { createInterface } from 'node:readline';
import { once } from 'node:events';
import { after } from 'node:test';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
import type { AssignmentModel } from '../../server/runtime/model-policy';
const instances:ProviderGateway[]=[];
after(async()=>{for(const gateway of instances)await gateway.closed();});
export class ProviderGateway {
  failure:any;
  private child:ChildProcessWithoutNullStreams;
  private ready:Promise<string>;
  private serial=0;
  private pending=new Map<number,{resolve:(v:any)=>void,reject:(e:Error)=>void}>();
  private previous:string;
  private origin='';
  private closing?:Promise<void>;
  private errors='';
  constructor(private root:string,private plan:AssignmentModel,private provider:any,private token:string,private observe:(event:any)=>void,private budgetRoot=root) {
    assert.equal(token,'host-token','Fixture token is explicit.');
    this.child=spawn(resolve('target/debug/examples/gateway-fixture'),[],{stdio:['pipe','pipe','pipe']});instances.push(this);
    this.child.stderr.on('data',chunk=>{this.errors+=chunk.toString();});
    this.previous=JSON.stringify(provider);
    this.ready=new Promise((resolve,reject)=>{
      createInterface({input:this.child.stdout}).on('line',line=>{
        const value=JSON.parse(line);
        if(value.ready){this.origin=value.origin;resolve(value.origin);}
        else if(value.event){if(value.event.type==='provider_failure'){const {type,...failure}=value.event;this.failure=failure;}if(value.event.type==='provider_request')this.failure=undefined;this.observe(value.event);}
        else if(value.id){this.pending.get(value.id)?.resolve(value.result);this.pending.delete(value.id);}
      });
      this.child.once('error',reject);
      this.child.once('exit',code=>{if(!this.origin)reject(new Error(`Native gateway exited ${code}: ${this.errors}`));for(const waiter of this.pending.values())waiter.reject(new Error(`Native gateway exited ${code}: ${this.errors}`));this.pending.clear();});
    });
    this.child.stdin.write(JSON.stringify(this.config())+'\n');
  }
  private config(){return {root:this.root,plan:this.plan,provider:this.provider,budgetRoot:this.budgetRoot};}
  private async command(op:string,extra:object={}) {
    await this.ready;const id=++this.serial;
    const value=new Promise<any>((resolve,reject)=>{this.pending.set(id,{resolve,reject});});
    this.child.stdin.write(JSON.stringify({op,id,...extra})+'\n');return value;
  }
  async serve(req:IncomingMessage,res:ServerResponse) {
    try {
      const origin=await this.ready;
      const provider=JSON.stringify(this.provider);
      if(provider!==this.previous){await this.command('replace',{config:this.config()});this.previous=provider;}
      const bytes=[];for await(const chunk of req)bytes.push(chunk);
      const controller=new AbortController();res.once('close',()=>controller.abort());
      const response=await fetch(origin+req.url,{method:req.method,headers:req.headers as Record<string,string>,body:req.method==='GET'?undefined:Buffer.concat(bytes),signal:controller.signal});
      res.writeHead(response.status,{'Content-Type':response.headers.get('content-type')??'application/json'});
      if(response.body)for await(const chunk of response.body)res.write(chunk);
      this.failure=(await this.command('state')).failure??undefined;
      res.end();
    }catch(error){if(!res.headersSent){res.writeHead(502);res.end('{}');}else res.destroy(error as Error);}
  }
  close(){this.closing??=(async()=>{await this.command('close');this.child.stdin.end();if(this.child.exitCode===null)await once(this.child,'exit');assert.equal(this.child.exitCode,0,this.errors);})();}
  async closed(){this.close();await this.closing;}
}
