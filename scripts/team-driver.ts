import { fixtureRepository } from './fixture-repository';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { startServer } from '../server/app';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await startServer({ port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime });
async function request(path: string, body?: unknown) {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
}
const observedRuntime = createRuntimeObserver(request);
try {
  const roles = (await request('/workspace')).roles;
  const custom = { ...roles[2], id: 'custom-reviewer', name: 'Custom reviewer', instructions: 'Check performance using evidence, then report to the coordinator.' };
  const createdRoles = await request('/roles', custom);
  const reporter = roles.find((role: { id: string }) => role.id === 'delivery-reporter');
  await request('/roles/delivery-reporter', { ...reporter, revision: reporter.revision ?? 1, model: roles[0].model, effort: roles[0].effort, name: 'Delivery reporter', instructions: 'Report progress and evidence. Do not edit repository documentation.' });
  const repo = await request('/repositories', { name: 'Fixture', path: fixtureRepository(config.root), targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Coordinator selects specialists', description: '', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Select the required team', criteria: ['Use configured specialist models', 'Preserve assignment snapshots'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Select the specialists needed for this request.' }] });
  const deadline = Date.now() + 30000; let state;
  do { state = await observedRuntime(run.id); if (JSON.stringify(state.events).includes('ALL_SPECIALISTS_RECONCILED')) break; await new Promise(resolve => setTimeout(resolve, 40)); } while (Date.now() < deadline);
  assert.ok(JSON.stringify(state.events).includes('ALL_SPECIALISTS_RECONCILED'), JSON.stringify({ assignments: state.assignments, proposals: state.teamProposals, error: state.error }));
  assert.equal(state.assignments.length, config.teamSize + 1 + (config.rotate ? 1 : 0));
  assert.equal(state.teamProposals.length, config.rotate ? 2 : 1); assert.equal(state.teamProposals[0].status, 'applied');
  assert.equal(state.attempts[0].ownership, 'stopped');
  assert.equal(state.attempts.length, config.teamSize + 2 + (config.rotate ? 4 : 0));
  assert.equal(state.assignments[1].model.resolved.selector, 'aperture-anthropic/anthropic.claude-sonnet-5');
  assert.equal(state.messages.length, config.teamSize + (config.rotate ? 1 : 0));
  assert.ok(state.messages.every((message: any) => message.disposition === 'received'));
  // R6: briefs are snapshotted with the accepted criterion text at admission.
  assert.deepEqual(state.assignments[1].brief.criteria, [{ index: 0, criterion: 'Use configured specialist models', obligation: 'Confirm the configured specialist models are used.', checkIds: [] }]);
  if (config.teamSize >= 2) assert.deepEqual(state.assignments[2].brief.dependsOn, ['SpecialistOne']);
  if (config.rotate) {
    assert.equal(state.assignments.filter((item: any) => !item.retiredAt).length, 4);
    assert.ok(state.assignments[1].retiredAt);
    assert.equal(state.attempts.find((item: any) => item.id === state.assignments[1].currentAttemptId).ownership, 'stopped');
    assert.equal(new Set(state.assignments.map((item: any) => item.mailboxName)).size, 5);
  }
  const updatedRoles = await request(`/roles/${custom.id}`, { ...custom, revision: createdRoles.find((role: { id: string }) => role.id === custom.id).revision, instructions: 'Later user instructions.', model: roles[0].model, effort: roles[0].effort });
  if (config.teamSize >= 2) {
    const saved = (await request(`/runs/${run.id}/runtime`)).assignments[2];
    assert.equal(saved.role.instructions, custom.instructions);
    assert.equal(saved.model.resolved.selector, 'aperture-google/gemini-3.8-flash');
  }
  async function rejectedProposal(attempt: any, args: unknown, expected: number) {
    const host = JSON.parse(readFileSync(join(config.runtime.root, attempt.id, 'host.json'), 'utf8'));
    const response = await fetch(app.origin + '/internal/team-proposal', { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json', Authorization: `Bearer ${host.token}` }, body: JSON.stringify({ attemptId: attempt.id, toolCallId: 'rejected-proposal', arguments: args }) });
    assert.equal(response.status, expected);
  }
  const currentCoordinator = state.attempts.find((attempt: any) => attempt.id === state.assignments[0].currentAttemptId);
  const currentSpecialist = state.attempts.find((attempt: any) => attempt.id === state.assignments[1].currentAttemptId);
  const requested = { specialists: [{ name: 'Unauthorized', roleId: custom.id, input: 'Do not launch.' }] };
  await rejectedProposal(currentSpecialist, requested, 409);
  await rejectedProposal(state.attempts[0], requested, 409);
  await rejectedProposal(currentCoordinator, { ...requested, model: 'unapproved/model' }, 400);
  await request(`/roles/${custom.id}`, { ...custom, revision: updatedRoles.find((role: { id: string }) => role.id === custom.id).revision, enabled: false });
  await rejectedProposal(currentCoordinator, requested, 409);
  assert.equal((await request(`/runs/${run.id}/runtime`)).teamProposals.length, config.rotate ? 2 : 1);
  console.log(JSON.stringify({ passed: true, teamSize: config.teamSize, assignments: state.assignments.map((assignment: any) => ({ name: assignment.name, roleId: assignment.role.id, model: assignment.model.resolved.selector })), proposalStatus: state.teamProposals[0].status }));
} finally { await app.close(); }
