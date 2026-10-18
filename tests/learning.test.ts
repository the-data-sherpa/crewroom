import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { RoleCatalog } from '../server/roles';
import { Learning } from '../server/learning';
import { resolveAssignment } from '../server/runtime/model-policy';

function fixture(t: { after: (fn: () => void) => void }) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-learning-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db')); const roles = new RoleCatalog(join(root, 'roles.json'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const catalog = [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }];
  const run = (title: string) => {
    const task = store.createTask({ title, description: '', repositoryId: repo.id });
    store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['Done'] });
    const queued = store.queue(task.id, { revision: 2 });
    const role = { ...roles.read().find(item => item.id === 'implementer')!, model: 'fixture/model', effort: 'low' as const };
    const coordinator = store.admitAssignment(queued.id, { name: 'Coordinator', role, model: resolveAssignment(role, catalog, {}, {}), coordinator: true, input: 'Coordinate' });
    const steering = store.steer(queued.id, { requestId: randomUUID(), body: 'Record the image correction.' });
    return { run: queued, coordinator, steering };
  };
  return { store, roles, repo, learning: new Learning(store, roles), run };
}
const decision = (value: 'approve' | 'reject', reason = 'Evidence reviewed.') => ({ decision: value, reason, requestId: randomUUID() });

test('R8: a role proposal must cite recorded evidence; approval versions the role for future assignments only', t => {
  const f = fixture(t); const first = f.run('First');
  const evidence = [{ kind: 'steering' as const, id: first.steering.id }];
  const change = 'Confirm the check image before running browser checks.';
  assert.throws(() => f.learning.propose(first.coordinator.attempt.id, 'bad', { target: { kind: 'role', roleId: 'implementer' }, change, rationale: 'x', evidence: [{ kind: 'check', id: 'missing' }] }), /not recorded/);
  assert.throws(() => f.learning.propose(first.coordinator.attempt.id, 'noop', { target: { kind: 'role', roleId: 'implementer' }, change: f.roles.read().find(item => item.id === 'implementer')!.instructions, rationale: 'x', evidence }), /does not change/);
  assert.throws(() => f.learning.propose(first.coordinator.attempt.id, 'model', { target: { kind: 'role', roleId: 'implementer' }, change, rationale: 'x', evidence, model: 'other/model' }));
  const proposal = f.learning.propose(first.coordinator.attempt.id, 'role-1', { target: { kind: 'role', roleId: 'implementer' }, change, rationale: 'Two image corrections were needed.', evidence });
  assert.equal(proposal.baseVersion, 1);
  assert.equal(f.learning.propose(first.coordinator.attempt.id, 'role-1', { target: { kind: 'role', roleId: 'implementer' }, change, rationale: 'Two image corrections were needed.', evidence }).id, proposal.id, 'A replayed tool call returns the saved proposal.');
  assert.equal(f.roles.read().find(item => item.id === 'implementer')!.instructions === change, false, 'Nothing changes before the operator decides.');

  const before = f.roles.read().find(item => item.id === 'implementer')!;
  const approval = decision('approve');
  const approved = f.learning.decide(proposal.id, approval);
  const after = f.roles.read().find(item => item.id === 'implementer')!;
  assert.equal(approved.status, 'approved'); assert.equal(approved.decision?.appliedVersion, 2);
  const unchanged = ({ id, name, category, purpose, model, effort, enabled }: typeof before) => ({ id, name, category, purpose, model, effort, enabled });
  assert.deepEqual(unchanged(after), unchanged(before), 'Only the instructions and revision change; model, effort and enabled state stay.');
  assert.equal(after.instructions, change); assert.equal(after.revision, 2);
  assert.equal(f.learning.decide(proposal.id, approval).id, proposal.id, 'Repeating the same decision is idempotent.');
  assert.throws(() => f.learning.decide(proposal.id, decision('reject')), /already been decided/);
  assert.equal(f.store.runtime(first.run.id).assignments[0].role.instructions, before.instructions, 'An admitted assignment keeps its role snapshot.');

  // A proposal against an older revision cannot overwrite a later change.
  const stale = f.learning.propose(first.coordinator.attempt.id, 'role-2', { target: { kind: 'role', roleId: 'implementer' }, change: 'Stale text.', rationale: 'x', evidence });
  f.roles.update('implementer', { ...f.roles.read().find(item => item.id === 'implementer')!, revision: 2, instructions: 'Operator edit.' });
  assert.throws(() => f.learning.decide(stale.id, decision('approve')), /changed since this proposal/);
  assert.equal(f.learning.decide(stale.id, decision('reject', 'Superseded by an operator edit.')).status, 'rejected');
});

