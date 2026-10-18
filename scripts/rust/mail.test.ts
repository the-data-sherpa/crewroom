import assert from 'node:assert/strict';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {once} from 'node:events';
import {setTimeout as sleep} from 'node:timers/promises';
import test from 'node:test';
import {NativeMail} from './mail-adapter';
import {RustMailClient} from '../../server/runtime/messaging';
const busy={isError:true,content:[{type:'text',text:JSON.stringify({error:{type:'RESOURCE_BUSY',recoverable:true,data:{db_error_classification:{safe_to_retry:true}}}})}]};
async function fixture(handler:(body:any,req:IncomingMessage,res:ServerResponse)=>unknown,options:{manualInitialize?:boolean}={}) {
  const requests:any[]=[];
  const server=createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);requests.push({body,headers:req.headers});res.setHeader('Content-Type','application/json');
    if(body.method!=='tools/call'&&!options.manualInitialize){res.setHeader('Mcp-Session-Id','fixture-session');res.end(body.id?JSON.stringify({id:body.id,result:{}}):'');return;}
    await handler(body,req,res);
  });server.listen(0,'127.0.0.1');await once(server,'listening');return {requests,url:`http://127.0.0.1:${(server.address() as any).port}/mcp/`,async close(){server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));}};
}
const toolReply=(res:ServerResponse,body:any,value:any)=>res.end(JSON.stringify({id:body.id,result:value}));

test('native and retained Mail clients agree on JSON/SSE text and structured results and preserve private session headers',{timeout:30000},async t=>{
  for(const mode of ['json-structured','json-text','sse']) for(const native of [false,true]) {
    const f=await fixture((body,req,res)=>{assert.equal(req.headers.authorization,'Bearer private-mail-token');assert.equal(req.headers['mcp-session-id'],'fixture-session');
      const value={ok:true,message:'αβ🦀'};
      if(mode==='sse'){res.setHeader('Content-Type','text/event-stream');res.end('event: message\ndata: '+JSON.stringify({id:'unrelated',result:{}})+'\n\ndata: '+JSON.stringify({id:body.id,result:{structuredContent:value}})+'\n\n');}
      else toolReply(res,body,mode==='json-text'?{content:[{type:'image',data:'unused'},{type:'text',text:JSON.stringify(value)}]}:{structuredContent:value});
    });t.after(()=>f.close());
    const client=native?new NativeMail({url:f.url,token:'private-mail-token'}):new RustMailClient(f.url,'private-mail-token');if(native)t.after(()=>(client as NativeMail).close());
    assert.deepEqual(await client.call('fetch_inbox_events',{}),{ok:true,message:'αβ🦀'});assert.deepEqual(f.requests.map(v=>v.body.method),['initialize','notifications/initialized','tools/call']);
    assert.equal(f.requests[0].body.params.protocolVersion,'2024-11-05');assert.equal(f.requests[1].body.id,undefined);
  }
});

test('native shared initialization delivers one failure to concurrent callers and permits a later fresh initialization',{timeout:30000},async t=>{
  let fail=true;
  const f=await fixture(async(body,_req,res)=>{
    if(body.method==='initialize'){await sleep(100);if(fail){res.writeHead(503);res.end('{}');return;}toolReply(res,body,{});}
    else if(body.method==='notifications/initialized')res.end('');else toolReply(res,body,{structuredContent:{ok:true}});
  },{manualInitialize:true});t.after(()=>f.close());const c=new NativeMail({url:f.url,token:'fixture'});t.after(()=>c.close());await c.ready;
  const results=await Promise.allSettled(Array.from({length:12},()=>c.call('fetch_inbox',{})));assert.ok(results.every(v=>v.status==='rejected'));assert.equal(f.requests.length,1);
  fail=false;const recovered=await Promise.all(Array.from({length:12},()=>c.call('fetch_inbox',{})));assert.ok(recovered.every(v=>v.ok));assert.equal(f.requests.filter(v=>v.body.method==='initialize').length,2);assert.equal(f.requests.filter(v=>v.body.method==='notifications/initialized').length,1);
});

