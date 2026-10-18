import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {rmSync} from 'node:fs';
import {DatabaseSync} from 'node:sqlite';
import test from 'node:test';
import {NativeMail} from './mail-adapter';
import {seedMessages} from './messaging-fixture';
async function fixture(handler:(name:string,args:any)=>any|Promise<any>) {
  const requests:any[]=[];const server=createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);res.setHeader('Content-Type','application/json');
    if(body.method!=='tools/call'){res.end(body.id?JSON.stringify({id:body.id,result:{}}):'');return;}
    const {name,arguments:args}=body.params;requests.push({name,args});
    try {const value=await handler(name,args);if(value==='drop'){res.destroy();return;}if(value==='503'){res.writeHead(503);res.end('{}');return;}
      const raw=JSON.stringify({id:body.id,result:{structuredContent:value}}).replace(/"(count|id|cursor|next_cursor|message_id)":(\d+)([,}])/g,'"$1":$2.0$3');res.end(raw);
    } catch(error) {res.writeHead(500);res.end(JSON.stringify({error:String(error)}));}
  });server.listen(0,'127.0.0.1');await once(server,'listening');return {requests,url:`http://127.0.0.1:${(server.address() as any).port}/mcp/`,async close(){server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));}};
}
const receipt=(id=7)=>({count:1,deliveries:[{payload:{id}}]});
function rows(database:string,table:string) {const db=new DatabaseSync(database,{readOnly:true});try{return db.prepare(`SELECT data FROM ${table}`).all().map(v=>JSON.parse(String(v.data)));}finally{db.close();}}

test('native messaging binds before registration, persists send/accept before external calls and preserves receipts through reopen',{timeout:30000},async t=>{
  const seed=seedMessages();let rejectAck=true;let sends=0;let acks=0;const f=await fixture((name,args)=>{
    if(name==='register_agent'){assert.ok(rows(seed.database,'assignments').some(v=>v.mailboxName===args.name));return {name:args.name};}
    if(name==='send_message'){sends++;const message=rows(seed.database,'peer_messages').find(v=>v.id===args.subject);assert.equal(message.disposition,'pending');assert.equal(message.body,args.body_md);assert.equal(args.idempotency_key,message.id);return receipt();}
    if(name==='acknowledge_message'){acks++;assert.equal(rows(seed.database,'peer_messages')[0].disposition,'accepted');return rejectAck?'503':{acknowledged:true};}
    return {};
  });let c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});
  await c.command('register',{target:seed.run.id});assert.deepEqual(f.requests.filter(v=>v.name==='register_agent').map(v=>v.args.name),['BlueLake','GreenCastle']);
  assert.ok(f.requests.filter(v=>v.name==='register_agent').every(v=>v.args.model==='fixture/model'));assert.ok(f.requests.filter(v=>v.name==='set_contact_policy').every(v=>v.args.policy==='open'));
  const args={target:seed.sender.attempt.id,key:'key',input:{to:'Specialist',body:'Message body'}};const first=await c.command('send',args);assert.equal(first.transportId,7);assert.equal(first.disposition,'stored');
  assert.deepEqual(await c.command('send',args),first);assert.equal(sends,1);await c.close();
  c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});assert.deepEqual(await c.command('send',args),first);assert.equal(sends,1);
  await assert.rejects(c.command('accept',{target:first.id}),/503/);assert.equal((await c.command('message',{target:first.id})).disposition,'accepted');rejectAck=false;await c.command('accept',{target:first.id});assert.equal(acks,2);
  await c.command('dispatch',{target:first.id});assert.equal((await c.command('message',{target:first.id})).disposition,'received');
  await assert.rejects(c.command('send',{...args,input:{to:'Specialist',body:'Changed'}}),/different content/);assert.equal(sends,1);
});

