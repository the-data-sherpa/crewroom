import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import type { PublicationRecord } from '../shared/publication';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-slot-'));
  seedRepository(root);
  const db = join(root, 'workflow.sqlite');
  const stores = [new Workbench(db)];
  const store = stores[0];
  const open = () => { const reopened = new Workbench(db); stores.push(reopened); return reopened; };
  t.after(() => { stores.forEach(item => item.close()); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const launch = { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' as const } };
  const queue = (title: string, withLaunch = true) => {
    const task = store.createTask({ title, description: '', repositoryId: repo.id });
    const accepted = store.refine(task.id, { revision: task.revision, sourceRevision: 1, brief: title, criteria: ['One result'] });
    return store.queue(task.id, { revision: accepted.revision, requestId: randomUUID(), ...(withLaunch ? { launch } : {}) }, defaultRoles);
  };
  return { root, db, store, open, repo, role, model, launch, queue };
}

test('QUE-01/03: admission holds through idle, PR creation, CI pending, operator pause and attention; ready requests a durable review release', async t => {
  const f = fixture(t);
  const first = f.queue('First'); const second = f.queue('Second');
  const decision = f.store.claimCodingSlot(first.id);
  assert.ok(decision.schedule?.id); assert.equal(f.store.codingQueue().slot?.runId, first.id);
  assert.match(f.store.codingQueue().plans[0].holdReason!, /Waiting for .* ready for review/);
  const work = await new RepositoryWork(f.store, join(f.root, 'worktrees')).prepare(first.id, { mode: 'new' });
  assert.equal(work.state, 'ready');
  const team = ['Coordinator', 'Writer', 'Reviewer', 'Reporter'].map((name, index) => f.store.admitAssignment(first.id,
    { name, role: f.role, model: f.model, coordinator: index === 0, input: `Work as ${name}.` }));
  assert.equal(team.length, 4); assert.equal(f.store.runtime(first.id).assignments.length, 4);
  const attempt = team[0].attempt;
  f.store.claimAttempt(attempt.id);
  f.store.recordAttemptIdentity(attempt.id, { hostPid: 1, ompPid: 2, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:9', pane: '%1' });
  f.store.startExecution(first.id);
  f.store.recordAgentEvent(attempt.id, 1, { type: 'session_settled' });
  assert.equal(f.store.codingSlot()?.id, first.id, 'An idle terminal never releases the slot.');
  const record: PublicationRecord = { id: randomUUID(), runId: first.id, attemptId: attempt.id, candidateId: randomUUID(),
    repositoryPath: f.root, repository: { uuid: `{${randomUUID()}}`, workspace: 'fixture', slug: 'repo' }, branch: work.branch,
    target: 'main', revision: work.checkpoints.at(-1)!.revision, title: 'First', description: 'Review', policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1 }, ready: false, blockers: ['Hosted checks pending'] };
  f.store.savePublication(record);
  assert.equal(f.store.codingSlot()?.id, first.id, 'PR creation is not readiness.');
  f.store.savePublication({ ...record, blockers: ['CI pending'] });
  assert.equal(f.store.codingSlot()?.id, first.id);
  f.store.setDesiredState(first.id, 'paused'); f.store.beginPause(first.id); f.store.recordOwnership(attempt.id, true); f.store.finishPause(first.id);
  assert.equal(f.store.codingSlot()?.id, first.id, 'Operator pause keeps an unready slot.');
  f.store.requireAttention(first.id, 'Inspect validation');
  assert.equal(f.store.codingSlot()?.id, first.id, 'Attention keeps an unready slot.');
  f.store.savePublication({ ...record, ready: true, blockers: [] });
  const released = f.store.releaseCodingSlot(first.id, 'review', record.candidateId);
  assert.equal(released.status, 'pausing'); assert.equal(released.pauseReason, 'review');
  assert.equal(released.slotRelease?.candidateId, record.candidateId);
  assert.equal(f.store.codingSlot(), undefined);
  f.store.finishPause(first.id);
  assert.equal(f.store.getRun(first.id).status, 'paused');
  const next = f.store.claimCodingSlot(second.id);
  assert.ok(next.schedule?.id); assert.notEqual(next.schedule?.id, decision.schedule?.id);
  assert.throws(() => f.store.reclaimCodingSlot(first.id), new RegExp(second.taskSnapshot.key));
  f.store.savePublication({ ...record, ready: false, blockers: ['Later CI regression'] });
  f.store.setDesiredState(second.id, 'cancelled');
  f.store.reclaimCodingSlot(first.id);
  assert.equal(f.store.codingSlot()?.id, first.id);
  assert.equal(f.store.getRun(first.id).slotRelease?.candidateId, record.candidateId, 'The released observation remains durable across explicit Resume.');
});

test('QUE-03: provider-observed merge releases the slot and requests a distinct safe pause', t => {
  const f = fixture(t);
  const first = f.queue('Merged'); const second = f.queue('After merge');
  f.store.claimCodingSlot(first.id);
  const { attempt } = f.store.admitAssignment(first.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Observe.' });
  f.store.claimAttempt(attempt.id);
  const record: PublicationRecord = { id: randomUUID(), runId: first.id, attemptId: randomUUID(), candidateId: randomUUID(),
    repositoryPath: f.root, repository: { uuid: `{${randomUUID()}}`, workspace: 'fixture', slug: 'repo' }, branch: 'fixture', target: 'main',
    revision: { base: 'a'.repeat(40), head: 'b'.repeat(40), tree: 'c'.repeat(40), worktreeHash: 'd'.repeat(64), dirty: false, requirementsHash: 'e'.repeat(64), instructionsHash: 'f'.repeat(64) },
    title: 'Merged', description: 'Fixture', policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1, providerState: 'MERGED' }, ready: false, blockers: [], merged: { commit: 'b'.repeat(40), observedAt: new Date().toISOString() } };
  f.store.savePublication(record);
  assert.equal(f.store.codingSlot(), undefined);
  assert.equal(f.store.getRun(first.id).slotRelease?.reason, 'merged');
  assert.equal(f.store.getRun(first.id).pauseReason, 'merged');
  assert.equal(f.store.claimNextCodingSlot(), undefined, 'A merged release does not bypass still-claimed attempts.');
  assert.throws(() => f.store.claimCodingSlot(second.id), /unconfirmed coding process ownership/);
  f.store.recordOwnership(attempt.id, true);
  f.store.finishPause(first.id);
  assert.equal(f.store.claimCodingSlot(second.id).id, second.id);
});

test('QUE-05: order, cancellation, missing details and changed requirements linearize before scheduling', async t => {
  const f = fixture(t);
  const missing = f.queue('Missing launch', false), stale = f.queue('Stale brief'), eligible = f.queue('Eligible');
  const revised = f.store.refine(stale.taskId, { revision: 2, sourceRevision: 1, brief: 'New brief', criteria: ['New result'] });
  assert.equal(revised.revision, 3);
  assert.equal(f.store.codingQueue().plans[0].holdReason, 'Needs launch details');
  assert.equal(f.store.codingQueue().plans[1].holdReason, 'Requirements changed');
  const order = f.store.codingQueue();
  f.store.reorderQueue({ requestId: randomUUID(), expectedRevision: order.revision, runIds: [eligible.id, stale.id, missing.id] });
  const chosen = f.store.claimCodingSlot(f.store.codingQueue().plans.find(plan => !f.store.eligibilityHold(plan))!.id);
  assert.equal(chosen.id, eligible.id); assert.equal(f.store.codingQueue().plans.length, 2);
  assert.throws(() => f.store.cancelQueued(eligible.id, { requestId: randomUUID() }), /queued/);
  f.store.setDesiredState(eligible.id, 'cancelled');
  assert.equal(f.store.codingSlot(), undefined);
  assert.equal(f.store.getRun(eligible.id).slotRelease?.reason, 'cancelled');
  await assert.rejects(new RepositoryWork(f.store, join(f.root, 'worktrees')).prepare(eligible.id, f.launch.repository), /prepare repository ownership/);
  assert.equal(f.store.repositoryWork(eligible.id), undefined, 'Cancelled decision cannot reserve a worktree.');
  assert.throws(() => f.store.admitInitialTeam(eligible.id, [{ name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.', brief: { criteria: [], dependsOn: [] } }]), /stopped before initial team admission/);
  assert.equal(f.store.runtime(eligible.id).assignments.length, 0);
  assert.equal(f.store.cancelQueued(missing.id, { requestId: randomUUID() }).status, 'cancelled');
  assert.equal(f.store.codingQueue().plans[0].id, stale.id);
});

test('QUE-05: reorder and scheduler selection linearize across store connections', t => {
  const f = fixture(t);
  const first = f.queue('Before reorder'), second = f.queue('After reorder');
  const revision = f.store.codingQueue().revision;
  const operator = f.open();
  operator.reorderQueue({ requestId: randomUUID(), expectedRevision: revision, runIds: [second.id, first.id] });
  assert.equal(f.store.claimNextCodingSlot()?.id, second.id);
  assert.equal(f.store.codingQueue().plans[0].id, first.id);
  assert.throws(() => operator.reorderQueue({ requestId: randomUUID(), expectedRevision: revision, runIds: [first.id] }), /Queue order changed/);
  assert.equal(operator.codingSlot()?.id, second.id);
});

test('QUE-03/05: restarts after decision, preparation and partial admission reuse one worktree and assignment IDs', async t => {
  const f = fixture(t);
  const plan = f.queue('Restart safely');
  const original = f.store.claimCodingSlot(plan.id);
  const decisionId = original.schedule!.id;
  f.store.close(); let store = f.open();
  assert.equal(store.codingSlot()?.schedule?.id, decisionId);
  const work = new RepositoryWork(store, join(f.root, 'worktrees'));
  const prepared = await work.prepare(plan.id, f.launch.repository);
  store.close(); store = f.open();
  assert.equal((await new RepositoryWork(store, join(f.root, 'worktrees')).prepare(plan.id, f.launch.repository)).path, prepared.path);
  const team = [
    { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.', brief: { criteria: [], dependsOn: [] } },
    { name: 'Writer', role: f.role, model: f.model, coordinator: false, input: 'Write.', brief: { criteria: [], dependsOn: [] } },
  ];
  const admitted = store.admitInitialTeam(plan.id, team);
  store.setDesiredState(plan.id, 'running');
  store.close(); store = f.open();
  assert.deepEqual(store.admitInitialTeam(plan.id, team).map(item => item.assignment.id), admitted.map(item => item.assignment.id));
  assert.equal(store.runtime(plan.id).assignments.length, 2);
  store.claimAttempt(admitted[0].attempt.id);
  store.recordOwnership(admitted[0].attempt.id, true);
  store.beginPause(plan.id, 'recovery'); store.finishPause(plan.id);
  store.close(); store = f.open();
  store.restartScheduledLaunch(plan.id);
  const resumed = store.admitInitialTeam(plan.id, team);
  assert.deepEqual(resumed.map(item => item.assignment.id), admitted.map(item => item.assignment.id));
  assert.notEqual(resumed[0].attempt.id, admitted[0].attempt.id);
  assert.equal(resumed[0].attempt.generation, 2);
  assert.equal(store.runtime(plan.id).assignments.length, 2);
  assert.equal(store.runtime(plan.id).attempts.length, 3);
  assert.equal(store.codingSlot()?.id, plan.id);
  assert.equal(store.codingQueue().plans.length, 0);
});
