import { readFileSync, writeFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { resolve, join } from 'node:path';
import assert from 'node:assert/strict';
import { Workbench } from '../server/workbench';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const runtimePath = join(config.root, 'runtime-config.json');
writeFileSync(runtimePath, JSON.stringify(config.runtime), { mode: 0o600 });
async function launch() {
  const child = spawn(process.execPath, ['--import', 'tsx', resolve('server/index.ts'), '--production'], { stdio: 'pipe', env: { ...process.env,
    CREWROOM_DATA_DIR: config.root, CREWROOM_ROLE_PATH: config.rolePath, CREWROOM_PORT: String(config.serverPort), CREWROOM_RUNTIME_CONFIG: runtimePath } });
  let log = ''; child.stdout.on('data', value => log += value); child.stderr.on('data', value => log += value);
  const deadline = Date.now() + 15000;
  while (!log.includes('Crewroom listening') && child.exitCode === null && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 30));
  assert.ok(log.includes('Crewroom listening'), log);
  return child;
}
let child = await launch();
const origin = `http://127.0.0.1:${config.serverPort}`;
async function request(path: string, body?: unknown) {
  const response = await fetch(origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
}
const observedRuntime = createRuntimeObserver(request);
try {
  const repo = await request('/repositories', { name: 'Fixture', path: config.repository, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Lost input receipt', description: '', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Reconcile delivery after supervisor loss', criteria: ['One delivery, same surviving process'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Create a checkpoint.' }] });
  let state;
  const deadline = Date.now() + 10000;
  do { state = await observedRuntime(run.id); await new Promise(resolve => setTimeout(resolve, 30)); }
  while ((!JSON.stringify(state.events).includes('CHECKPOINT_COMPLETE') || state.attempts[0].state !== 'idle') && Date.now() < deadline);
  assert.equal(state.attempts[0].state, 'idle');
  // Freeze the controller and perform the public workflow/host dispatch boundary
  // without saving its response, reproducing a lost supervisor acknowledgement.
  child.kill('SIGSTOP');
  const store = new Workbench(join(config.root, 'workflow.sqlite'));
  const note = store.steer(run.id, { requestId: 'lost-receipt', body: 'Document the fixture.' });
  const attempt = state.attempts[0]; const inputId = `steering:${note.id}`;
  store.dispatchSteering(attempt.id, [note.id], inputId);
  const host = JSON.parse(readFileSync(join(config.runtime.root, attempt.id, 'host.json'), 'utf8'));
  const body = { op: 'input', id: inputId, message: JSON.stringify({ steering: note }) };
  const deliver = () => fetch(attempt.hostUrl, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${host.token}` }, body: JSON.stringify(body) });
  const deliveries = await Promise.all([deliver(), deliver()]);
  assert.ok(deliveries.every(response => response.ok), 'Concurrent identical dispatches must share one runner prompt.');
  assert.equal(store.steering(run.id)[0].delivery, 'dispatched'); store.close();
  const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGKILL'); await exited;
  child = await launch();
  let observed;
  const recoveredBy = Date.now() + 8000;
  do { observed = await request('/workspace'); await new Promise(resolve => setTimeout(resolve, 30)); }
  while (observed.messages[0].delivery !== 'received' && Date.now() < recoveredBy);
  assert.equal(observed.messages[0].delivery, 'received');
  assert.deepEqual(observed.messages[0].transitions.map((entry: { state: string }) => entry.state), ['dispatched', 'received']);
  assert.ok(observed.messages[0].transitions.every((entry: { at?: string }) => entry.at));
  state = await observedRuntime(run.id);
  assert.equal(state.attempts.length, 1); assert.equal(state.attempts[0].ompPid, attempt.ompPid);
  assert.equal(observed.runs[0].status, 'running');
  assert.ok(state.events.some((item: any) => item.event.type === 'prompt_result' && item.event.id === inputId));
  console.log(JSON.stringify({ passed: true, scenario: 'lost-receipt', ompPid: attempt.ompPid, steering: observed.messages[0] }));
} finally {
  if (child.exitCode === null && child.signalCode === null) { child.kill('SIGCONT'); const exited = new Promise(resolve => child.once('exit', resolve)); child.kill('SIGTERM'); await exited; }
}