test('R8: an approval is durable before the role is saved, and an interrupted save completes exactly once', t => {
  const f = fixture(t); const { coordinator, steering } = f.run('Interrupted');
  const evidence = [{ kind: 'steering' as const, id: steering.id }];
  const change = 'Read retained check output before repairing.';
  const propose = (call: string) => f.learning.propose(coordinator.attempt.id, call, { target: { kind: 'role', roleId: 'reviewer' }, change, rationale: 'x', evidence });
  const reviewer = () => f.roles.read().find(item => item.id === 'reviewer')!;
  // Rejecting a pending proposal never leaves its text in force: nothing was saved before approval.
  const rejected = propose('rejected');
  assert.equal(f.learning.decide(rejected.id, decision('reject')).status, 'rejected'); assert.notEqual(reviewer().instructions, change);
  // Crash after the approval was recorded, before the role save: startup saves it once.
  const first = propose('first'); const twin = propose('twin');
  f.store.decideLearning(first.id, { requestId: randomUUID(), reason: 'ok', status: 'approved', pendingRoleSave: true });
  assert.notEqual(reviewer().instructions, change, 'The recorded approval precedes the role change.');
  f.learning.reconcile();
  assert.equal(reviewer().instructions, change); assert.equal(reviewer().revision, 2); assert.equal(reviewer().learningProposalId, first.id);
  assert.equal(f.store.getLearningProposal(first.id).decision?.appliedVersion, 2);
  // Crash after the role save, before completion: the save is recognized by its proposal, not repeated.
  const second = f.learning.propose(coordinator.attempt.id, 'second', { target: { kind: 'role', roleId: 'reviewer' }, change: 'Second change.', rationale: 'x', evidence });
  f.store.decideLearning(second.id, { requestId: randomUUID(), reason: 'ok', status: 'approved', pendingRoleSave: true });
  f.roles.update('reviewer', { ...reviewer(), revision: 2, instructions: 'Second change.' }, { learningProposalId: second.id });
  f.learning.reconcile();
  assert.equal(reviewer().revision, 3); assert.equal(f.store.getLearningProposal(second.id).decision?.appliedVersion, 3);
  // An identical proposal against the same base cannot claim another proposal's save, and an operator edit clears the provenance.
  assert.throws(() => f.learning.decide(twin.id, decision('approve')), /changed since this proposal/);
  f.roles.update('reviewer', { ...reviewer(), learningProposalId: twin.id } as never);
  assert.equal(reviewer().learningProposalId, undefined, 'Operators cannot set approval provenance.');
  // A role changed while an approval was interrupted is recorded as not applied rather than overwritten.
  const third = f.learning.propose(coordinator.attempt.id, 'third', { target: { kind: 'role', roleId: 'reviewer' }, change: 'Third change.', rationale: 'x', evidence });
  f.store.decideLearning(third.id, { requestId: randomUUID(), reason: 'ok', status: 'approved', pendingRoleSave: true });
  f.roles.update('reviewer', { ...reviewer(), revision: 4, instructions: 'Manual edit.' });
  f.learning.reconcile();
  assert.equal(f.store.getLearningProposal(third.id).status, 'superseded'); assert.equal(reviewer().instructions, 'Manual edit.');
});

