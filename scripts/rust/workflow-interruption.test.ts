import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import test from 'node:test';
import {workflowFixture,until} from './workflow-support';
import {prepareContributedPair} from './workflow-coding-fixture';
import {gitLatch} from '../../tests/fixtures/git-latch';

test('controller SIGKILL during coding transition recovers one committed candidate and fresh current agents',{timeout:90000},async t=>{
 const f=await workflowFixture(t,'coding',2);const {work,coordinator,writer}=await prepareContributedPair(f);const latch=gitLatch(f.root,'commit');
 try{
  await f.kill();await f.reopen();const original=await f.idle();
  latch.arm();const completion=await f.tool(writer,'coding/complete',{summary:'Recover the committed effect'},'complete');await latch.blocked();assert.equal((await f.c.command('run',{target:f.run.id})).codingTransition.completionId,completion.id);
  await f.kill();latch.release();await latch.finished();const committed=execFileSync('git',['-C',work.path,'rev-parse','HEAD'],{encoding:'utf8'}).trim();assert.notEqual(committed,completion.head);
  await f.reopen();await until(async()=>!(await f.c.command('busy',{target:f.run.id})));const run=await f.c.command('run',{target:f.run.id});assert.equal(run.status,'running',JSON.stringify(run));await f.idle();const record=await f.c.command('record',{target:f.run.id});assert.equal(record.candidates.length,1);assert.equal(record.candidates[0].revision.head,committed);assert.equal(record.completions[0].state,'completed');assert.equal(run.codingTransition,undefined);assert.equal((await f.snapshot()).attempts.length,original.attempts.length+4);
 }finally{latch.release();latch.restore();}
});