test('native expired-key reconciliation matches exact body/sender/subject once and never resends absent or ambiguous outcomes',{timeout:30000},async t=>{
  const seed=seedMessages();const inbox:any[]=[];let sends=0;const f=await fixture((name,args)=>{
    if(name==='register_agent')return {name:args.name};
    if(name==='send_message'){sends++;inbox.push({id:7,subject:args.subject,from:args.sender_name,body_md:args.body_md});return 'drop';}
    if(name==='fetch_inbox'){assert.equal(args.mark_read,false);assert.equal(args.include_bodies,true);return inbox;}
    return {};
  });const c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000,retentionSeconds:1});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await c.command('register',{target:seed.run.id});
  await assert.rejects(c.command('send',{target:seed.sender.attempt.id,key:'lost',input:{to:'Specialist',body:'One logical send'}}));let saved=(await c.command('runtime',{target:seed.run.id})).messages[0];assert.equal(saved.transportId,undefined);assert.equal(sends,1);
  await c.command('clock',{now:6000});saved=await c.command('reconcile',{target:saved.id});assert.equal(saved.transportId,7);assert.equal(sends,1);
  for(const ambiguous of [false,true]) {
    const message=await c.command('prepare',{target:seed.sender.attempt.id,key:ambiguous?'ambiguous':'absent',input:{to:'Specialist',body:'Uncertain'}});
    if(ambiguous)inbox.push(...[21,22].map(id=>({id,subject:message.id,from:'BlueLake',body_md:'Uncertain'})));
    await c.command('clock',{now:ambiguous?16000:11000});await assert.rejects(c.command('reconcile',{target:message.id}),/uncertain.*Do not resend/);assert.equal((await c.command('message',{target:message.id})).disposition,'uncertain');assert.equal(sends,1);
  }
});

test('native mailbox observation is throttled, non-consuming and atomic across malformed pages and restart',{timeout:30000},async t=>{
  const seed=seedMessages();let malformed=false;const f=await fixture((name,args)=>{
    if(name==='register_agent')return {name:args.name};
    if(name==='fetch_inbox_events'){
      assert.equal(args.mark_read,undefined);assert.equal(args.limit,100);
      if(malformed)return {events:[{cursor:3,message_id:7}],next_cursor:args.after,has_more:true};
      return args.after===0?{events:[{cursor:1,message_id:7,kind:'sent'}],next_cursor:1,has_more:true}:args.after===1?{events:[{cursor:2,message_id:7,kind:'read'}],next_cursor:2,has_more:false}:{events:[],next_cursor:2,has_more:false};
    }
    return {};
  });let c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await c.command('register',{target:seed.run.id});
  const first=await c.command('observe',{target:seed.run.id});assert.equal(first.length,4);const count=f.requests.length;
  for(const now of [1000,1100,2000,2499]){await c.command('clock',{now});assert.deepEqual(await c.command('observe',{target:seed.run.id}),[]);}assert.equal(f.requests.length,count);
  await c.command('clock',{now:2500});assert.deepEqual(await c.command('observe',{target:seed.run.id}),[]);assert.equal(f.requests.length,count+2);
  malformed=true;await c.command('clock',{now:4000});await assert.rejects(c.command('observe',{target:seed.run.id}),/no progress/);await c.close();malformed=false;
  c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:5000});assert.deepEqual(await c.command('observe',{target:seed.run.id}),[]);assert.ok(f.requests.slice(-2).every(v=>v.args.after===2));assert.equal((await c.command('runtime',{target:seed.run.id})).messages.length,0);assert.ok(!f.requests.some(v=>v.name==='acknowledge_message'||v.name==='fetch_inbox'));
});

test('native message transport refuses mismatched registrations and malformed storage receipts without advancing durable delivery',{timeout:30000},async t=>{
  const seed=seedMessages();let wrong=true;let value:any=receipt();let sends=0;const f=await fixture((name,args)=>{
    if(name==='register_agent')return {name:wrong?'WrongName':args.name};if(name==='send_message'){sends++;return value;}return {};
  });const c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});
  await assert.rejects(c.command('register',{target:seed.run.id}),/identity mismatch/);assert.equal(rows(seed.database,'assignments')[0].mailboxName,'BlueLake');assert.ok(!f.requests.some(v=>v.name==='set_contact_policy'));wrong=false;await c.command('register',{target:seed.run.id});
  const args={target:seed.sender.attempt.id,key:'bad-receipt',input:{to:'Specialist',body:'Pending'}};
  for(value of [{count:2,deliveries:[{payload:{id:7}}]},{count:1,deliveries:[]},receipt(0),receipt(0.5),{count:1,deliveries:[{payload:{id:'7'}}]}]){
    await assert.rejects(c.command('send',args),/Invalid Mail storage receipt/);const message=(await c.command('runtime',{target:seed.run.id})).messages[0];assert.equal(message.disposition,'pending');assert.equal(message.transportId,undefined);
  }
  value=receipt();assert.equal((await c.command('send',args)).transportId,7);assert.equal(sends,6);
});

