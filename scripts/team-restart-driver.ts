import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { savedInputs } from '../server/runtime/inputs';
import assert from 'node:assert/strict';
import { Workbench } from '../server/workbench';
import { stopOwned } from '../server/runtime/processes';
import { resolveAssignment } from '../server/runtime/model-policy';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const runtimePath = join(config.root, 'runtime-config.json'); writeFileSync(runtimePath, JSON.stringify(config.runtime), { mode: 0o600 });
async function launch() {
  const child = spawn(process.execPath, ['--import', 'tsx', resolve('server/index.ts'), '--production'], { stdio: 'pipe', env: { ...process.env,
    CREWROOM_DATA_DIR: config.root, CREWROOM_ROLE_PATH: config.rolePath, CREWROOM_PORT: String(config.serverPort), CREWROOM_RUNTIME_CONFIG: runtimePath } });
  let log = ''; child.stdout.on('data', chunk => log += chunk); child.stderr.on('data', chunk => log += chunk);
  const deadline = Date.now() + 25000;
  while (!log.includes('Crewroom listening') && child.exitCode === null && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 30));
  assert.ok(log.includes('Crewroom listening'), log); return child;
}
let child = await launch(); const origin = `http://127.0.0.1:${config.serverPort}`;
async function request(path: string, body?: unknown) {
  const response = await fetch(origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
}
const observedRuntime = createRuntimeObserver(request);
try {
  const roles = (await request('/workspace')).roles;
  const checkout = join(config.root, 'checkout'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { stdio: 'pipe' });
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'README.md'), 'Disposable team transition fixture.\n'); git('add', 'README.md'); git('commit', '-m', 'Fixture');
  const repo = await request('/repositories', { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Interrupted team change', description: '', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Recover the accepted team change', criteria: ['No duplicate assignments or old owners'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Create a checkpoint.' }] });
  let state; const initialBy = Date.now() + 10000;
  do { state = await observedRuntime(run.id); await new Promise(resolve => setTimeout(resolve, 30)); }
  while ((!JSON.stringify(state.events).includes('CHECKPOINT_COMPLETE') || state.attempts[0].state !== 'idle') && Date.now() < initialBy);
  assert.equal(state.attempts[0].state, 'idle');
  child.kill('SIGSTOP');
  const store = new Workbench(join(config.root, 'workflow.sqlite'));
  const specialists = ['SpecialistOne', 'SpecialistTwo'].slice(0, config.teamSize).map((name, index) => ({ name, role: roles[index + 1], input: 'Assess the assignment and report to Coordinator.', model: resolveAssignment(roles[index + 1], config.runtime.catalog, config.runtime.policy, { inputs: ['text'], tools: true }) }));
  const proposal = config.operatorInsertion ? store.proposeOperatorSpecialist(run.id, { requestId: randomUUID(), roleId: specialists[0].role.id, input: specialists[0].input, dependsOn: ['Coordinator'], criteria: [0] }, specialists[0].role, specialists[0].model)
    : store.proposeTeam(state.attempts[0].id, 'crash-boundary', specialists);
  store.beginTeamTransition(proposal.id);
  if (config.teamFault === 'after-apply') {
    store.beginPause(run.id);
    for (const attempt of state.attempts) {
      const root = join(config.runtime.root, attempt.id);
      assert.equal((await stopOwned(join(root, 'host-process'))).state, 'stopped');
      assert.equal((await stopOwned(join(root, 'omp-process'))).state, 'stopped');
      store.recordOwnership(attempt.id, true);
    }
    store.finishPause(run.id); store.applyTeamProposal(proposal.id);
  }
  store.close();
  const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGKILL'); await exited;
  child = await launch();
  const deadline = Date.now() + 20000;
  do { state = await observedRuntime(run.id); if (JSON.stringify(state.events).includes('ALL_SPECIALISTS_RECONCILED')) break; await new Promise(resolve => setTimeout(resolve, 30)); } while (Date.now() < deadline);
  assert.ok(JSON.stringify(state.events).includes('ALL_SPECIALISTS_RECONCILED'));
  assert.equal(state.assignments.length, config.teamSize + 1);
  assert.equal(state.teamProposals.length, 1); assert.equal(state.teamProposals[0].status, 'applied');
  assert.equal(state.attempts.length, config.teamSize + 2);
  assert.equal(state.attempts[0].ownership, 'stopped');
  assert.equal((await request('/workspace')).runs[0].teamTransition, undefined);
  assert.ok(state.messages.every((message: any) => message.disposition === 'received'));
  if (config.operatorInsertion) {
    assert.equal(state.teamProposals[0].origin, 'operator');
    assert.equal(state.deliveryNotices.filter((notice: any) => notice.payload.kind === 'operator_specialist_proposed').length, 1);
    assert.equal(state.deliveryNotices.find((notice: any) => notice.payload.kind === 'operator_specialist_proposed').disposition, 'received');
    const coordinator = state.assignments.find((assignment: any) => assignment.coordinator);
    const notice = state.deliveryNotices.find((item: any) => item.payload.kind === 'operator_specialist_proposed');
    const delivered = savedInputs(join(config.runtime.root, coordinator.currentAttemptId)).find(input => input.id === notice.id);
    assert.ok(delivered?.message);
    assert.match(JSON.parse(delivered.message).instruction, /operator specialist proposal/i);
    assert.doesNotMatch(JSON.parse(delivered.message).instruction, /provider observation/);
    const replay = await request(`/runs/${run.id}/specialists`, proposal.request);
    assert.equal(replay.id, proposal.id);
    assert.equal((await request(`/runs/${run.id}/runtime`)).assignments.length, 2);
  }
  console.log(JSON.stringify({ passed: true, fault: config.teamFault, assignments: state.assignments.length, attempts: state.attempts.length }));
} finally {
  if (child.exitCode === null && child.signalCode === null) { child.kill('SIGCONT'); const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGTERM'); await exited; }
}
