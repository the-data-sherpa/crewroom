import assert from 'node:assert/strict';
import {test,type TestContext} from 'node:test';
import {createServer,type IncomingMessage,type ServerResponse} from 'node:http';
import {gzipSync,deflateSync,deflateRawSync,brotliCompressSync,zstdCompressSync} from 'node:zlib';
import {JiraClient,JiraUnavailable} from '../../server/jira-client';
import {NativeJiraClient} from './provider-client-adapter';
const source={siteUrl:'https://fixture.atlassian.net',cloudId:'00000000-0000-4000-8000-000000000022',filterId:'12345',assigneeAccountId:'operator-1',name:'Assigned work'};
async function http(t:TestContext,handler:(req:IncomingMessage,res:ServerResponse)=>void){
 const server=createServer(handler);await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));t.after(async()=>{server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()));});return `http://127.0.0.1:${(server.address() as {port:number}).port}`;
}
function clients(t:TestContext,apiUrl:string,credentials=async()=> 'synthetic'){
 const native=new NativeJiraClient({source,apiUrl,credentials});t.after(()=>native.close());return [new JiraClient({source,apiUrl,credentials}),native] as const;
}
async function outcome(f:()=>Promise<unknown>):Promise<{ok:boolean,value?:unknown,status?:number,sent?:boolean,message?:string}>{try{return {ok:true,value:await f()};}catch(error:any){return {ok:false,status:error.status,sent:error.requestSent,message:error.message};}}

test('native transport matches decoded JSON, empty responses, compressed limits and malformed streams',async t=>{
 let bytes:Buffer=Buffer.from('{}');let coding:string|undefined;let status=200;
 const api=await http(t,(_req,res)=>{res.statusCode=status;if(coding)res.setHeader('Content-Encoding',coding);res.end(bytes);});const pair=clients(t,api);
 const payload=Buffer.from(JSON.stringify({message:'résumé, 日本語, 😀',items:[1,true,null]}));
 for(const [name,encode] of [['gzip',gzipSync],['x-gzip',gzipSync],['deflate',deflateSync],['deflate',deflateRawSync],['br',brotliCompressSync],['zstd',zstdCompressSync]] as const){coding=name;bytes=encode(payload);assert.deepEqual(await pair[1].request('fixture'),await pair[0].request('fixture'),name);}
 coding='br, gzip';bytes=gzipSync(brotliCompressSync(payload));assert.deepEqual(await pair[1].request('fixture'),await pair[0].request('fixture'));
 for(const value of [Buffer.alloc(0),Buffer.from('null'),Buffer.from('{"text":"\ufffd"}'),Buffer.from([0x22,0xff,0x22])]){coding=undefined;bytes=value;assert.deepEqual(await pair[1].request('fixture'),await pair[0].request('fixture'));}
 status=204;coding='gzip';bytes=Buffer.alloc(0);assert.equal(await pair[1].request('fixture'),undefined);assert.equal(await pair[0].request('fixture'),undefined);status=200;
 for(const value of [Buffer.from('{invalid'),Buffer.from([0xef,0xbb,0xbf,0x7b,0x7d])]){coding=undefined;bytes=value;assert.deepEqual(await outcome(()=>pair[1].request('fixture')),await outcome(()=>pair[0].request('fixture')));}
 coding='gzip';bytes=gzipSync(Buffer.from('"'+'x'.repeat(4*1024*1024)+'"'));assert.deepEqual(await outcome(()=>pair[1].request('fixture')),await outcome(()=>pair[0].request('fixture')));
 coding=undefined;bytes=Buffer.from('"'+'x'.repeat(4*1024*1024-2)+'"');assert.equal((await pair[1].request('fixture')).length,4*1024*1024-2);assert.equal((await pair[0].request('fixture')).length,4*1024*1024-2);
 coding='gzip';bytes=Buffer.from('not gzip');assert.equal((await outcome(()=>pair[1].request('fixture'))).sent,true);assert.equal((await outcome(()=>pair[0].request('fixture'))).sent,true);
});