test('a stale expired lookup cannot replace an in-flight send receipt; the store remains responsive during Mail IO',{timeout:30000},async t=>{
  const seed=seedMessages();let releaseSend!:(v:any)=>void,releaseLookup!:(v:any)=>void;
  let sawSend!:()=>void,sawLookup!:()=>void;
  const sending=new Promise<void>(r=>{sawSend=r;}),looking=new Promise<void>(r=>{sawLookup=r;});
  const f=await fixture((name,args)=>{
    if(name==='register_agent')return {name:args.name};
    if(name==='send_message'){sawSend();return new Promise(r=>{releaseSend=r;});}
    if(name==='fetch_inbox'){sawLookup();return new Promise(r=>{releaseLookup=r;});}return {};
  });const c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000,retentionSeconds:1});t.after(async()=>{releaseSend?.(receipt());releaseLookup?.([]);await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await c.command('register',{target:seed.run.id});
  const send=c.command('send',{target:seed.sender.attempt.id,key:'overlap',input:{to:'Specialist',body:'Concurrent'}});await sending;
  const pending=(await c.command('runtime',{target:seed.run.id})).messages[0];assert.equal(pending.disposition,'pending');await c.command('clock',{now:6000});
  const lookup=c.command('reconcile',{target:pending.id});await looking;releaseSend(receipt());assert.equal((await send).transportId,7);releaseLookup([]);
  assert.equal((await lookup).transportId,7);assert.equal((await c.command('message',{target:pending.id})).disposition,'stored');
});

test('mailbox polling bounds backlog, empty pages and payloads while visiting other mailboxes',{timeout:30000},async t=>{
  for(const mode of ['backlog','empty','bytes']){
    const seed=seedMessages();const payload='x'.repeat(3*1024*1024);const f=await fixture((name,args)=>{
      if(name==='register_agent')return {name:args.name};
      if(name==='fetch_inbox_events'){
        const count=mode==='backlog'?100:mode==='empty'?0:1;const next=args.after+Math.max(1,count);
        return {events:Array.from({length:count},(_,i)=>({cursor:args.after+i+1,message_id:7,...(mode==='bytes'?{body:payload}:{})})),next_cursor:next,has_more:true};
      }return {};
    });const c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await c.command('register',{target:seed.run.id});
    const first=await c.command('observe',{target:seed.run.id});const requests=f.requests.filter(v=>v.name==='fetch_inbox_events');assert.equal(first.length,mode==='backlog'?1000:mode==='empty'?0:3);assert.equal(requests.length,mode==='backlog'?10:mode==='empty'?100:3);assert.ok(requests.every((v,i)=>v.args.agent_name===(i%2?'GreenCastle':'BlueLake')));
    await c.command('clock',{now:2500});await c.command('observe',{target:seed.run.id});const resumed=f.requests.filter(v=>v.name==='fetch_inbox_events')[requests.length];assert.equal(resumed.args.agent_name,mode==='bytes'?'GreenCastle':'BlueLake');assert.ok(resumed.args.after>0);
  }
});

test('observation has one total deadline, excludes concurrent walks and retains completed pages',{timeout:30000},async t=>{
  const seed=seedMessages();let seen!:()=>void;const first=new Promise<void>(r=>{seen=r;});
  const f=await fixture(async(name,args)=>{if(name==='register_agent')return {name:args.name};if(name==='fetch_inbox_events'){seen();await new Promise(r=>setTimeout(r,7000));return {events:[{cursor:args.after+1,message_id:7}],next_cursor:args.after+1,has_more:true};}return {};});
  const c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await c.command('register',{target:seed.run.id});
  const started=Date.now();const pending=c.command('observe',{target:seed.run.id});const rejected=assert.rejects(pending,/budget exhausted/);await first;await c.command('clock',{now:3000});assert.deepEqual(await c.command('observe',{target:seed.run.id}),[]);assert.equal((await c.command('runtime',{target:seed.run.id})).messages.length,0);await rejected;assert.ok(Date.now()-started<14000);assert.equal(f.requests.filter(v=>v.name==='fetch_inbox_events').length,2);
});

