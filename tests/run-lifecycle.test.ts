import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { Run } from '../shared/types';
import { transitionRun } from '../server/run-lifecycle';

const initial = (): Run => ({ id: 'run', taskId: 'ticket', status: 'awaiting_runner', position: 0, createdAt: '2026-09-28', taskRevision: 1, taskSnapshot: {} as Run['taskSnapshot'], roles: [], stages: [], activeMs: 0, activeLimitMs: 1000 });

test('a budget stop charges active time once, retains its reason through pause, and resumes only after extension', () => {
  const original = initial();
  const running = transitionRun(original, { type: 'start' }, 100);
  assert.equal(original.status, 'awaiting_runner', 'transitions never mutate persisted input');
  assert.equal(running.status, 'running');
  assert.equal(running.startedAt, new Date(100).toISOString());
  const timedOut = transitionRun(running, { type: 'account' }, 1200);
  assert.deepEqual([timedOut.status, timedOut.pauseReason, timedOut.activeMs, timedOut.activeSince], ['pausing', 'budget', 1100, 1200]);
  const paused = transitionRun(timedOut, { type: 'finishPause', unresolved: false }, 1300);
  assert.deepEqual([paused.status, paused.pauseReason, paused.activeMs, paused.activeSince], ['paused', 'budget', 1200, undefined]);
  assert.throws(() => transitionRun(paused, { type: 'start' }, 2000), /budget/i);
  const extended = transitionRun(paused, { type: 'setLimit', activeLimitMs: 2000 }, 2000);
  assert.deepEqual([extended.status, extended.pauseReason, extended.activeMs], ['paused', 'budget', 1200]);
  assert.equal(transitionRun(extended, { type: 'start' }, 2000).startedAt, running.startedAt, 'Resume retains the first recorded start.');
  assert.deepEqual([transitionRun(extended, { type: 'start' }, 2000).status, extended.status], ['running', 'paused']);
});
