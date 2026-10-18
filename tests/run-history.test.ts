import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { historyFixture } from './fixtures/history';
import { Workbench } from '../server/workbench';
import { readRunHistory, recordedCause } from '../server/run-history';
import { runCauses, runNextAction, runOutcome } from '../shared/run-history';
import { Results } from '../server/results';
import { parseTicketLocation, ticketHref } from '../src/navigation';
import type { Run } from '../shared/types';

test('HIS-01: attempts retain recorded causes, verified timing, exact-revision evidence and partial usage after reopen', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-history-')); const fixture = await historyFixture(root);
  let store = fixture.store;
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const legacy = store.getAttempt(fixture.coordinator.attempt.id);
  delete legacy.createdAt; delete legacy.startedAt; delete legacy.stoppedAt;
  (store as any).db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify(legacy), legacy.id);
  const before = readRunHistory(store, fixture.run.id);
  const legacyDetail = before.attempts.find(item => item.attemptId === legacy.id)!;
  assert.deepEqual([legacyDetail.createdAt, legacyDetail.startedAt, legacyDetail.stoppedAt], [null, null, null]);
  const startedAt = store.getRun(fixture.run.id).startedAt; assert.ok(startedAt);
  const retired = before.attempts.find(item => item.name === 'Reporter')!;
  assert.ok(retired.retiredAt); assert.equal(retired.retirementReason, 'Fixture reporting complete');
  assert.match(retired.causes.join('\n'), /Assignment retired: Fixture reporting complete/);
  assert.doesNotMatch(retired.nextAction, /current attempt/); assert.match(retired.nextAction, /cannot resume/);
  const old = before.attempts.find(item => item.attemptId === fixture.oldAttemptId)!;
  assert.equal(old.current, false); assert.equal(old.state, 'exited');
  assert.ok(old.startedAt && old.createdAt && old.stoppedAt);
  assert.ok(Date.parse(old.createdAt!) <= Date.parse(old.startedAt!));
  assert.ok(Date.parse(old.startedAt!) <= Date.parse(old.stoppedAt!));
  assert.match(old.causes.join('\n'), /Fixture turn interrupted/); assert.match(old.causes.join('\n'), /signal SIGTERM/);
  assert.match(old.causes.join('\n'), /Fixture process identity was uncertain/, 'Reconciliation does not erase an earlier recorded ownership reason.');
  assert.equal(old.evidence.length, 2);
  for (const item of old.evidence) assert.ok(item.revision.includes(fixture.candidate.revision.head) && item.revision.includes(fixture.candidate.revision.base));
  assert.ok(old.evidence.every(item => item.artifact?.sha256));
  const replacement = before.attempts.find(item => item.attemptId === fixture.replacement.id)!;
  assert.deepEqual(replacement.evidence, []); assert.match(replacement.causes[0], /Cause unknown/);
  assert.equal(before.usage.total.state, 'partial'); assert.equal(before.usage.total.tokens?.total, 20);
  assert.ok(before.usage.total.incompleteAttempts.some(item => item.attemptId === replacement.attemptId));
  assert.equal(before.usage.rows.find(item => item.attemptId === old.attemptId)?.state, 'actual');
  assert.equal(before.usage.rows.find(item => item.attemptId === replacement.attemptId)?.tokens, null);
  assert.deepEqual((await new Results(store, fixture.work, fixture.evidence).read(fixture.run.id)).usage, before.usage, 'Wall and Results share one aggregator.');
  const noUsage = readRunHistory(store, fixture.next.id).usage;
  assert.equal(noUsage.total.state, 'unavailable'); assert.equal(noUsage.total.tokens, null); assert.equal(noUsage.total.costUsd, null);
  store.close(); store = new Workbench(join(root, 'workflow.sqlite'));
  assert.deepEqual(readRunHistory(store, fixture.run.id), before);
  assert.equal(store.getRun(fixture.run.id).startedAt, startedAt);
});

test('HIS-02: missing legacy timing stays unknown; run causes and next actions use recorded facts', () => {
  const base: Run = { id: 'r', taskId: 't', status: 'attention_required', createdAt: '2026-10-01', taskRevision: 1, taskSnapshot: {} as Run['taskSnapshot'], roles: [], stages: [] };
  assert.deepEqual(runCauses(base), ['Cause unknown: no reason was recorded.']);
  for (const [patch, reason, next] of [
    [{ attentionReason: 'Process ownership blocked' }, /Process ownership blocked/, /Resolve ownership/],
    [{ pauseReason: 'budget', status: 'paused' }, /Active-time limit/, /higher total/],
    [{ launchHold: { reason: 'Git setup failed', at: 'now', source: 'preparation' } }, /Preparation held: Git setup failed/, /Save launch details/],
    [{ launchHold: { reason: 'Role disabled', at: 'now', source: 'preflight' } }, /Launch held: Role disabled/, /rechecks/],
  ] as const) {
    const run = { ...base, ...patch }; assert.match(runCauses(run).join('\n'), reason); assert.match(runNextAction(run), next);
  }
  assert.match(runOutcome({ ...base, status: 'cancelled' }), /Cancelled/);
  assert.equal(recordedCause({ type: 'process_exit', code: 17, signal: null }), 'Runner exited: code 17.');
  assert.match(recordedCause({ type: 'prompt_result', status: 'error' })!, /unknown/);
  assert.match(recordedCause({ type: 'model_policy_rejected', requestedModel: 'fixture/model', requestedEffort: 'high' })!, /fixture\/model · high/);
  assert.equal(recordedCause({ type: 'prompt_result', status: 'success' }), undefined);
});

test('HIS-03: ticket URLs preserve run, attempt and tab without confusing ticket identity', () => {
  const route = { taskId: 'site:task/?', runId: 'run&one', attemptId: 'attempt#two', tab: 'evidence' as const };
  assert.deepEqual(parseTicketLocation(ticketHref(route)), route);
  assert.equal(parseTicketLocation('#legacy-ticket').taskId, 'legacy-ticket');
  assert.equal(parseTicketLocation('#ticket?run=one&tab=missing').tab, undefined);
  assert.equal(parseTicketLocation('#bad%escape').taskId, 'bad%escape');
});


test('HIS-06: a queued replacement has unavailable usage but does not reduce completeness of executed work', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-queued-usage-')); const store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const task = store.createTask({ title: 'Usage', description: '' });
  const run = store.queueRefinement(task.id, { revision: task.revision, purpose: 'explore' });
  const { defaultRoles } = await import('../server/roles');
  const { resolveAssignment } = await import('../server/runtime/model-policy');
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { assignment, attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Explore' });
  store.claimAttempt(attempt.id);
  store.recordAgentEvent(attempt.id, 1, { type: 'provider_request' });
  store.recordAgentEvent(attempt.id, 2, { type: 'provider_usage', state: 'actual', usage: { input: 1, output: 1, total: 2, cacheRead: 0, cacheWrite: 0, reasoning: 0 } });
  store.recordOwnership(attempt.id, true); store.beginPause(run.id); store.finishPause(run.id);
  const queued = store.replaceAttempt(assignment.id, { automatic: false });
  const usage = readRunHistory(store, run.id).usage;
  assert.equal(usage.total.state, 'actual'); assert.equal(usage.total.tokens?.total, 2);
  assert.deepEqual(usage.total.incompleteAttempts, []);
  const row = usage.rows.find(item => item.attemptId === queued.id)!;
  assert.equal(row.state, 'unavailable'); assert.equal(row.tokens, null); assert.equal(row.awaitingStart, true);
});
