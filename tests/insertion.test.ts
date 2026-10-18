import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { DatabaseSync } from 'node:sqlite';
import { Workbench } from '../server/workbench';
import { insertionFixture } from './fixtures/insertion';
import { publicationFixture } from './fixtures/publication';
import { startServer } from '../server/app';

const request = (roleId = 'tech-writer') => ({ requestId: randomUUID(), roleId, input: 'Explain the result and report to Coordinator.', dependsOn: ['Coordinator'], criteria: [0] });
function stopped(store: Workbench, runId: string) {
  store.beginPause(runId); for (const attempt of store.runtime(runId).attempts) store.recordOwnership(attempt.id, true); store.finishPause(runId);
}
function running(store: Workbench, runId: string) {
  for (const assignment of store.runtime(runId).assignments.filter(item => !item.retiredAt)) {
    const old = store.getAttempt(assignment.currentAttemptId);
    const attempt = old.state === 'admitted' ? old : store.replaceAttempt(assignment.id, { automatic: false });
    store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 100, ompPid: 101, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:59999', pane: '%1' });
  }
  store.setDesiredState(runId, 'running'); store.startExecution(runId);
}

test('INS-01/02: a queued operator proposal and notice survive restart and apply exactly once with a captured brief', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-')); const f = insertionFixture(root);
  let store = f.store; t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const value = request(); const proposal = store.proposeOperatorSpecialist(f.run.id, value, f.role, f.model);
  assert.equal(proposal.origin, 'operator'); assert.equal(proposal.attemptId, undefined); assert.equal(proposal.status, 'pending');
  assert.equal(store.runtime(f.run.id).assignments.length, 0);
  assert.equal(store.deliveryNotices(f.run.id).length, 1); assert.equal(store.deliveryNotices(f.run.id)[0].disposition, 'pending');
  store.close(); store = new Workbench(f.path);
  assert.equal(store.proposeOperatorSpecialist(f.run.id, value, f.role, f.model).id, proposal.id);
  assert.throws(() => store.proposeOperatorSpecialist(f.run.id, { ...value, input: 'Different' }, f.role, f.model), /different content/);
  const team = [{ name: 'Coordinator', role: f.roles[0], model: f.model, coordinator: true, input: 'Coordinate', brief: { criteria: [], dependsOn: [] } }];
  store.admitInitialTeam(f.run.id, team); running(store, f.run.id);
  assert.throws(() => store.applyTeamProposal(proposal.id), /Pause and confirm/);
  store.beginTeamTransition(proposal.id); stopped(store, f.run.id);
  const applied = store.applyTeamProposal(proposal.id, f.roles);
  store.close(); store = new Workbench(f.path);
  assert.deepEqual(store.applyTeamProposal(proposal.id, f.roles), applied);
  assert.equal(store.runtime(f.run.id).assignments.length, 2);
  const assignment = store.runtime(f.run.id).assignments[1];
  assert.equal(assignment.role.revision, 1); assert.equal(assignment.model.resolved.selector, 'fixture/model');
  assert.deepEqual(assignment.brief, { criteria: [{ index: 0, criterion: 'Explain the result', obligation: value.input, checkIds: [] }], dependsOn: ['Coordinator'] });
  assert.equal(store.deliveryNotices(f.run.id).length, 1);
  assert.match(store.insertionBlockers(f.run.id).join(' '), /reconcile and close/);
});

test('INS-04: role, dependency, criteria, limit, pending, terminal and refinement refusals leave no partial records', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-guards-')); const f = insertionFixture(root);
  t.after(() => { f.store.close(); rmSync(root, { recursive: true, force: true }); });
  const propose = (value: unknown, role = f.role) => f.store.proposeOperatorSpecialist(f.run.id, value, role, f.model);
  assert.throws(() => propose(request(), { ...f.role, enabled: false }), /disabled/);
  assert.throws(() => propose({ ...request(), dependsOn: ['Foreign'] }), /not an assignment/);
  assert.throws(() => propose({ ...request(), criteria: [1] }), /not an accepted/);
  assert.equal(f.store.teamProposals(f.run.id).length, 0); assert.equal(f.store.deliveryNotices(f.run.id).length, 0);
  const proposal = propose(request()); assert.throws(() => propose(request()), /existing team proposal/);
  f.store.setDesiredState(f.run.id, 'cancelled'); assert.equal(f.store.teamProposals(f.run.id)[0].status, 'rejected');
  assert.throws(() => propose(request()), /has ended/); assert.equal(f.store.deliveryNotices(f.run.id).length, 1);
  assert.equal(f.store.insertionReplay(f.run.id, proposal.request!)?.id, proposal.id);
  const task = f.store.createTask({ title: 'Refine', description: '' }); const refine = f.store.queueRefinement(task.id, { revision: 1, purpose: 'explore' });
  assert.throws(() => f.store.proposeOperatorSpecialist(refine.id, request(), f.role, f.model), /Refinement/);
});