test('registration keeps retired indices, uses current attempt fallback and replays a lost name binding',{timeout:30000},async t=>{
  const seed=seedMessages();const db=new DatabaseSync(seed.database);
  const retired={...seed.sender.assignment,retiredAt:'1970-01-01T00:00:01.000Z',mailboxName:'BlueLake'};
  db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify(retired),retired.id);
  const attempt={...seed.recipient.attempt,model:{...seed.recipient.attempt.model,resolved:{...seed.recipient.attempt.model?.resolved,selector:'fixture/fallback'}}};
  db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(attempt),attempt.id);db.close();
  let drop=true;const f=await fixture((name,args)=>{if(name==='register_agent'){assert.equal(args.name,'GreenCastle');assert.equal(args.model,'fixture/fallback');if(drop){drop=false;return 'drop';}return {name:args.name};}if(name==='fetch_inbox_events')return {events:[],next_cursor:0,has_more:false};return {};});
  let c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});
  await assert.rejects(c.command('register',{target:seed.run.id}));assert.equal(rows(seed.database,'assignments')[1].mailboxName,'GreenCastle');await c.close();c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});await c.command('register',{target:seed.run.id});
  assert.equal(f.requests.filter(v=>v.name==='register_agent').length,2);await c.command('observe',{target:seed.run.id});assert.deepEqual(f.requests.filter(v=>v.name==='fetch_inbox_events').map(v=>v.args.agent_name),['BlueLake','GreenCastle']);
});

test('observer pages canonicalize whole numbers and roll back invalid or failed transactions',{timeout:30000},async t=>{
  const seed=seedMessages();let mode='fraction';let after:number[]=[];
  const f=await fixture((name,args)=>{if(name==='register_agent')return {name:args.name};if(name==='fetch_inbox_events'){after.push(args.after);return {events:[{cursor:mode==='fraction'?1.5:mode==='ahead'?2:1,message_id:7}],next_cursor:1,has_more:false};}return {};});
  const db=new DatabaseSync(seed.database);db.exec("CREATE TRIGGER reject_mail_cursor BEFORE INSERT ON observer_cursors BEGIN SELECT RAISE(ABORT, 'fixture persistence failure'); END;");db.close();
  let c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await c.command('register',{target:seed.run.id});
  for(const [i,next] of ['fraction','ahead','database'].entries()){mode=next;await c.command('clock',{now:1000+i*1500});await assert.rejects(c.command('observe',{target:seed.run.id}));}
  assert.deepEqual(after,[0,0,0]);await c.close();const verify=new DatabaseSync(seed.database);assert.equal(verify.prepare('SELECT count(*) AS n FROM observations').get()!.n,0);verify.exec('DROP TRIGGER reject_mail_cursor');verify.close();
  c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:10000});assert.equal((await c.command('observe',{target:seed.run.id})).length,2);
});

test('one hundred historical identities cannot be recycled for another assignment',{timeout:30000},async t=>{
  const seed=seedMessages();const db=new DatabaseSync(seed.database);
  // Inject a synthetic retirement history; assignment and attempt identities
  // remain distinct and linked. No executing process or provider is represented.
  for(const entry of [seed.sender,seed.recipient]) db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify({...entry.assignment,retiredAt:'1970-01-01T00:00:01.000Z'}),entry.assignment.id);
  for(let i=2;i<=100;i++){
    const id=`historical-${i}`,attemptId=`historical-attempt-${i}`,name=`Historical ${i}`;
    const assignment={...seed.recipient.assignment,id,name,currentAttemptId:attemptId,...(i<100?{retiredAt:'1970-01-01T00:00:01.000Z'}:{})};
    const attempt={...seed.recipient.attempt,id:attemptId,assignmentId:id};
    db.prepare('INSERT INTO assignments(id,run_id,name,data) VALUES(?,?,?,?)').run(id,seed.run.id,name,JSON.stringify(assignment));db.prepare('INSERT INTO attempts(id,assignment_id,data) VALUES(?,?,?)').run(attemptId,id,JSON.stringify(attempt));
  }db.close();
  const f=await fixture(()=>({}));const c=new NativeMail({url:f.url,token:'fixture',database:seed.database,project:seed.root,now:1000});t.after(async()=>{await c.close();await f.close();rmSync(seed.root,{recursive:true,force:true});});await assert.rejects(c.command('register',{target:seed.run.id}),/exhausted the 100/);assert.deepEqual(f.requests.map(v=>v.name),['ensure_project']);assert.equal(rows(seed.database,'assignments').filter(v=>v.mailboxName).length,0);
});
