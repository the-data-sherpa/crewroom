import assert from 'node:assert/strict';
import {test} from 'node:test';
import {mkdirSync,mkdtempSync,rmSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {ContainerCommand as Native} from './container-adapter';
import {ContainerCommand as Retained} from '../../server/runtime/container-command';
import {checkImage,requireDockerEngine} from './docker-engine';
const engine=await requireDockerEngine();
const image=checkImage();
async function until(check:()=>boolean){const end=Date.now()+5000;while(!check()){assert.ok(Date.now()<end,'Fixture event did not arrive');await new Promise(r=>setTimeout(r,10));}}
test('live native logs can stop observation without stopping the Docker command',async t=>{
 const root=mkdtempSync(join(tmpdir(),'cr-native-follow-'));const workspace=join(root,'workspace');mkdirSync(workspace);const job=new Native(join(root,'job'),engine);
 t.after(async()=>{await job.stop();await job.remove();rmSync(root,{recursive:true,force:true});});
 await job.start({image,workspace,command:'/bin/sh',args:['-c',"printf 'live résumé 🦀\\n'; sleep 60"]});
 const chunks:Buffer[]=[];const cancel=new AbortController();const following=job.follow(b=>chunks.push(b),cancel.signal);await until(()=>Buffer.concat(chunks).includes(Buffer.from('live résumé 🦀\n')));cancel.abort();await following;
 assert.equal((await job.inspect()).state,'running');await job.stop();const receipt=await job.receipt();assert.equal(receipt.completed,false);assert.match(receipt.output,/live résumé 🦀/);await job.remove();assert.deepEqual(await new Native(job.root,engine).receipt(),receipt);
});
test('explicit temporary execution and memory bounds survive real Docker inspection',async t=>{
 const root=mkdtempSync(join(tmpdir(),'cr-native-tmp-exec-'));const workspace=join(root,'workspace');mkdirSync(workspace);const job=new Native(join(root,'job'),engine);
 t.after(async()=>{await job.stop();await job.remove();rmSync(root,{recursive:true,force:true});});
 await job.start({image,workspace,command:'/bin/sh',args:['-c',"printf '#!/bin/sh\\necho admitted-temporary-executable\\n' > /tmp/check; chmod 700 /tmp/check; /tmp/check"],timeoutMs:5000,memoryMiB:128,tmpExecutable:true});await job.wait();const receipt=await job.receipt();assert.equal(receipt.completed,true);assert.equal(receipt.exitCode,0);assert.match(receipt.output,/admitted-temporary-executable/);await job.remove();
});
test('Rust and retained TypeScript reconcile each other’s current-format command journals and receipts',async t=>{
 const root=mkdtempSync(join(tmpdir(),'cr-native-container-cross-'));const workspace=join(root,'workspace');mkdirSync(workspace);const jobs:Native[]=[];
 t.after(async()=>{for(const job of jobs){await job.stop();await job.remove();}rmSync(root,{recursive:true,force:true});});
 for(const fromNative of [false,true]){
  const path=join(root,String(fromNative));const native=new Native(path,engine),retained=new Retained(path,engine);jobs.push(native);
  const starter=fromNative?native:retained;const other=fromNative?retained:native;
  await starter.start({image,workspace,command:'/bin/sh',args:['-c','echo cross-language-retained; sleep 60']});assert.equal((await other.inspect()).state,'running');
  assert.equal((await other.stop()).state,'stopped');const receipt=await other.receipt();assert.equal(receipt.completed,false);assert.match(receipt.output,/cross-language-retained/);await other.remove();assert.deepEqual(await starter.receipt(),receipt);assert.equal((await starter.inspect()).state,'stopped');
 }
});
