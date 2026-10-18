import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, realpathSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-store-'));
  seedRepository(root);
  const path = join(root, 'workflow.sqlite');
  const stores: Workbench[] = [];
  const open = () => { const store = new Workbench(path); stores.push(store); return store; };
  t.after(() => { stores.forEach(store => store.close()); rmSync(root, { recursive: true, force: true }); });
  return { open, root };
}

export const imported = (siteUrl = 'https://fixture.atlassian.net', summary = 'Fix sum') => ({
  siteUrl, issues: [{ id: '100', key: 'SIM-1', fields: {
    summary, description: 'Imported requirement', status: { name: 'Backlog' },
  } }],
});

test('INT-01/04: source refresh and reopen retain site identity and independent refinements', t => {
  const { open } = fixture(t);
  const store = open();
  const task = store.importJira(imported())[0];
  store.refine(task.id, { revision: task.revision, sourceRevision: 1, brief: 'Handle zero', criteria: ['Zero plus zero is zero'] });
  store.importJira(imported('https://fixture.atlassian.net/', 'Fix arithmetic'));
  store.importJira(imported('https://another.atlassian.net'));
  store.close();
  const reopened = open().snapshot();
  assert.equal(reopened.tasks.length, 2);
  const saved = reopened.tasks.find(item => item.id === task.id)!;
  assert.equal(saved.title, 'Fix arithmetic');
  assert.equal(saved.brief, 'Handle zero');
  assert.deepEqual(saved.criteria, ['Zero plus zero is zero']);
  assert.equal(saved.sourceRevision, 2);
  assert.equal(saved.needsReacceptance, true);
  assert.equal(saved.revision, 2);
});

test('INT-06/QUE-05: explicit repository and accepted brief form one immutable queued plan across connections', t => {
  const { open, root } = fixture(t);
  const store = open();
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Handle zero', description: 'Local request', repositoryId: repository.id });
  assert.throws(() => store.queue(task.id, { revision: 1 }), /brief and.*criteria/i);
  const brief = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Handle zero', criteria: ['0 + 0 = 0'] });
  const run = store.queue(task.id, { revision: brief.revision });
  const other = open();
  assert.equal(other.queue(task.id, { revision: brief.revision }).id, run.id);
  assert.equal(other.snapshot().runs.length, 1);
  assert.equal(run.status, 'awaiting_runner');
  const changed = store.refine(task.id, { revision: brief.revision, sourceRevision: 1, brief: 'Changed after admission', criteria: ['New criterion'] });
  assert.equal(other.snapshot().runs[0].taskSnapshot.brief, 'Handle zero');
  assert.throws(() => other.queue(task.id, { revision: changed.revision }), /already queued.*earlier brief/);
  assert.throws(() => store.refine(task.id, { revision: brief.revision, sourceRevision: 1, brief: 'Stale tab', criteria: [] }), /Reload/);
  assert.equal(other.snapshot().repositories[0].path, realpathSync(root));
});

test('INT-04: accepted history survives reopen, and a queued plan adopts only by a keyed decision', t => {
  const { open, root } = fixture(t); let store = open();
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Handle zero', description: '', repositoryId: repo.id });
  const first = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Initial', criteria: ['Preserve zero'] });
  const run = store.queue(task.id, { revision: first.revision });
  const updated = store.refine(task.id, { revision: first.revision, sourceRevision: 1, brief: 'Extended', criteria: ['Preserve zero', 'Reject negatives'], decisionNote: 'Explicitly cover negative input.' });
  assert.equal(store.getRun(run.id).taskSnapshot.brief, 'Initial');
  const requestId = crypto.randomUUID(); const input = { requestId, revision: updated.revision };
  const adopted = store.adoptRequirements(run.id, input);
  assert.equal(adopted.taskSnapshot.brief, 'Extended'); assert.deepEqual(adopted.taskSnapshot.criteria, ['Preserve zero', 'Reject negatives']);
  assert.equal(adopted.adoptions?.[0].fromRevision, first.revision);
  assert.equal(store.adoptRequirements(run.id, input).adoptions?.length, 1);
  assert.throws(() => store.adoptRequirements(run.id, { ...input, reason: 'Different replay' }), /different content/);
  assert.throws(() => store.queue(task.id, { revision: first.revision }), /brief changed/i);
  store.close(); store = open();
  assert.deepEqual(store.requirementRevisions(task.id).map(item => [item.revision, item.brief, item.criteria, item.origin, item.decisionNote]), [
    [2, 'Initial', ['Preserve zero'], 'operator', undefined], [3, 'Extended', ['Preserve zero', 'Reject negatives'], 'operator', 'Explicitly cover negative input.'],
  ]);
  assert.equal(store.getRun(run.id).adoptions?.[0].requestId, requestId);
});

