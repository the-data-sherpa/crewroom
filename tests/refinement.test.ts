import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

function fixture(t: TestContext) {
  const dir = mkdtempSync(join(tmpdir(), 'crewroom-refinement-'));
  seedRepository(dir);
  const stores: Workbench[] = [];
  const open = () => { const store = new Workbench(join(dir, 'workflow.sqlite')); stores.push(store); return store; };
  t.after(() => { stores.forEach(store => store.close()); rmSync(dir, { recursive: true, force: true }); });
  return { open, dir };
}
const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
const proposal = { brief: 'Handle zero safely', criteria: ['Zero input returns zero'], decisions: ['Use exact arithmetic'], unresolvedQuestions: ['Should negative inputs be allowed?'] };

test('INT-03/08: one open run across connections, refinement then coding, keyed replay', t => {
  const { open, dir } = fixture(t); const store = open(); const other = open();
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: dir, targetBranch: 'main' });
  const task = store.createTask({ title: 'Refine before coding', description: 'Initial idea', repositoryId: repository.id });
  const requestId = crypto.randomUUID();
  const explore = store.queueRefinement(task.id, { revision: 1, purpose: 'explore', requestId });
  assert.equal(other.queueRefinement(task.id, { revision: 1, purpose: 'explore', requestId }).id, explore.id);
  assert.throws(() => other.queueRefinement(task.id, { revision: 1, purpose: 'grill', requestId: crypto.randomUUID() }), /Close the open run/);
  assert.throws(() => other.queue(task.id, { revision: 1 }), /brief and acceptance criteria/);
  assert.throws(() => other.assignRepository(task.id, { repositoryId: repository.id, revision: 1 }), /fixed for the admitted run/);
  const { attempt } = store.admitAssignment(explore.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Explore.' });
  assert.throws(() => other.admitAssignment(explore.id, { name: 'Specialist', role, model, coordinator: false, input: 'Code.' }), /one coordinator only/);
  store.claimAttempt(attempt.id); store.recordAttemptIdentity(attempt.id, { hostPid: 123, ompPid: 124, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:9', pane: '%1' }); store.startExecution(explore.id);
  assert.deepEqual(other.executingRuns(), [{ runId: explore.id, coding: false }]);
  assert.throws(() => other.closeRefinement(explore.id), /ownership must be confirmed stopped/);
  assert.throws(() => other.proposeTeam(attempt.id, 'team', [{ name: 'Specialist', role, model, input: 'Code.' }]), /cannot propose specialists/);
  store.recordOwnership(attempt.id, true); store.setDesiredState(explore.id, 'paused'); store.beginPause(explore.id); store.finishPause(explore.id);
  const closed = other.closeRefinement(explore.id); assert.equal(closed.status, 'closed');
  assert.equal(store.closeRefinement(explore.id).status, 'closed');
  const grill = store.queueRefinement(task.id, { revision: 1, purpose: 'grill', requestId: crypto.randomUUID() });
  assert.notEqual(grill.id, explore.id); other.closeRefinement(grill.id);
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Do the work', criteria: ['Complete it'] });
  const codingRequest = crypto.randomUUID(); const coding = store.queue(task.id, { revision: accepted.revision, requestId: codingRequest });
  assert.equal(other.queue(task.id, { revision: accepted.revision, requestId: codingRequest }).id, coding.id);
  assert.notEqual(coding.id, grill.id); assert.equal(store.snapshot().runs.length, 3);
  assert.throws(() => store.queueRefinement(task.id, { revision: accepted.revision, purpose: 'explore' }), /Close the open run/);
});

test('INT-03: proposal acceptance is atomic, linked, stale-safe; discard and Jira refresh retain history', t => {
  const { open } = fixture(t); let store = open();
  const task = store.importJira({ siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '123', key: 'SIM-123', fields: { summary: 'Explore', description: 'Source', status: { name: 'Backlog' } } }] })[0];
  const run = store.queueRefinement(task.id, { revision: 1, purpose: 'explore' });
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Explore.' });
  store.claimAttempt(attempt.id); store.recordAttemptIdentity(attempt.id, { hostPid: 123, ompPid: 124, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:9', pane: '%1' }); store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  assert.throws(() => store.proposeRefinement(attempt.id, 'bad', { ...proposal, criteria: [] }), /Too small/);
  const first = store.proposeRefinement(attempt.id, 'first', proposal);
  assert.deepEqual(store.snapshot().notes.filter(note => note.proposalId === first.id).map(note => note.revision), [1, 1]);
  assert.equal(store.proposeRefinement(attempt.id, 'first', proposal).id, first.id);
  assert.throws(() => store.proposeRefinement(attempt.id, 'first', { ...proposal, brief: 'different' }), /different content/);
  const accepted = store.decideRefinement(first.id, { decision: 'accept', requestId: crypto.randomUUID(), brief: 'Operator edited brief' });
  assert.equal(store.decideRefinement(first.id, { decision: 'accept', requestId: accepted.decision!.requestId, brief: 'Operator edited brief' }).id, first.id);
  assert.throws(() => store.decideRefinement(first.id, { decision: 'accept', requestId: accepted.decision!.requestId, brief: 'Different replay' }), /different content/);
  assert.equal(accepted.decision?.revision, 2); assert.equal(store.getTask(task.id).brief, 'Operator edited brief');
  assert.equal(store.getRun(run.id).taskRevision, 1);
  const { createdAt: _createdAt, ...savedRevision } = store.requirementRevisions(task.id).at(-1)!;
  assert.deepEqual(savedRevision, { taskId: task.id, revision: 2, brief: 'Operator edited brief', criteria: proposal.criteria, sourceRevision: 1, origin: 'refinement', proposalId: first.id });
  assert.deepEqual(store.snapshot().notes.filter(note => note.proposalId === first.id).map(note => [note.kind, note.body, note.revision]), [['decision', proposal.decisions[0], 2], ['unresolved_question', proposal.unresolvedQuestions[0], 2]]);
  const next = store.proposeRefinement(attempt.id, 'stale', { ...proposal, brief: 'Old snapshot' });
  assert.throws(() => store.decideRefinement(next.id, { decision: 'accept', requestId: crypto.randomUUID() }), /earlier revision/);
  assert.throws(() => store.decideRefinement(next.id, { decision: 'discard', requestId: crypto.randomUUID() }), /Discard requires a reason/);
  store.decideRefinement(next.id, { decision: 'discard', requestId: crypto.randomUUID(), reason: 'No longer needed' });
  store.recordOwnership(attempt.id, true); store.setDesiredState(run.id, 'paused'); store.beginPause(run.id); store.finishPause(run.id); store.closeRefinement(run.id);
  store.importJira({ siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '123', key: 'SIM-123', fields: { summary: 'Explore', description: 'Refreshed source', status: { name: 'In Progress' } } }] });
  store.close(); store = open();
  assert.equal(store.getTask(task.id).brief, 'Operator edited brief'); assert.equal(store.getTask(task.id).needsReacceptance, true);
  assert.equal(store.refinementProposals().length, 2);
  assert.equal(store.snapshot().notes.length, 5);
  assert.deepEqual(store.snapshot().notes.filter(note => note.proposalId === next.id).map(note => [note.body, note.revision]),
    [[proposal.decisions[0], 1], [proposal.unresolvedQuestions[0], 1], ['No longer needed', 1]]);
  assert.equal(store.getRun(run.id).status, 'closed');
});