test('native reads and keyed rate limits retry; ambiguous writes and unsafe tool errors are never repeated',{timeout:30000},async t=>{
  const scenarios=[
    {name:'fetch_inbox',args:{},statuses:[503,503],expected:3,ok:true},
    {name:'send_message',args:{idempotency_key:'key'},statuses:[429,429],expected:3,ok:true},
    {name:'send_message',args:{idempotency_key:'key'},statuses:[503],expected:1,ok:false},
    {name:'send_message',args:{},statuses:[429],expected:1,ok:false},
    {name:'register_agent',args:{},statuses:[503],expected:1,ok:false},
    {name:'fetch_inbox',args:{},statuses:[401],expected:1,ok:false},
    {name:'send_message',args:{},statuses:[0],expected:1,ok:false},
    {name:'fetch_inbox',args:{},statuses:[0,0],expected:3,ok:true},
  ];
  for(const scenario of scenarios) {
    let count=0;const f=await fixture((body,_req,res)=>{const status=scenario.statuses[count++]??200;if(status===0){res.destroy();return;}if(status!==200){res.writeHead(status,{'Retry-After':'0'});res.end('{}');return;}toolReply(res,body,{structuredContent:{ok:true}});});
    const c=new NativeMail({url:f.url,token:'fixture'});try{if(scenario.ok)assert.deepEqual(await c.call(scenario.name,scenario.args),{ok:true});else await assert.rejects(c.call(scenario.name,scenario.args));assert.equal(count,scenario.expected);}finally{await c.close();await f.close();}
  }
  for(const safe of [true,false]) {
    let count=0;const f=await fixture((body,_req,res)=>{count++;toolReply(res,body,count<3?(safe?busy:{isError:true,content:[{type:'text',text:'uncertain private-mail-token'}]}):{structuredContent:{ok:true}});});t.after(()=>f.close());const c=new NativeMail({url:f.url,token:'private-mail-token'});t.after(()=>c.close());
    if(safe)assert.deepEqual(await c.call('register_agent',{}),{ok:true});else await assert.rejects(c.call('register_agent',{}),e=>e instanceof Error&&!e.message.includes('private-mail-token')&&e.message.includes('[redacted]'));assert.equal(count,safe?3:1);
  }
});

test('native retry bounds honor Retry-After and stop after five tool attempts',{timeout:30000},async t=>{
  for(const mode of ['busy','throttle','large-delay']) {
    const times:number[]=[];const f=await fixture((body,_req,res)=>{times.push(Date.now());if(mode==='busy')toolReply(res,body,busy);else{res.writeHead(429,{'Retry-After':mode==='large-delay'?'6':'0.15'});res.end('{}');}});t.after(()=>f.close());const c=new NativeMail({url:f.url,token:'fixture'});t.after(()=>c.close());
    await assert.rejects(c.call('fetch_inbox',{}));assert.equal(times.length,mode==='large-delay'?1:5);if(mode==='throttle')assert.ok(times[1]-times[0]>=140);
  }
});

test('native total deadline includes initialization, notification, slow bodies and read retries',{timeout:30000},async t=>{
  for(const mode of ['initialize','body','retry']) {
    const f=await fixture(async(body,_req,res)=>{
      if(mode==='initialize'){await sleep(150);toolReply(res,body,{});}
      else if(mode==='body'){res.writeHead(200);res.write('{');}
      else {await sleep(60);res.writeHead(503);res.end('{}');}
    },{manualInitialize:mode==='initialize'});t.after(()=>f.close());const c=new NativeMail({url:f.url,token:'fixture',budgetMs:mode==='initialize'?250:100});t.after(()=>c.close());
    const start=Date.now();await assert.rejects(c.call('fetch_inbox',{}),e=>mode==='retry'?e instanceof Error:e instanceof DOMException&&e.name==='TimeoutError');assert.ok(Date.now()-start<1000);
    assert.deepEqual(f.requests.map(v=>v.body.method),mode==='initialize'?['initialize','notifications/initialized']:['initialize','notifications/initialized','tools/call']);
  }
});

