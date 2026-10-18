import assert from 'node:assert/strict';
import { once } from 'node:events';
import { appendFileSync, existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer,request } from 'node:http';
import { createServer as netServer } from 'node:net';
import test from 'node:test';
import { setTimeout as sleep } from 'node:timers/promises';
import { NativeHost, until, backend } from './host-fixture';

test('native host binds separate identities, private model proxy, exact tools and authenticated controls',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin,{}, {coordinator:true});t.after(()=>h.close());
  const identity=await h.ready();assert.equal(identity.hostPid,h.proc.pid);assert.notEqual(identity.ompPid,h.proc.pid);assert.equal(identity.thinkingLevel,'low');assert.equal(identity.model.id,'model');
  const model=JSON.parse(readFileSync(join(h.root,'config/models.yml'),'utf8'));assert.deepEqual(Object.keys(model.providers),['fixture']);assert.equal(model.providers.fixture.apiKey,h.config.providerToken);assert.equal(model.providers.fixture.baseUrl,h.origin+'/provider/v1');
  assert.equal((await h.request({op:'identity'},h.config.providerToken)).status,403);assert.equal((await fetch(h.origin)).status,403);
  assert.equal((await fetch(h.origin,{method:'POST',headers:{Authorization:`Bearer ${h.config.token}`,Origin:'https://foreign.example'},body:'{"op":"identity"}'})).status,403);
  const state=await h.value({op:'state'});assert.equal(state.token,undefined);assert.equal(state.headers,undefined);
  assert.deepEqual(h.requests().find(v=>v.type==='set_host_tools').tools.map((v:any)=>v.name),['send_message','list_roles','propose_team','steering_receipt','playbook_status','propose_playbook','record_playbook_step','propose_learning']);
  await h.input('message',{kind:'tool',name:'send_message',arguments:{to:'Engineer',body:'actual backend'}});assert.equal((await h.terminal('message')).status,'completed');
  assert.equal(b.requests.length,1);assert.equal(b.requests[0].url,'/internal/agent-message');assert.equal(b.requests[0].headers.origin,b.origin);assert.equal(b.requests[0].headers.authorization,`Bearer ${h.config.token}`);assert.equal(b.requests[0].body.attemptId,h.config.attemptId);
  await h.input('denied',{kind:'tool',id:'denied-wire',toolCallId:'denied-call',name:'run_shell',arguments:{command:'false'}});assert.equal((await h.terminal('denied')).status,'error');assert.equal(b.requests.length,1);
  assert.ok(!JSON.stringify(h.events()).includes(h.config.token));
});

test('same-key input shares admission and preserves terminal-before-response across non-consuming observers',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();
  const action={kind:'terminal-first'},first=h.input('same',action);await until(()=>existsSync(join(h.root,'prompt-reply-waiting')));
  const second=h.input('same',action);assert.equal((await h.request({op:'input',id:'same',message:'different'})).status,409);
  assert.equal((await h.value({op:'input_status',id:'same'})).disposition,'completed');writeFileSync(join(h.root,'prompt-reply-release'),'go');
  assert.deepEqual(await Promise.all([first,second]),[{received:true},{received:true}]);
  assert.equal(h.requests().filter(v=>v.type==='prompt').length,1);assert.equal((await h.value({op:'input_status',id:'same'})).disposition,'completed');
  const page=await h.value({op:'events',after:0});assert.deepEqual(await h.value({op:'events',after:0}),page);
  const after=page.at(-1).sequence,waiting=h.value({op:'events',after,wait:25000});await h.input('next',{});const newer=await waiting;assert.ok(newer.every((v:any)=>v.sequence>after));
});