test('INT-04: version-16 store migrates its current refinement as the first immutable entry', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-revision-migration-')); const path = join(root, 'workflow.sqlite');
  let store = new Workbench(path);
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const task = store.createTask({ title: 'Retained brief', description: '' });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Existing brief', criteria: ['Retain input'] });
  store.close(); const old = new DatabaseSync(path);
  old.exec('DROP TABLE requirement_revisions; PRAGMA user_version=16;'); old.close();
  store = new Workbench(path);
  assert.deepEqual(store.requirementRevisions(task.id).map(item => [item.revision, item.brief, item.criteria, item.sourceRevision]), [[accepted.revision, 'Existing brief', ['Retain input'], 1]]);
  assert.equal(store.getTask(task.id).brief, 'Existing brief');
});

test('INT-04: active adoption re-briefs changed or removed criteria through a safe team proposal', t => {
  const { open, root } = fixture(t); const store = open();
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Scope', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Initial', criteria: ['Keep zero', 'Old negatives', 'Keep positives', 'Retire legacy'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate.' });
  const changed = store.admitAssignment(run.id, { name: 'Negatives', role, model, coordinator: false, input: 'Check negatives.', brief: { criteria: [{ index: 1, criterion: 'Old negatives', obligation: 'Check negatives', checkIds: [] }], dependsOn: [] } });
  const unaffected = store.admitAssignment(run.id, { name: 'Positives', role, model, coordinator: false, input: 'Check positives.', brief: { criteria: [{ index: 2, criterion: 'Keep positives', obligation: 'Check positives', checkIds: [] }], dependsOn: [] } });
  const removed = store.admitAssignment(run.id, { name: 'Legacy', role, model, coordinator: false, input: 'Check legacy.', brief: { criteria: [{ index: 3, criterion: 'Retire legacy', obligation: 'Check legacy', checkIds: [] }], dependsOn: [] } });
  store.claimAttempt(coordinator.attempt.id); store.startExecution(run.id);
  const accepted = store.refine(task.id, { revision: 2, sourceRevision: 1, brief: 'Updated', criteria: ['Keep zero', 'New negatives', 'Keep positives'] });
  const input = { requestId: crypto.randomUUID(), revision: accepted.revision };
  assert.throws(() => store.adoptRequirements(run.id, input), /requires an operator reason/);
  assert.equal(store.getRun(run.id).taskRevision, 2); assert.equal(store.steering(run.id).length, 0);
  const decision = store.adoptRequirements(run.id, { ...input, reason: 'Negative inputs changed after investigation.' });
  assert.equal(decision.taskRevision, 3); assert.equal(decision.status, 'pausing');
  assert.equal(decision.adoptions?.[0].reason, 'Negative inputs changed after investigation.');
  const message = store.steering(run.id)[0]; assert.equal(message.delivery, 'pending'); assert.equal(message.kind, 'requirements_adoption');
  assert.match(message.body, /New negatives/); assert.match(message.body, /Negative inputs changed/);
  const assignments = store.runtime(run.id).assignments;
  assert.deepEqual(assignments.find(item => item.id === changed.assignment.id)?.rebriefRequired?.criterionIndices, [1]);
  assert.deepEqual(assignments.find(item => item.id === removed.assignment.id)?.rebriefRequired?.criterionIndices, [3]);
  assert.equal(assignments.find(item => item.id === unaffected.assignment.id)?.rebriefRequired, undefined);
  assert.equal(assignments.find(item => item.id === changed.assignment.id)?.brief?.criteria[0].criterion, 'Old negatives');
  assert.equal(assignments.find(item => item.id === unaffected.assignment.id)?.brief?.criteria[0].criterion, 'Keep positives');
  assert.equal(store.adoptRequirements(run.id, { ...input, reason: 'Negative inputs changed after investigation.' }).adoptions?.length, 1);
  assert.equal(store.steering(run.id).length, 1);
  store.recordOwnership(coordinator.attempt.id, true); store.finishPause(run.id);
  const resumed = store.replaceAttempt(coordinator.assignment.id, { automatic: false });
  store.claimAttempt(resumed.id);
  store.recordAttemptIdentity(resumed.id, { hostPid: 22, ompPid: 23, sessionId: 'rebrief', hostUrl: 'http://127.0.0.1:9', pane: '%1' });
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const proposal = store.proposeTeam(resumed.id, 'rebrief-negative', [{ name: 'NewNegatives', role, model, input: 'Cover the new criterion.',
    brief: { criteria: [{ index: 1, criterion: 'New negatives', obligation: 'Test the new negatives', checkIds: [] }], dependsOn: [] } }],
  [{ name: 'Negatives', reason: 'Original criterion changed.' }, { name: 'Legacy', reason: 'Legacy criterion was removed.' }]);
  store.recordOwnership(resumed.id, true); store.beginPause(run.id); store.finishPause(run.id);
  store.applyTeamProposal(proposal.id);
  const revised = store.runtime(run.id).assignments;
  assert.ok(revised.find(item => item.id === changed.assignment.id)?.retiredAt);
  assert.ok(revised.find(item => item.id === removed.assignment.id)?.retiredAt);
  assert.equal(revised.find(item => item.id === removed.assignment.id)?.brief?.criteria[0].criterion, 'Retire legacy');
  assert.equal(revised.find(item => item.id === changed.assignment.id)?.brief?.criteria[0].criterion, 'Old negatives');
  assert.equal(revised.find(item => item.name === 'NewNegatives')?.brief?.criteria[0].criterion, 'New negatives');
});

test('INT-04/MSG-02: repository choice, notes and undelivered steering survive reopen without changing source fields', t => {
  const { open, root } = fixture(t);
  const store = open();
  const task = store.importJira(imported())[0];
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const assigned = store.assignRepository(task.id, { repositoryId: repo.id, revision: task.revision });
  const brief = store.refine(task.id, { revision: assigned.revision, sourceRevision: 1, brief: 'Handle zero', criteria: ['0 + 0 = 0'] });
  store.addNote(task.id, { mode: 'decision', body: 'Include the zero case.' });
  const run = store.queue(task.id, { revision: brief.revision });
  const message = store.steer(run.id, { body: 'Keep this change small.', requestId: 'click-1' });
  assert.equal(store.steer(run.id, { body: 'Keep this change small.', requestId: 'click-1' }).id, message.id);
  assert.throws(() => store.steer(run.id, { body: 'Changed duplicate', requestId: 'click-1' }), /different content/);
  store.close();
  const saved = open().snapshot();
  assert.equal(saved.notes[0].body, 'Include the zero case.');
  assert.equal(saved.messages.length, 1);
  assert.equal(saved.messages[0].delivery, 'pending');
  assert.equal(saved.tasks[0].description, 'Imported requirement');
  assert.equal(saved.tasks[0].repositoryId, repo.id);
});

test('INT-04: saving a stale source view cannot silently accept refreshed requirements', t => {
  const { open } = fixture(t);
  const store = open();
  const task = store.importJira(imported())[0];
  store.importJira(imported('https://fixture.atlassian.net', 'Changed upstream'));
  assert.throws(() => store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Old requirements', criteria: ['Old'] }), /Source requirements changed/);
  assert.equal(store.getTask(task.id).brief, '');
});

test('INT-05: Jira pause clears only when the run adopts the accepted current source', t => {
  const { open, root } = fixture(t); const store = open();
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.importJira(imported())[0];
  const assigned = store.assignRepository(task.id, { repositoryId: repo.id, revision: 1 });
  const first = store.refine(task.id, { revision: assigned.revision, sourceRevision: 1, brief: 'Original', criteria: ['Preserve zero'] });
  const run = store.queue(task.id, { revision: first.revision });
  store.importJira(imported('https://fixture.atlassian.net', 'New source'));
  assert.ok(store.getRun(run.id).sourceConflict);
  assert.throws(() => store.adoptRequirements(run.id, { requestId: crypto.randomUUID(), revision: first.revision + 1 }), /Accept the current source/);
  const current = store.getTask(task.id);
  const accepted = store.refine(task.id, { revision: current.revision, sourceRevision: current.sourceRevision, brief: 'Accepted source', criteria: ['Preserve zero'] });
  assert.equal(store.requirementRevisions(task.id).at(-1)?.origin, 'jira-reacceptance');
  assert.ok(store.getRun(run.id).sourceConflict, 'Saving the brief alone does not resume the run.');
  store.importJira(imported('https://fixture.atlassian.net', 'Another source'));
  assert.throws(() => store.adoptRequirements(run.id, { requestId: crypto.randomUUID(), revision: accepted.revision }), /Accept the current source/);
  const newer = store.getTask(task.id);
  const latest = store.refine(task.id, { revision: newer.revision, sourceRevision: newer.sourceRevision, brief: 'Newest source', criteria: ['Preserve zero'] });
  const adopted = store.adoptRequirements(run.id, { requestId: crypto.randomUUID(), revision: latest.revision });
  assert.equal(adopted.sourceConflict, undefined);
  assert.equal(adopted.status, 'awaiting_runner');
  assert.equal(adopted.taskSnapshot.title, 'Another source');
  assert.equal(store.requirementRevisions(task.id)[0].brief, 'Original');
});

test('LIF-01/MSG-08: admitted attempts own sender identity and durable inputs across reopen', t => {
  const { open, root } = fixture(t); const store = open();
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Message exchange', description: '', repositoryId: repository.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Exchange findings', criteria: ['Reply returns to coordinator'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[1], model: 'fixture/code', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'code', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Start here.' });
  const specialist = store.admitAssignment(run.id, { name: 'Specialist', role, model, coordinator: false, input: 'Wait for findings.' });
  store.close(); const reopened = open();
  assert.equal(reopened.runtime(run.id).assignments.length, 2);
  assert.equal(reopened.runtime(run.id).attempts[0].input, 'Start here.');
  const message = reopened.prepareMessage(coordinator.attempt.id, 'tool-call-1', { to: specialist.assignment.id, body: 'Inspect the zero case.' });
  assert.equal(reopened.prepareMessage(coordinator.attempt.id, 'tool-call-1', { to: specialist.assignment.id, body: 'Inspect the zero case.' }).id, message.id);
  assert.equal(message.from, coordinator.assignment.id);
  assert.throws(() => reopened.prepareMessage(coordinator.attempt.id, 'tool-call-2', { to: 'outside-this-run', body: 'Escape' }), /recipient/);
  assert.throws(() => reopened.prepareMessage(coordinator.attempt.id, 'tool-call-3', { to: specialist.assignment.id, from: 'forged', body: 'Escape' }), /Unrecognized key/);
  assert.equal(reopened.runtime(run.id).messages.length, 1);
});
