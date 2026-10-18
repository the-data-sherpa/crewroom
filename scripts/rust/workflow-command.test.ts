import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFileSync,existsSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';
import {reviewedCandidate} from './workflow-coding-fixture';
import {ContainerCommand} from './container-adapter';
import {checkImage,dockerSocket} from './docker-engine';
const socket=dockerSocket();
test('Pause and concurrent Close stop a real active Docker check, drain its backend work and preserve interrupted evidence',{timeout:120000},async t=>{
 const f=await workflowFixture(t,'coding',2);const {reviewer}=await reviewedCandidate(f,[{id:'slow',image:checkImage(),command:'/bin/sh',args:['-c','echo workflow-command-started; sleep 60'],timeoutMs:90000,memoryMiB:256,tmpExecutable:false,required:false}]);
 await f.input(reviewer,{kind:'tool',name:'run_check',toolCallId:'interrupted-check',arguments:{checkId:'slow'}},'long-check');const root=join(f.runtime,reviewer,'tools',createHash('sha256').update('interrupted-check').digest('hex'));const job=new ContainerCommand(root,{socket});
 await until(()=>existsSync(join(root,'live.log'))&&readFileSync(join(root,'live.log'),'utf8').includes('workflow-command-started'));assert.equal((await job.inspect()).state,'running');
 const pausing=f.pause();await until(async()=>(await f.c.command('run',{target:f.run.id})).desiredState==='paused');const closing=f.c.command('close_runtime');assert.equal((await pausing).status,'paused');assert.deepEqual(await closing,{stopped:true,errors:[]});
 assert.equal((await job.inspect()).state,'stopped');const receipt=await job.receipt();assert.equal(receipt.started,true);assert.equal(receipt.completed,false);assert.match(receipt.output,/workflow-command-started/);assert.equal(JSON.parse(readFileSync(join(root,'container.json'),'utf8')).removed,true);
 const record=await f.c.command('record',{target:f.run.id});const interrupted=record.checks.find((c:any)=>c.checkId==='slow');assert.ok(interrupted);assert.equal(interrupted.completed,false);assert.equal(record.checkOperations.find((o:any)=>o.toolCallId==='interrupted-check').state,'recorded');assert.equal(await f.c.command('busy',{target:f.run.id}),false);assert.ok((await f.snapshot()).attempts.every((a:any)=>a.ownership==='stopped'));
});