test('busy failures retain keyed content; steer admission and close drain an input whose reply was lost',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();
  writeFileSync(join(h.root,'busy'),'busy');const body={op:'input',id:'busy',message:'{}'};assert.equal((await h.request(body)).status,409);rmSync(join(h.root,'busy'));
  assert.equal((await h.request({...body,message:'changed'})).status,409);assert.equal((await h.request(body)).status,200);
  await h.terminal('busy');writeFileSync(join(h.root,'busy'),'busy');assert.equal((await h.request({op:'input',id:'steer',message:'{}',mode:'steer'})).status,200);await h.terminal('steer');
  assert.equal(h.requests().find(v=>v.type==='prompt'&&v.id==='steer').streamingBehavior,'steer');rmSync(join(h.root,'busy'));
  const controller=new AbortController();const request=fetch(h.origin,{method:'POST',headers:{Authorization:`Bearer ${h.config.token}`},body:JSON.stringify({op:'input',id:'unknown',message:JSON.stringify({kind:'unknown'})}),signal:controller.signal}).catch(()=>null);
  await until(()=>h.requests().some(v=>v.type==='prompt'&&v.id==='unknown'));controller.abort();await request;
  assert.equal((await h.value({op:'input_status',id:'unknown'})).disposition,'accepted');await h.value({op:'close'});await h.exited;
  assert.ok(readFileSync(join(h.root,'inputs.jsonl'),'utf8').includes('"disposition":"accepted"'));
});

test('signal during runner setup settles admission before host stop',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin,{setupGate:'negotiate'});t.after(()=>h.close());
  await until(()=>existsSync(join(h.root,'negotiate-waiting')));h.proc.kill('SIGTERM');writeFileSync(join(h.root,'negotiate-release'),'go');await h.exited;
  assert.equal(h.proc.exitCode,1);assert.ok(!existsSync(join(h.root,'endpoint.json')));assert.equal(h.requests().filter(v=>v.type==='prompt').length,0);
});

test('a corrupt journal read closes admission and stops the owned runner',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();
  appendFileSync(join(h.root,'inputs.jsonl'),'{');
  assert.equal((await h.request({op:'input_status',id:'unknown'})).status,409);
  // Ownership probes can each take five seconds, followed by the HTTP drain.
  try {await until(()=>h.proc.exitCode!==null,15000);} catch(error) {throw new Error(JSON.stringify({root:h.root,events:h.events(),requests:h.requests(),stderr:h.error}),{cause:error});}
  assert.equal(h.proc.exitCode,1);assert.equal(h.requests().filter(v=>v.type==='prompt').length,0);
});

test('close and tool cancellation prevent a delayed preparation reply from launching a command',{timeout:30000},async t=>{
  for(const operation of ['close','cancel']) for(const tool of ['run_check','run_shell']) {
    let release!:()=>void,seen!:()=>void;
    const waiting=new Promise<void>(r=>{seen=r;}),gate=new Promise<void>(r=>{release=r;});
    const server=createServer(async(req,res)=>{for await(const _chunk of req){}seen();await gate;res.setHeader('Content-Type','application/json');res.end(JSON.stringify({spec:{workspace:'/must-not-run',image:'not-admitted',command:'/bin/false',args:[]}}));});
    server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>server.close());
    const h=new NativeHost(`http://127.0.0.1:${(server.address() as any).port}`,{}, {coding:true,commandEngine:{socket:'/deliberately-unused.sock'}});t.after(()=>h.close());await h.ready();
    await h.input('preparing',{kind:'tool',name:tool,arguments:tool==='run_check'?{checkId:'fixture'}:{command:'false'},...(operation==='cancel'?{cancelGate:'cancel-prepare'}:{})});await waiting;
    if(operation==='close'){await h.value({op:'close'});await h.exited;}
    else {await until(()=>existsSync(join(h.root,'cancel-prepare-waiting')));writeFileSync(join(h.root,'cancel-prepare-release'),'cancel');assert.equal((await h.terminal('preparing')).status,'error');}
    release();await sleep(100);assert.ok(!existsSync(join(h.root,'tools')),'Late preparation must not publish a command intent');
    if(operation==='cancel')await h.input('after-cancel',{});
  }
});

test('restore validates retained identity and a rejected history is rebound to the new attempt directory',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const previous=realpathSync(mkdtempSync(join(tmpdir(),'crewroom-previous-session-')));t.after(()=>rmSync(previous,{recursive:true,force:true}));writeFileSync(join(previous,'saved-session'),'retained');
  for(const expected of ['saved-session','wrong-session']) {
    const h=new NativeHost(b.origin,{}, {previousAttemptId:'previous',previousSessionId:expected,resumeSessionDir:previous});t.after(()=>h.close());const identity=await h.ready();
    assert.equal(identity.recoveryMode,expected==='saved-session'?'session':'replacement');assert.equal(identity.sessionDirectory,expected==='saved-session'?previous:join(h.root,'sessions'));
    assert.equal(identity.sessionId,expected==='saved-session'?'saved-session':'fresh-session');await h.input('after-restore',{});await h.terminal('after-restore');
  }
});