test('ROL-02: a competing save after learning reads its base supersedes approval without overwriting', t => {
  const f = fixture(t); const { coordinator, steering } = f.run('Concurrent approval');
  const proposal = f.learning.propose(coordinator.attempt.id, 'race', { target: { kind: 'role', roleId: 'reviewer' }, change: 'Proposed instructions.', rationale: 'x', evidence: [{ kind: 'steering', id: steering.id }] });
  const original = f.roles.update.bind(f.roles);
  f.roles.update = (...args: Parameters<typeof original>) => {
    const current = f.roles.read().find(item => item.id === 'reviewer')!;
    original('reviewer', { ...current, revision: current.revision ?? 1, instructions: 'Competing operator instructions.' });
    return original(...args);
  };
  const decided = f.learning.decide(proposal.id, decision('approve'));
  assert.equal(decided.status, 'superseded');
  assert.match(decided.decision?.notApplied ?? '', /role changed before/);
  assert.equal(f.roles.read().find(item => item.id === 'reviewer')!.instructions, 'Competing operator instructions.');
  assert.equal(f.roles.read().find(item => item.id === 'reviewer')!.revision, 2);
  f.learning.reconcile();
  assert.equal(f.roles.read().find(item => item.id === 'reviewer')!.revision, 2);
});

test('R8: an approval whose role save failed is retried while the server keeps running', t => {
  const f = fixture(t); const { coordinator, steering } = f.run('Transient');
  const proposal = f.learning.propose(coordinator.attempt.id, 'call', { target: { kind: 'role', roleId: 'reviewer' }, change: 'Retry me.', rationale: 'x', evidence: [{ kind: 'steering', id: steering.id }] });
  const save = f.roles.update.bind(f.roles); let fail = true;
  f.roles.update = (...args: Parameters<typeof save>) => { if (fail) throw new Error('disk full'); return save(...args); };
  assert.throws(() => f.learning.decide(proposal.id, decision('approve')), /disk full/);
  assert.equal(f.store.getLearningProposal(proposal.id).decision?.pendingRoleSave, true);
  fail = false;
  const retried = f.learning.decide(proposal.id, decision('approve'));
  assert.equal(retried.decision?.appliedVersion, 2); assert.equal(f.roles.read().find(item => item.id === 'reviewer')!.instructions, 'Retry me.');
});

test('R8: replaying a proposal tool call returns the saved proposal even after the target changed', t => {
  const f = fixture(t); const { coordinator, steering } = f.run('Replay');
  const input = { target: { kind: 'role', roleId: 'implementer' }, change: 'Cite the check receipt in every handoff.', rationale: 'x', evidence: [{ kind: 'steering', id: steering.id }] };
  const proposal = f.learning.propose(coordinator.attempt.id, 'call', input);
  f.learning.decide(proposal.id, decision('approve'));
  assert.equal(f.learning.propose(coordinator.attempt.id, 'call', input).id, proposal.id);
  assert.throws(() => f.learning.propose(coordinator.attempt.id, 'call', { ...input, change: 'Other text.' }), /different content/);
});

test('R8: approved project guidance is versioned and snapshotted into new assignments only', t => {
  const f = fixture(t); const first = f.run('First');
  const evidence = [{ kind: 'steering' as const, id: first.steering.id }];
  const proposal = f.learning.propose(first.coordinator.attempt.id, 'project-1', { target: { kind: 'project' }, change: 'Browser checks need the corrected image.', rationale: 'x', evidence });
  assert.throws(() => f.learning.propose(first.coordinator.attempt.id, 'role-target', { target: { kind: 'project', roleId: 'implementer' }, change: 'y', rationale: 'x', evidence }));
  assert.equal(proposal.baseVersion, 0);
  const competing = f.learning.propose(first.coordinator.attempt.id, 'project-2', { target: { kind: 'project' }, change: 'Other guidance.', rationale: 'x', evidence });
  assert.equal(f.learning.decide(proposal.id, decision('approve')).decision?.appliedVersion, 1);
  assert.throws(() => f.learning.decide(competing.id, decision('approve')), /changed since this proposal/);
  assert.deepEqual(f.store.snapshot().repositories[0].guidance?.map(item => [item.version, item.text]), [[1, 'Browser checks need the corrected image.']]);
  assert.equal(f.store.runtime(first.run.id).assignments[0].projectGuidance, undefined, 'The earlier assignment keeps what it was given.');
  const second = f.run('Second');
  assert.deepEqual(f.store.runtime(second.run.id).assignments[0].projectGuidance, { version: 1, text: 'Browser checks need the corrected image.' });
});
