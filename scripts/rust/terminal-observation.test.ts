/** Real checked processes/tmux; scripted provider/RPC peer, no live agent delivery. */
import assert from 'node:assert/strict';
import test from 'node:test';
import {supervisorFixture} from './supervisor-support';

test('terminal admission binds exact process ownership, run, pane and window', {timeout:45000}, async t => {
  const f=await supervisorFixture(t,2);
  const seed=f.seeds[0];
  const request=()=>f.c.command('terminal',{runId:seed.run.id,target:seed.attempt.id});
  await assert.rejects(request(),/identity|pane/);
  const launched=await f.launch();
  const second=await f.launch(1);
  const secondDescriptor=await f.c.command('terminal',{runId:f.seeds[1].run.id,target:f.seeds[1].attempt.id});
  assert.equal(secondDescriptor.pane,second.pane);
  const descriptor=await request();
  assert.equal(descriptor.readOnly,true);assert.equal(descriptor.state,'live');
  assert.equal(descriptor.hostPid,launched.hostPid);assert.equal(descriptor.ompPid,launched.ompPid);
  assert.equal(descriptor.pane,launched.pane);assert.match(descriptor.window,/^@\d+$/);
  assert.ok(descriptor.socketInode>0);assert.equal(descriptor.session,'crewroom');
  assert.doesNotMatch(JSON.stringify(descriptor),/providerToken|hostToken|synthetic-|authorization/);
  f.tmux('new-session','-d','-t','crewroom','-s','crewroom-view-fixture');
  f.tmux('select-window','-t',`crewroom-view-fixture:${secondDescriptor.window}`);
  assert.equal((await request()).pane,launched.pane);
  assert.equal((await f.c.command('terminal',{runId:f.seeds[1].run.id,target:f.seeds[1].attempt.id})).pane,second.pane);
  f.tmux('kill-session','-t','crewroom-view-fixture');
  await assert.rejects(f.c.command('terminal',{runId:f.seeds[1].run.id,target:seed.attempt.id}),/belong/);
  f.tmux('rename-window','-t',descriptor.window,'wrong-attempt');
  await assert.rejects(request(),/pane does not match/);
  f.tmux('rename-window','-t',descriptor.window,seed.attempt.id);
  assert.equal((await request()).pane,descriptor.pane);
  assert.equal((await f.stop()).stopped,true);
  const retained=await request();assert.equal(retained.state,'stopped');
  assert.equal(retained.pane,descriptor.pane);
});