test('native transport preserves callback order and conservative sent outcomes without retrying',async t=>{
 let calls=0;let status=200;let lost=false;let body='{}';let redirect=false;
 const api=await http(t,(req,res)=>{calls++;if(lost){req.socket.destroy();return;}res.statusCode=status;if(redirect)res.setHeader('Location',api+'/outside');res.end(body);});
 for(const mode of ['credentials','before','ok','lost','401','429','500','301','304','parse']){
  const order:string[]=[];calls=0;status=Number(mode)||200;lost=mode==='lost';body=mode==='parse'?'{invalid':'{}';redirect=mode==='301';
  const pair=clients(t,api,async()=>{order.push('credentials');if(mode==='credentials')throw new JiraUnavailable('Fixture credentials unavailable.');return 'synthetic';});
  const results=[];
  for(const client of pair){const before=calls;order.length=0;results.push(await outcome(()=>client.request('fixture','POST',{value:1},async()=>{order.push('before');if(mode==='before')throw new JiraUnavailable('Fixture dispatch refused.');})));assert.deepEqual(order,mode==='credentials'?['credentials']:['credentials','before']);assert.equal(calls-before,['credentials','before'].includes(mode)?0:1);}
  assert.deepEqual(results[1],results[0],mode);
 }
});

test('native credential bytes and header rejection match the retained dispatch boundary',async t=>{
 let wire:string|undefined;let calls=0;const api=await http(t,(req,res)=>{calls++;wire=req.headers.authorization;res.end('{}');});
 for(const token of ['synthetic',' synthetic ','synthetic\t','é','Ā','synthetic\0','synthetic\u000b','synthetic\u007f','synthetic\u0080','','synthetic\r','synthetic\n']){
  const pair=clients(t,api,async()=>token);const results=[];
  for(const client of pair){const prior=calls;wire=undefined;let before=0;const result=await outcome(()=>client.request('fixture','GET',undefined,()=>{before++;}));results.push({result,wire,calls:calls-prior,before});}
  assert.deepEqual(results[1],results[0],JSON.stringify(token));
 }
});

test('native request deadline includes stalled response bodies and remains a sent unknown outcome',{timeout:40000},async t=>{
 const api=await http(t,(_req,res)=>{res.writeHead(200,{'Content-Type':'application/json'});res.write('{');});const pair=clients(t,api);
 const started=Date.now();const results=await Promise.all(pair.map(c=>outcome(()=>c.request('fixture','POST',{}))));assert.ok(Date.now()-started>=14000);assert.deepEqual(results[1],results[0]);assert.equal(results[1].sent,true);
});

test('native decoding bounds refuse excessive codings, deep JSON and unpaired surrogates without claiming an unsent request',async t=>{
 let bytes=Buffer.from('{}');let coding:string|undefined;let calls=0;
 const api=await http(t,(_req,res)=>{calls++;if(coding)res.setHeader('Content-Encoding',coding);res.end(bytes);});const pair=clients(t,api);
 for(const mode of ['six-codings','deep-json','unpaired-surrogate']){
  coding=undefined;
  if(mode==='six-codings'){coding=Array(6).fill('gzip').join(', ');bytes=Buffer.from('{}');for(let i=0;i<6;i++)bytes=gzipSync(bytes);}
  else bytes=Buffer.from(mode==='deep-json'?'['.repeat(128)+'0'+']'.repeat(128):'"\\ud800"');
  const retained=await outcome(()=>pair[0].request('fixture','POST',{}));assert.equal(retained.ok,mode!=='six-codings',mode+' retained parser boundary');
  const prior=calls;const result=await outcome(()=>pair[1].request('fixture','POST',{}));assert.equal(result.ok,false,mode);assert.equal(result.sent,true,mode);assert.equal(result.status,undefined,mode);assert.equal(calls-prior,1,mode+' no retry');
  if(mode==='six-codings')assert.deepEqual(result,retained);
 }
});