test('startup stays unavailable until negotiation and provider credentials cannot use controls',{timeout:30000},async t=>{
  const lease=netServer();lease.listen(0,'127.0.0.1');await once(lease,'listening');const port=(lease.address() as any).port;await new Promise<void>(r=>lease.close(()=>r()));
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin,{setupGate:'negotiate'},{port});t.after(()=>h.close());h.origin=`http://127.0.0.1:${port}`;
  await until(()=>existsSync(join(h.root,'negotiate-waiting')));assert.equal((await h.request({op:'identity'})).status,503);assert.ok(!existsSync(join(h.root,'endpoint.json')));
  writeFileSync(join(h.root,'negotiate-release'),'go');await h.ready();
  const denied=await fetch(h.origin+'/provider/v1/responses',{method:'POST',headers:{Authorization:`Bearer ${h.config.token}`},body:'{}'});assert.equal(denied.status,403);assert.equal(b.requests.length,0);
});

test('large complete events and slow observers do not block input or shutdown when pane output is unread',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();h.proc.stdout.pause();
  const payload='αβ🦀'.repeat(150000);writeFileSync(join(h.root,'fixture-events.json'),JSON.stringify([
    ...Array.from({length:200},()=>({type:'message_update',assistantMessageEvent:{type:'text_delta',delta:'x'.repeat(8192)}})),
    {type:'fixture_complete',payload,token:'fixture-hidden'}
  ]));
  await h.input('large',{fixtureEvents:true});await h.terminal('large');
  const page=await h.value({op:'events',after:0});assert.equal(page.find((v:any)=>v.event.type==='fixture_complete').event.payload,payload);assert.ok(!JSON.stringify(page).includes('fixture-hidden'));
  assert.deepEqual(await h.value({op:'events',after:0}),page);const after=page.at(-1).sequence;
  const pending=Array.from({length:40},()=>h.request({op:'events',after,wait:25000}));await sleep(100);
  assert.equal((await h.value({op:'identity'})).hostPid,h.proc.pid);await h.value({op:'close'});
  const replies=await Promise.all(pending);assert.ok(replies.some(v=>v.status===409));assert.ok(replies.some(v=>v.status===200));await h.exited;assert.equal(h.proc.exitCode,0);h.proc.stdout.resume();
});

test('control decodes split UTF-8 and rejects malformed or oversized bodies before prompt',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();
  async function raw(chunks:Buffer[]) {return new Promise<number>((resolve,reject)=>{const req=request(h.origin,{method:'POST',headers:{Authorization:`Bearer ${h.config.token}`}},res=>{res.resume();res.once('end',()=>resolve(res.statusCode!));});req.on('error',reject);for(const chunk of chunks)req.write(chunk);req.end();});}
  const bytes=Buffer.from(JSON.stringify({op:'input',id:'utf8',message:'🦀αβ'}));const split=bytes.indexOf(Buffer.from('🦀'))+1;assert.equal(await raw([bytes.subarray(0,split),bytes.subarray(split)]),200);await h.terminal('utf8');
  assert.equal(await raw([Buffer.from([0xff])]),409);assert.equal(await raw([Buffer.from(JSON.stringify({op:'input',id:'too-big',message:'a'.repeat(150001)}))]),409);assert.equal(h.requests().filter(v=>v.type==='prompt').length,1);
  writeFileSync(join(h.root,'malformed-state'),'bad');assert.equal((await h.request({op:'input',id:'bad-state',message:'{}'})).status,409);assert.equal(h.requests().filter(v=>v.type==='prompt').length,1);
});

test('journal append failures stop admission and preserve an uncertain partial write',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());
  for(const journal of ['inputs','events']) {
    const h=new NativeHost(b.origin,{hostFileLimit:65536});t.after(()=>h.close());await h.ready();
    if(journal==='events')writeFileSync(join(h.root,'fixture-events.json'),JSON.stringify([{type:'fixture_complete',payload:'x'.repeat(100000)}]));
    try {await h.request({op:'input',id:'write-failure',message:journal==='inputs'?'x'.repeat(100000):JSON.stringify({fixtureEvents:true})});}catch{}
    await until(()=>h.proc.exitCode!==null,10000);assert.equal(h.proc.exitCode,1);const bytes=readFileSync(join(h.root,journal+'.jsonl'));assert.equal(bytes.length,65536);assert.notEqual(bytes.at(-1),10);
    if(journal==='inputs')assert.equal(h.requests().filter(v=>v.type==='prompt').length,0);
  }
});

