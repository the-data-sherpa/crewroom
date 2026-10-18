/** Actual isolated Mail with native durable records; no agent or remote provider is started. */
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {NativeMail} from './mail-adapter';
import {seedMessages} from './messaging-fixture';
const config=JSON.parse(readFileSync(process.argv[2],'utf8'));
const seed=seedMessages(join(config.root,'work'));const options={...config,database:seed.database,now:1000};
const clients:NativeMail[]=[];const create=(extra:Record<string,unknown>={})=>{const c=new NativeMail({...options,...extra});clients.push(c);return c;};let c=create();
const workers:any[]=[];const run=seed.run.id,sender=seed.sender.attempt.id,recipient=seed.recipient.attempt.id;
const send=(attempt:string,key:string,input:any)=>c.command('send',{target:attempt,key,input});
const message=(id:string)=>c.command('message',{target:id});
const control=async(path:string)=>{const response=await fetch(config.controlUrl+path,{method:'POST',signal:AbortSignal.timeout(30000)});assert.ok(response.ok);};
try {
  await c.command('register',{target:run});const first=await send(sender,'call-1',{to:'Specialist',body:'Check zero.'});assert.ok(first.transportId);
  assert.equal((await send(sender,'call-1',{to:'Specialist',body:'Check zero.'})).transportId,first.transportId);
  const receipt=()=>c.call('get_message_delivery_receipt',{project_key:config.project,message_id:first.transportId});const before=await receipt();
  const events=await c.command('observe',{target:run});assert.ok(events.some((v:any)=>v.message_id===first.transportId));assert.deepEqual(await receipt(),before);
  await c.close();c=create();assert.equal((await send(sender,'call-1',{to:'Specialist',body:'Check zero.'})).transportId,first.transportId);assert.deepEqual(await c.command('observe',{target:run}),[]);
  await c.command('accept',{target:first.id});assert.equal((await receipt()).recipients[0].acknowledged,true);
  const reply=await send(recipient,'call-2',{to:'Coordinator',body:'Zero confirmed.',replyTo:first.id});assert.equal(reply.replyTo,first.id);
  const inbox=async(name:string)=>c.call('fetch_inbox',{project_key:config.project,agent_name:name,include_bodies:true,mark_read:false,limit:100});assert.equal((await inbox('GreenCastle')).filter((v:any)=>v.subject===first.id).length,1);
  const workerConfig=join(config.root,'worker.json');writeFileSync(workerConfig,JSON.stringify(options),{mode:0o600});
  for(const window of ['before-persistence','after-persistence','before-ack','after-ack','after-processing']){
    const sent=await send(sender,`window-${window}`,{to:'Specialist',body:`Crash at ${window}`});await c.close();
    const child=spawn(process.execPath,['--import','tsx','scripts/rust/mail-window-worker.ts',workerConfig,sent.id,recipient,window],{stdio:'pipe'});let output='',error='';child.stdout.on('data',b=>{output+=b;});child.stderr.on('data',b=>{error+=b;});const [code]=await once(child,'exit');assert.equal(code,0,output+error);workers.push(JSON.parse(output));
    c=create();const saved=await message(sent.id);assert.equal(saved.disposition,window==='before-persistence'?'stored':window==='after-processing'?'received':'accepted');
    const external=await c.call('get_message_delivery_receipt',{project_key:config.project,message_id:sent.transportId});assert.equal(external.recipients[0].acknowledged,['after-ack','after-processing'].includes(window));
    await c.command('accept',{target:sent.id});await c.command('dispatch',{target:sent.id});const input={to:'Coordinator',body:`Effect for ${sent.id}`,replyTo:sent.id};const effect=await send(recipient,`effect-${sent.id}`,input);assert.equal((await send(recipient,`effect-${sent.id}`,input)).id,effect.id);
    assert.equal((await inbox('BlueLake')).filter((v:any)=>v.subject===effect.id).length,1);assert.equal((await c.command('runtime',{target:run})).messages.filter((v:any)=>v.toolCallId===`effect-${sent.id}`).length,1);
  }
  await assert.rejects(send(sender,'lost-call',{to:'Specialist',body:'LOSE_REPLY'}));const lost=(await c.command('runtime',{target:run})).messages.find((v:any)=>v.toolCallId==='lost-call');assert.equal(lost.transportId,undefined);
  await c.close();c=create({retentionSeconds:1,now:6000});await c.command('ownership',{target:sender});assert.ok((await c.command('reconcile',{target:lost.id})).transportId);
  const absent=await c.command('prepare',{target:recipient,key:'absent-call',input:{to:'Coordinator',body:'Never sent'}});await c.command('clock',{now:11000});await assert.rejects(c.command('reconcile',{target:absent.id}),/uncertain.*Do not resend/);
  const after=await inbox('GreenCastle');assert.equal(after.filter((v:any)=>v.body_md==='LOSE_REPLY').length,1);assert.equal(after.filter((v:any)=>v.body_md==='Never sent').length,0);
  await c.close();c=create({now:11000});await control('/offline');await assert.rejects(send(recipient,'outage-call',{to:'Coordinator',body:'Retain through downtime'}),/503/);
  const pending=(await c.command('runtime',{target:run})).messages.find((v:any)=>v.toolCallId==='outage-call');assert.equal(pending.disposition,'pending');assert.equal(pending.transportId,undefined);
  await control('/online');const recovered=await c.command('reconcile',{target:pending.id});assert.ok(recovered.transportId);await control('/restart');await c.close();c=create({now:11000});
  assert.equal((await send(recipient,'outage-call',{to:'Coordinator',body:'Retain through downtime'})).transportId,recovered.transportId);await c.command('accept',{target:recovered.id});assert.equal((await inbox('BlueLake')).filter((v:any)=>v.subject===recovered.id).length,1);
  await c.close();
  const report={passed:true,runId:run,workers,processes:clients.map(v=>({pid:v.proc.pid,exitCode:v.proc.exitCode,signal:v.proc.signalCode})),allOwnedProcessesExited:clients.every(v=>v.proc.exitCode!==null||v.proc.signalCode!==null),checks:['five killed native SQLite receive windows','one durable/external reply effect per replayed receive','keyed send and restart replay','passive durable cursor preserves external unread and ack state','durable input before acknowledgement','causal reply','lost-response reconciliation after local retry window expiry','uncertain absent send blocked','service downtime retains pending intent','two service restarts retain one logical effect']};
  writeFileSync(join(config.root,'native-report.json'),JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}finally{for(const client of clients)await client.close();}
