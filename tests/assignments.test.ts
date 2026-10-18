import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles, RoleCatalog } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

test('ROL-02/03/06: the coordinator proposes a bounded team and insertion snapshots roles after ownership stops', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-assignment-'));
  seedRepository(root);
  let store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Choose a team', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Select specialists by need', criteria: ['Stable assignment policy'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Choose the required specialists.' });
  store.claimAttempt(coordinator.attempt.id);
  store.recordAttemptIdentity(coordinator.attempt.id, { hostPid: 1, ompPid: 2, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:4000', pane: '%0' });
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const specialists = [{ name: 'Implementer', role: { ...role, id: 'implementer', instructions: 'Original implementation contract.' }, model, input: 'Implement the accepted brief.' },
    { name: 'Reviewer', role: { ...role, id: 'reviewer' }, model, input: 'Independently review the revision.' }];
  const proposal = store.proposeTeam(coordinator.attempt.id, 'call-1', specialists);
  assert.equal(proposal.status, 'pending');
  assert.equal(store.proposeTeam(coordinator.attempt.id, 'call-1', specialists).id, proposal.id);
  assert.equal(store.runtime(run.id).assignments.length, 1, 'A proposal is not executing work.');
  assert.throws(() => store.proposeTeam(coordinator.attempt.id, 'call-2', [{ ...specialists[0], name: 'Third' }, { ...specialists[1], name: 'Fourth' }]), /limit/i);
  assert.throws(() => store.applyTeamProposal(proposal.id), /pause|ownership/i);
  store.beginTeamTransition(proposal.id);
  specialists[0].role.instructions = 'Later user edit';
  store.beginPause(run.id); store.recordOwnership(coordinator.attempt.id, true); store.finishPause(run.id);
  store.applyTeamProposal(proposal.id);
  store.close(); store = new Workbench(join(root, 'db'));
  assert.equal(store.getRun(run.id).teamTransition?.proposalId, proposal.id);
  const result = store.applyTeamProposal(proposal.id);
  assert.equal(result.status, 'applied');
  const team = store.runtime(run.id);
  assert.equal(team.assignments.length, 3);
  assert.equal(team.assignments[1].role.instructions, 'Original implementation contract.');
  assert.ok(team.attempts.slice(1).every(attempt => attempt.state === 'admitted'));
  assert.throws(() => store.proposeTeam(team.attempts[1].id, 'forged-proposal', [{ ...specialists[0], name: 'Another' }]), /coordinator/i);
  const activate = () => {
    const active = store.runtime(run.id).assignments.filter(item => !item.retiredAt);
    for (const item of active) if (store.getAttempt(item.currentAttemptId).state !== 'admitted') store.replaceAttempt(item.id, { automatic: false });
    store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
    for (const [i, item] of store.runtime(run.id).assignments.filter(item => !item.retiredAt).entries()) {
      store.claimAttempt(item.currentAttemptId); store.recordAttemptIdentity(item.currentAttemptId, { hostPid: i + 1, ompPid: i + 10, sessionId: item.currentAttemptId, hostUrl: `http://127.0.0.1:${7000 + i}`, pane: `%${i}` });
    }
    store.startExecution(run.id);
    return store.runtime(run.id).assignments.find(item => item.coordinator)!.currentAttemptId;
  };
  const activeCoordinator = activate();
  const rotation = store.proposeTeam(activeCoordinator, 'rotate', [{ ...specialists[0], name: 'UX' }, { ...specialists[1], name: 'Reporter' }], [{ name: 'Implementer', reason: 'Assessment completed; free this active slot.' }]);
  assert.equal(store.runtime(run.id).assignments.filter(item => !item.retiredAt).length, 3);
  store.beginPause(run.id); for (const attempt of store.runtime(run.id).attempts) store.recordOwnership(attempt.id, true); store.finishPause(run.id);
  store.applyTeamProposal(rotation.id);
  assert.equal(store.runtime(run.id).assignments.length, 5);
  assert.equal(store.runtime(run.id).assignments.filter(item => !item.retiredAt).length, 4);
  assert.equal(store.runtime(run.id).assignments[1].retirementReason, 'Assessment completed; free this active slot.');
  const nextCoordinator = activate();
  assert.throws(() => store.prepareMessage(nextCoordinator, 'retired-recipient', { to: 'Implementer', body: 'Do not deliver to retired work.' }), /recipient|retired/i);

});

test('ROL-03/04: admitted snapshots retain revision/model while later admission uses edits and disabled roles fail', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-role-snapshots-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const roles = new RoleCatalog(join(root, 'roles.json'));
  const catalog = [
    { provider: 'fixture', id: 'first', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true },
    { provider: 'fixture', id: 'second', api: 'openai-responses', inputs: ['text'], efforts: ['high'], tools: true },
  ];
  const queue = (name: string) => {
    const task = store.createTask({ title: name, description: '', repositoryId: repository.id });
    store.refine(task.id, { revision: 1, sourceRevision: 1, brief: name, criteria: ['Done'] });
    return store.queue(task.id, { revision: 2 });
  };
  const first = queue('First admission');
  const second = queue('Later admission');
  const initial = roles.read().find(role => role.id === 'tech-writer')!;
  roles.update(initial.id, { ...initial, revision: 1, model: 'fixture/first', effort: 'low', instructions: 'Original instructions.' });
  const admit = (runId: string) => {
    const role = roles.read().find(item => item.id === initial.id)!;
    return store.admitAssignment(runId, { name: 'Writer', role, model: resolveAssignment(role, catalog, {}, {}), coordinator: false, input: 'Write.' }).assignment;
  };
  const snapshot = admit(first.id);
  assert.equal(snapshot.role.revision, 2);
  assert.equal(snapshot.model.resolved.selector, 'fixture/first');
  roles.update(initial.id, { ...roles.read().find(role => role.id === initial.id)!, model: 'fixture/second', effort: 'high', instructions: 'Updated instructions.' });
  const later = admit(second.id);
  assert.equal(later.role.revision, 3);
  assert.equal(later.model.resolved.selector, 'fixture/second');
  assert.equal(later.model.resolved.effort, 'high');
  assert.equal(later.role.instructions, 'Updated instructions.');
  assert.equal(store.runtime(first.id).assignments[0].role.instructions, 'Original instructions.');
  assert.equal(store.runtime(first.id).assignments[0].role.revision, 2);
  assert.equal(store.runtime(first.id).assignments[0].model.resolved.selector, 'fixture/first');
  roles.update(initial.id, { ...roles.read().find(role => role.id === initial.id)!, enabled: false });
  const disabledRun = queue('Disabled admission');
  const disabled = roles.read().find(role => role.id === initial.id)!;
  assert.throws(() => resolveAssignment(disabled, catalog, {}, {}), /Role is disabled/);
  assert.throws(() => store.admitAssignment(disabledRun.id, { name: 'Writer', role: disabled, model: later.model, coordinator: false, input: 'Write.' }), /Disabled roles cannot receive new assignments/);
  assert.equal(store.runtime(disabledRun.id).assignments.length, 0);
  assert.equal(store.runtime(first.id).assignments[0].role.enabled, true);
});

test('ROL-04: disabling a pending role rejects the team once without stopping its run or blocking the next proposal', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-team-disabled-'));
  seedRepository(root);
  let store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Pending team', description: '', repositoryId: repository.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Choose available specialists.', criteria: ['Allowed roles only'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate.' });
  store.claimAttempt(attempt.id);
  store.recordAttemptIdentity(attempt.id, { hostPid: 1, ompPid: 2, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:4000', pane: '%0' });
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const specialist = { name: 'Writer', role: { ...role, id: 'tech-writer' }, model, input: 'Write.' };
  const pending = store.proposeTeam(attempt.id, 'disabled-role', [specialist]);
  const rejected = store.rejectUnavailableTeamProposal(pending.id, [{ ...specialist.role, enabled: false }]);
  assert.equal(rejected?.status, 'rejected'); assert.match(rejected?.reason ?? '', /disabled or unavailable/);
  assert.equal(store.getRun(run.id).status, 'running');
  assert.equal(store.runtime(run.id).assignments.length, 1);
  const notices = store.deliveryNotices(run.id);
  assert.equal(notices.length, 1); assert.equal(notices[0].payload.subjectId, pending.id);
  assert.equal(notices[0].payload.kind, 'team_proposal_rejected');
  store.dispatchDeliveryNotice(attempt.id, notices[0].id);
  store.dispatchDeliveryNotice(attempt.id, notices[0].id, true);
  assert.equal(store.deliveryNotices(run.id)[0].disposition, 'received');
  store.close(); store = new Workbench(join(root, 'db'));
  assert.equal(store.applyTeamProposal(pending.id, [{ ...specialist.role, enabled: true }]).status, 'rejected', 'Restart cannot admit the rejected team.');
  assert.equal(store.rejectUnavailableTeamProposal(pending.id, [{ ...specialist.role, enabled: false }])?.id, pending.id);
  assert.equal(store.deliveryNotices(run.id).length, 1, 'Replay does not duplicate the coordinator notice.');
  const replacement = store.proposeTeam(attempt.id, 'available-role', [{ ...specialist, name: 'Replacement' }]);
  assert.equal(replacement.status, 'pending');
  assert.equal(store.runtime(run.id).assignments.length, 1);
});
