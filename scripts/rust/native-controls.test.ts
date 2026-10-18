/** Native controls connected to actual service-owned host/RPC processes. */
import assert from 'node:assert/strict';
import {readFileSync, writeFileSync, rmSync} from 'node:fs';
import {join} from 'node:path';
import test from 'node:test';
import {NativeDaemon} from './daemon-client';
import {nativeUi} from './native-ui-driver';
import {inspectOwned} from './process-adapter';
import {workflowFixture, until} from './workflow-support';

test('native launch automatically admits coding; busy insertion and Pause preserve intermediate and stopped facts through restart', {timeout:180000}, async t => {
  const f = await workflowFixture(t, 'coding', 0, false, {service:true, configure({config}) {
    config.runtime.launchPrefix.push('--unique-sessions');
  }});
  assert.ok(f.c instanceof NativeDaemon);
  const ui = (action: string, extra: Record<string,unknown> = {}) => nativeUi(t, f.root,
    {descriptor:join(f.config.authority, 'native-connection.json'), taskId:f.task.id, runId:f.run.id, action, ...extra});
  await ui('queue');
  const first = await f.idle();
  assert.equal(first.run.status, 'running'); assert.ok(first.run.launchCompletedAt);
  assert.equal(first.assignments.length, 1); assert.equal(first.attempts.length, 1);
  const coordinator = first.assignments[0].currentAttemptId;
  // A real host/RPC pair is held busy while the operator proposes another role.
  const gate = join(f.runtime, coordinator, 'native-work');
  await f.input(coordinator, {gate:'native-work', events:[{type:'agent_start'}]}, 'ongoing-native-work');
  await until(async () => (await f.snapshot()).attempts.find((a:any) => a.id === coordinator)?.state === 'executing');
  const inserted = await ui('insert');
  assert.equal(inserted.runtime.assignments.length, 1);
  assert.equal(inserted.runtime.teamProposals[0].status, 'pending');
  writeFileSync(gate+'-release', 'release');
  await until(() => readFileSync(join(f.runtime, coordinator, 'events.jsonl'), 'utf8').split('\n').filter(Boolean)
    .map(line => JSON.parse(line).event).some(e => e.type === 'prompt_result' && e.id === 'ongoing-native-work'));
  await f.input(coordinator, {events:[{type:'session_settled'}]}, 'native-work-settled');
  const applied = await until(async () => {
    const value = await f.snapshot();
    return value.teamProposals[0]?.status === 'applied' && value.assignments.length === 2 && !value.busy ? value : false;
  }, 45000);
  assert.notEqual(applied.assignments[0].currentAttemptId, coordinator);
  const idle = await f.idle();
  // Stop only the exact disposable runner PID from its authenticated descriptor.
  // Native shutdown must retain pausing until its process helper confirms stop.
  const current = idle.attempts.find((a:any) => a.id === idle.assignments[0].currentAttemptId);
  const descriptor = await inspectOwned(join(f.runtime, current.id, 'omp-process'));
  assert.equal(descriptor.state, 'live');
  assert.equal(descriptor.owner.pid, current.ompPid);
  process.kill(current.ompPid, 'SIGSTOP');
  const paused = await ui('pause');
  assert.equal(paused.sawPausingWithUnstoppedOwnership, true);
  assert.ok(paused.runtime.attempts.every((a:any) => a.ownership === 'stopped'));
  await f.c.close(); await f.reopen();
  const reopened = await ui('inspect', {status:'paused'});
  assert.equal(reopened.runtime.assignments.length, 2);
  assert.equal(reopened.runtime.teamProposals[0].status, 'applied');
  assert.ok(reopened.runtime.attempts.every((a:any) => a.ownership === 'stopped'));
  assert.equal(reopened.runtime.run.desiredState, 'paused');
  // Simulate an older Resume arriving after a newer Pause, even when already paused.
  const controlRevision = reopened.runtime.run.controlRevision;
  await f.c.request(`/api/runs/${f.run.id}/pause`, {});
  assert.ok((await f.snapshot()).run.controlRevision > controlRevision);
  await assert.rejects(f.c.request(`/api/runs/${f.run.id}/resume`, {controlRevision}), /newer control/);
  assert.equal((await f.snapshot()).run.desiredState, 'paused');
  assert.ok((await f.snapshot()).attempts.every((a:any) => a.ownership === 'stopped'));
  await ui('resume'); await f.idle(); await f.pause();
  rmSync(gate+'-release', {force:true});
});
