import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { mkdtempSync,rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { gzipSync,deflateSync,deflateRawSync,brotliCompressSync,zstdCompressSync } from 'node:zlib';
import { ProviderGateway } from './gateway-adapter';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { defaultRoles } from '../../server/roles';
test('native HTTP decoding preserves compressed JSON/SSE reports and refuses broken streams',async t=>{
 const root=mkdtempSync(join(tmpdir(),'crewroom-native-codings-'));let encoded:Buffer=Buffer.alloc(0),coding='',contentType='application/json';const observed:any[]=[];
 const upstream=createServer((req,res)=>{assert.equal(req.headers['accept-encoding'],'gzip, deflate');res.writeHead(200,{'Content-Type':contentType,'Content-Encoding':coding});res.end(encoded);});
 await new Promise<void>(resolve=>upstream.listen(0,'127.0.0.1',resolve));
 const plan=resolveAssignment({...defaultRoles[0],model:'fixture/model',effort:'off'},[{provider:'fixture',id:'model',api:'openai-responses',inputs:['text'],efforts:['off'],tools:true}],{},{});
 const gateway=new ProviderGateway(root,plan,{baseUrl:`http://127.0.0.1:${(upstream.address() as any).port}/v1`,apiKey:'private-key',models:[{id:'model',contextWindow:100000,maxTokens:1000}]},'host-token',v=>observed.push(v));
 const front=createServer((req,res)=>{void gateway.serve(req,res)});await new Promise<void>(resolve=>front.listen(0,'127.0.0.1',resolve));
 t.after(async()=>{await gateway.closed();front.closeAllConnections();upstream.closeAllConnections();await Promise.all([new Promise<void>(r=>front.close(()=>r())),new Promise<void>(r=>upstream.close(()=>r()))]);rmSync(root,{recursive:true,force:true});});
 const call=()=>fetch(`http://127.0.0.1:${(front.address() as any).port}/provider/v1/responses`,{method:'POST',headers:{Authorization:'Bearer host-token','Content-Type':'application/json'},body:JSON.stringify({model:'model',input:'fixture'})});
 const encoders:[string,(b:Buffer)=>Buffer][]=[['gzip',gzipSync],['x-gzip',gzipSync],['deflate',deflateSync],['deflate',deflateRawSync],['br',brotliCompressSync],['zstd',zstdCompressSync],['gzip, br',b=>brotliCompressSync(gzipSync(b))],['gzip',b=>Buffer.concat([gzipSync(b.subarray(0,10)),gzipSync(b.subarray(10))])],['custom',b=>b]];
 for(const stream of [false,true])for(const [name,encode] of encoders){
  const event={object:'response',status:'completed',output:[{type:'message',content:[{type:'output_text',text:'résumé 🦀'}]}],usage:{input_tokens:3,output_tokens:7}};
  const payload=stream?'data: '+JSON.stringify({type:'response.completed',response:event})+'\n\n':JSON.stringify(event);
  t.diagnostic(`decoding ${name} stream=${stream}`);coding=name;contentType=stream?'text/event-stream':'application/json';encoded=encode(Buffer.from(payload));const before=observed.length;
  const response=await call();assert.equal(response.status,200);assert.equal(await response.text(),payload,`${name} stream=${stream}`);
  const usage=observed.slice(before).filter(v=>v.type==='provider_usage');assert.equal(usage.length,1);assert.equal(usage[0].state,'actual');assert.equal(usage[0].usage.total,10);assert.equal(gateway.failure,undefined);
 }
 t.diagnostic('rejecting malformed gzip');coding='gzip';encoded=Buffer.from('invalid compressed body');await assert.rejects(async()=>{const response=await call();await response.text();});
 for(let i=0;i<100&&gateway.failure?.kind!=='disconnect';i++)await new Promise(r=>setTimeout(r,10));
 assert.equal(gateway.failure?.kind,'disconnect');
 coding='gzip';encoded=Buffer.alloc(0);let response=await call();assert.equal(await response.text(),'');
 coding=Array(6).fill('gzip').join(',');encoded=gzipSync(Buffer.from('{}'));response=await call();assert.equal(response.status,502);await response.text();
});