test('native Mail refuses redirected, malformed, uncorrelated and oversized responses without replay',{timeout:30000},async t=>{
  for(const mode of ['redirect','id','json','empty','tool-shape','oversized','duplicate-sse']) {
    let count=0;const f=await fixture((body,_req,res)=>{count++;
      if(mode==='redirect'){res.writeHead(307,{Location:'/other'});res.end('{}');}
      else if(mode==='id')res.end(JSON.stringify({id:'other',result:{structuredContent:{ok:true}}}));
      else if(mode==='json')res.end('{broken');else if(mode==='empty')res.end('');
      else if(mode==='tool-shape')toolReply(res,body,{isError:'false',structuredContent:{ok:true}});
      else if(mode==='oversized')res.end('x'.repeat(8*1024*1024+1));
      else {res.setHeader('Content-Type','text/event-stream');const frame='data: '+JSON.stringify({id:body.id,result:{structuredContent:{ok:true}}})+'\n\n';res.end(frame+frame);}
    });t.after(()=>f.close());const c=new NativeMail({url:f.url,token:'fixture'});t.after(()=>c.close());await assert.rejects(c.call('send_message',{idempotency_key:'key'}));assert.equal(count,1);
  }
});

test('native initialization and request capacity stay bounded under a burst',{timeout:30000},async t=>{
  let initializing!:()=>void,release!:()=>void;const began=new Promise<void>(r=>{initializing=r;}),held=new Promise<void>(r=>{release=r;});
  const f=await fixture(async(body,_req,res)=>{if(body.method==='initialize'){initializing();await held;toolReply(res,body,{});}else if(body.method==='notifications/initialized')res.end('');else toolReply(res,body,{structuredContent:{ok:true}});},{manualInitialize:true});
  const c=new NativeMail({url:f.url,token:'fixture'});t.after(async()=>{release();await c.close();await f.close();});await c.ready;
  const results=Promise.allSettled(Array.from({length:80},()=>c.call('fetch_inbox',{})));await began;await sleep(100);release();const all=await results;
  assert.equal(all.filter(v=>v.status==='fulfilled').length,64);assert.ok(all.filter(v=>v.status==='rejected').every(v=>v.status==='rejected'&&v.reason.message.includes('pending request limit')));assert.equal(f.requests.filter(v=>v.body.method==='initialize').length,1);
});

test('Mail name and Retry-After policies match retained JavaScript semantics with conservative millisecond rounding',{timeout:30000},async t=>{
  const c=new NativeMail({url:'http://127.0.0.1:1/mcp/',token:'fixture'});t.after(()=>c.close());
  const values=[null,'',' ','\ufeff0.25\ufeff','0','-1','1.5','.2','1e-4','+3','1e3','0x10','0Xff','0o10','0b11','0x','NaN','Infinity','-Infinity','nonsense','Sat, 03 Oct 2026 00:00:02 GMT','2026-10-03T00:00:02Z','2026-10-02T00:00:00Z'];const now=Date.parse('2026-10-03T00:00:00Z');
  const result=await c.command('policy',{values,now});const expected=values.map(raw=>{const n=Number(raw);const ms=Number.isFinite(n)?Math.max(0,n*1000):Math.max(0,Date.parse(raw!)-now);return Number.isFinite(ms)?Math.ceil(ms):0;});assert.deepEqual(result.delays,expected);
  const names=[...new Set(['BlueLake','GreenCastle','RedPine','SilverHill','AmberCreek',...['Blue','Green','Red','Silver','Amber','White','Brown','Orange','Purple','Golden'].flatMap(a=>['Lake','Castle','Pine','Hill','Creek','Mountain','Dog','Stone','River','Forest'].map(n=>a+n))])];assert.deepEqual(result.names,names);
});
