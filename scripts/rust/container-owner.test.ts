/** A real Docker command must read a private candidate without root capabilities. */
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {mkdirSync,mkdtempSync,readFileSync,rmSync,statSync,writeFileSync} from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {ContainerCommand} from './container-adapter';
import {requireDockerEngine} from './docker-engine';

test('private candidate exports are readable by the pinned local owner and read-only work stays protected', async t=>{
 const image=process.env.CREWROOM_CHECK_IMAGE;assert.ok(image,'Set a pinned local image with /bin/sh');
 const root=mkdtempSync(join(tmpdir(),'crewroom-container-owner-'));
 const workspace=join(root,'source');mkdirSync(workspace,{mode:0o700});
 writeFileSync(join(workspace,'private.txt'),'candidate content\n',{mode:0o600});
 const engine=await requireDockerEngine();
 const jobs:ContainerCommand[]=[];
 t.after(async()=>{for(const job of jobs){await job.stop();await job.remove();}rmSync(root,{recursive:true,force:true});});
 for(const readOnlyWorkspace of [false,true]){
  const job=new ContainerCommand(join(root,String(readOnlyWorkspace)),engine);jobs.push(job);
  await job.start({image,workspace,readOnlyWorkspace,command:'/bin/sh',args:['-c',
   readOnlyWorkspace?'cat private.txt && if echo changed > private.txt; then exit 2; fi':'cat private.txt && echo writable-copy > output.txt'],timeoutMs:5000});
  await job.wait();const receipt=await job.receipt();assert.equal(receipt.exitCode,0,receipt.output);
  assert.match(receipt.output,/candidate content/);
  const intent=JSON.parse(readFileSync(join(job.root,'container.json'),'utf8'));
  assert.equal(receipt.user,intent.user);
  assert.equal(intent.user,`${process.geteuid!()}:${process.getegid!()}`);
  await job.remove();assert.deepEqual(await new ContainerCommand(job.root,engine).receipt(),receipt);
 }
 assert.equal(readFileSync(join(workspace,'private.txt'),'utf8'),'candidate content\n');
 assert.equal(readFileSync(join(workspace,'output.txt'),'utf8'),'writable-copy\n');
 assert.equal(statSync(workspace).mode&0o777,0o700);
 assert.equal(statSync(join(workspace,'private.txt')).mode&0o777,0o600);
});