test('INS-04: queued launch changes and admission preserve insertion reservations; newly disabled roles are rejected on apply', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-reservation-')); const f = insertionFixture(root);
  t.after(() => { f.store.close(); rmSync(root, { recursive: true, force: true }); });
  const proposal = f.store.proposeOperatorSpecialist(f.run.id, request(), f.role, f.model);
  const member = (name: string, coordinator = false) => ({ name, coordinator, roleId: 'triage', input: 'Coordinate', criteria: [], dependsOn: [] });
  const launch = { repository: { mode: 'new' }, team: [member('Other', true)] };
  assert.throws(() => f.store.setQueueLaunch(f.run.id, { requestId: randomUUID(), launch }, f.roles), /Coordinator.*not an assignment/);
  const team = [member('Coordinator', true), member('One'), member('Two'), member('Three')].map(item => ({ ...item, role: f.roles[0], model: f.model, brief: { criteria: [], dependsOn: [] } }));
  assert.throws(() => f.store.admitInitialTeam(f.run.id, team), /limit includes pending/);
  assert.equal(f.store.runtime(f.run.id).assignments.length, 0);
  f.store.admitInitialTeam(f.run.id, team.slice(0, 1)); running(f.store, f.run.id); stopped(f.store, f.run.id);
  const roles = f.roles.map(role => role.id === f.role.id ? { ...role, enabled: false } : role);
  assert.equal(f.store.applyTeamProposal(proposal.id, roles).status, 'rejected');
  assert.equal(f.store.runtime(f.run.id).assignments.length, 1);
  assert.deepEqual(f.store.insertionBlockers(f.run.id), []);
});

test('INS-03: insertion holds readiness, closure retains unchanged evidence, changed code requires new checks and review', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-evidence-')); const f = await publicationFixture(root);
  t.after(() => { f.store.close(); rmSync(root, { recursive: true, force: true }); });
  assert.equal((await f.evidence.assess(f.run.id)).ready, true);
  assert.throws(() => f.store.proposeOperatorSpecialist(f.run.id, request('delivery-reporter'), f.reporter.assignment.role, f.reporter.assignment.model), /limit is three/);
  const retirement = f.store.proposeTeam(f.coordinator.attempt.id, 'free-slot', [], [{ name: 'Reporter', reason: 'Reporting finished' }]);
  stopped(f.store, f.run.id); f.store.applyTeamProposal(retirement.id); running(f.store, f.run.id);
  const role = { ...f.reporter.assignment.role, id: 'tech-writer', name: 'Technical writer' };
  const proposal = f.store.proposeOperatorSpecialist(f.run.id, request(), role, f.reporter.assignment.model);
  let assessment = await f.evidence.assess(f.run.id); assert.equal(assessment.ready, false); assert.equal(assessment.confirmedInvalidation, false);
  stopped(f.store, f.run.id); f.store.applyTeamProposal(proposal.id); running(f.store, f.run.id);
  assessment = await f.evidence.assess(f.run.id); assert.equal(assessment.ready, false); assert.equal(assessment.confirmedInvalidation, false);
  const coordinator = f.store.runtime(f.run.id).assignments.find(item => item.coordinator)!;
  const close = f.store.proposeTeam(coordinator.currentAttemptId, 'close-inserted', [], [{ name: proposal.specialists[0].name, reason: 'Reconciled the result; no code change needed.' }]);
  stopped(f.store, f.run.id); f.store.applyTeamProposal(close.id);
  assessment = await f.evidence.assess(f.run.id); assert.equal(assessment.ready, true); assert.equal(assessment.candidate?.id, f.candidate.id);
  assert.equal(f.store.codingRecord(f.run.id)!.reviews.length, 1);
  writeFileSync(join(f.owned.path, 'answer'), 'changed again'); f.git(f.owned.path, 'commit', '-am', 'Follow up');
  assert.equal((await f.evidence.assess(f.run.id)).confirmedInvalidation, true);
  const next = await f.evidence.registerCandidate(f.run.id, f.candidate.implementationAttempts); assert.notEqual(next.id, f.candidate.id);
  assessment = await f.evidence.assess(f.run.id); assert.match(assessment.blockers.join(' '), /Required check.*Independent review/);
});