test('a disconnected close requester and an incomplete body cannot hold stopped execution open',{timeout:15000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();
  const partial=request(h.origin,{method:'POST',headers:{Authorization:`Bearer ${h.config.token}`,'Content-Length':'100'}});partial.on('error',()=>{});partial.write('{');t.after(()=>partial.destroy());
  const close=request(h.origin,{method:'POST',headers:{Authorization:`Bearer ${h.config.token}`}},res=>res.resume());close.on('error',()=>{});close.end(JSON.stringify({op:'close'}));await until(()=>h.requests().some(v=>v.type==='abort'));close.destroy();
  await until(()=>h.proc.exitCode!==null,5000);assert.equal(h.proc.exitCode,0);
});

test('failed negotiation and unexpected runner exit leave no executing child',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());
  const failed=new NativeHost(b.origin,{protocolVersion:1});t.after(()=>failed.close());await failed.exited;assert.equal(failed.proc.exitCode,1);assert.ok(!existsSync(join(failed.root,'endpoint.json')));
  const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();await h.input('exit',{kind:'exit'}).catch(()=>{});await h.exited;assert.equal(h.proc.exitCode,1);
});

test('duplicate tool frames cannot produce duplicate or contradictory backend effects',{timeout:30000},async t=>{
  let release!:()=>void,entered!:()=>void;const gate=new Promise<void>(r=>{release=r;}),seen=new Promise<void>(r=>{entered=r;});let calls=0;
  const server=createServer(async(req,res)=>{for await(const _ of req){}calls++;entered();await gate;res.end('{"stored":true}');});server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>{release();server.close();});
  const h=new NativeHost(`http://127.0.0.1:${(server.address() as any).port}`);t.after(()=>h.close());await h.ready();
  await h.input('duplicates',{kind:'tool',name:'send_message',arguments:{to:'Coordinator',body:'one'},duplicates:3});await seen;await sleep(50);assert.equal(calls,1);release();assert.equal((await h.terminal('duplicates')).status,'completed');
  await h.input('reused-call',{kind:'tool',id:'another-wire',name:'send_message',arguments:{to:'Coordinator',body:'two'}});assert.equal((await h.terminal('reused-call')).status,'error');assert.equal(calls,1);
  await h.input('reused-wire',{kind:'tool',name:'send_message',arguments:{to:'Coordinator',body:'three'}}).catch(()=>{});await h.exited;assert.equal(h.proc.exitCode,1);assert.equal(calls,1);
});

test('backend results and split pane text cannot expose configured credentials',{timeout:30000},async t=>{
  const server=createServer(async(req,res)=>{for await(const _ of req){}res.end(JSON.stringify({stored:true,apiKey:'secret-value',note:'fixture-hidden'}));});server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>server.close());
  const h=new NativeHost(`http://127.0.0.1:${(server.address() as any).port}`);t.after(()=>h.close());await h.ready();await h.input('secret-result',{kind:'tool',name:'send_message',arguments:{to:'Coordinator',body:'fixture'}});await h.terminal('secret-result');
  const result=h.requests().find(v=>v.type==='host_tool_result');assert.ok(!JSON.stringify(result).includes('fixture-hidden'));assert.ok(!JSON.stringify(result).includes('secret-value'));
  await h.input('split-secret',{events:['fixture-','hidden','x'.repeat(300)].map(delta=>({type:'message_update',assistantMessageEvent:{type:'text_delta',delta}}))});await h.terminal('split-secret');await until(()=>h.output.includes('[redacted]'));assert.ok(!h.output.includes('fixture-hidden'));
});

test('a conflicting active tool identity closes the host without a second backend effect',{timeout:30000},async t=>{
  const b=await backend();t.after(()=>b.server.close());const h=new NativeHost(b.origin);t.after(()=>h.close());await h.ready();
  await h.input('conflict',{kind:'tool',name:'send_message',arguments:{to:'Coordinator',body:'one'},conflict:true}).catch(()=>{});await h.exited;assert.equal(h.proc.exitCode,1);assert.ok(b.requests.length<=1);
  assert.ok(h.requests().filter(v=>v.type==='host_tool_result').length<=1);
});