test('INS-01/04: API enforces origin, validates models and replays a request after role edits', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-api-')); const f = insertionFixture(root); f.store.close();
  const app = await startServer({ port: 0, dataDir: root, serveUi: false, rolePath: f.rolePath, runtimeConfig: f.runtimeConfig });
  t.after(async () => { await app.close(); rmSync(root, { recursive: true, force: true }); });
  const value = request(); const url = `${app.origin}/api/runs/${f.run.id}/specialists`;
  const post = (body: unknown, origin = app.origin) => fetch(url, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  assert.equal((await post(value, 'https://foreign.invalid')).status, 403);
  assert.equal((await post({ ...value, roleId: 'sre' })).status, 409);
  const first = await post(value); assert.equal(first.status, 201); const saved = await first.json();
  writeFileSync(f.rolePath, JSON.stringify(f.roles.map(role => role.id === value.roleId ? { ...role, enabled: false } : role)));
  const replay = await post(value); assert.equal(replay.status, 201); assert.equal((await replay.json()).id, saved.id);
});

test('INS-02: v25 migration preserves coordinator proposal identity and idempotent replay', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-migration-')); const f = insertionFixture(root);
  let store = f.store; t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const coordinator = store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.roles[0], model: f.model, coordinator: true, input: 'Coordinate' }); running(store, f.run.id);
  const specialists = [{ name: 'ExistingProposal', role: f.role, model: f.model, input: 'Inspect' }];
  const proposal = store.proposeTeam(coordinator.attempt.id, 'old-proposal', specialists);
  store.close(); const db = new DatabaseSync(f.path);
  db.exec('DROP INDEX operator_proposal_request; PRAGMA user_version=24;'); db.close();
  store = new Workbench(f.path);
  assert.equal(JSON.stringify(store.proposeTeam(coordinator.attempt.id, 'old-proposal', specialists)), JSON.stringify(proposal));
  assert.equal(store.teamProposals(f.run.id).length, 1);
});

test('INS-04: an accepted requirements change rejects the old insertion brief before admission', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-adoption-')); const f = insertionFixture(root);
  t.after(() => { f.store.close(); rmSync(root, { recursive: true, force: true }); });
  f.store.acceptRequirements(f.run.taskId, { revision: 2, sourceRevision: 1, brief: 'Two criteria', criteria: ['One', 'Two'] });
  f.store.adoptRequirements(f.run.id, { requestId: randomUUID(), revision: 3, reason: 'Use the accepted change.' });
  const proposal = f.store.proposeOperatorSpecialist(f.run.id, { ...request(), criteria: [1] }, f.role, f.model);
  let observations = 0; const unsubscribe = f.store.onChange(() => observations++);
  assert.equal(f.store.rejectUnavailableTeamProposal(proposal.id, f.roles), undefined);
  unsubscribe(); assert.equal(observations, 0, 'An unchanged available proposal must not wake observers with empty transactions.');
  f.store.acceptRequirements(f.run.taskId, { revision: 3, sourceRevision: 1, brief: 'One criterion', criteria: ['One'] });
  f.store.adoptRequirements(f.run.id, { requestId: randomUUID(), revision: 4, reason: 'Remove the second criterion.' });
  assert.equal(f.store.teamProposals(f.run.id)[0].status, 'rejected', 'Adoption itself settles the proposal before any launch preflight.');
  assert.match(f.store.teamProposals(f.run.id)[0].reason!, /requirements changed/);
  assert.equal(f.store.deliveryNotices(f.run.id)[0].payload.specialist, undefined, 'The undelivered old brief no longer enters coordinator input.');
  assert.equal(f.store.deliveryNotices(f.run.id)[1].payload.origin, 'operator');
  assert.equal(f.store.runtime(f.run.id).assignments.length, 0);
  f.store.setQueueLaunch(f.run.id, { requestId: randomUUID(), launch: { repository: { mode: 'new' }, team: [{ name: 'Coordinator', coordinator: true, roleId: 'triage', input: 'Coordinate' }] } }, f.roles);
  const next = f.store.proposeOperatorSpecialist(f.run.id, request(), f.role, f.model);
  assert.notEqual(next.id, proposal.id);
  f.store.admitInitialTeam(f.run.id, [{ name: 'Coordinator', role: f.roles[0], model: f.model, coordinator: true, input: 'Coordinate', brief: { criteria: [], dependsOn: [] } }]);
  assert.equal(f.store.runtime(f.run.id).assignments.length, 1);
});
