import { fixtureRepository } from './fixture-repository';
import { readFileSync } from 'node:fs';
import { startServer } from '../server/app';
import assert from 'node:assert/strict';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await startServer({ port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime });
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
};
const observedRuntime = createRuntimeObserver(request);
try {
  const repo = await request('/repositories', { name: 'Fixture', path: fixtureRepository(config.root), targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Strict wire effort', description: '', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Use the admitted effort', criteria: ['Never execute a substituted effort'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Execute with exactly the admitted model and effort.' }] });
  let state: any;
  const deadline = Date.now() + 12000;
  while (Date.now() < deadline) {
    state = await observedRuntime(run.id);
    if (state.events.some((item: any) => item.event.type === 'prompt_result')) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.ok(state.events.some((item: any) => item.event.type === 'model_policy_rejected'), 'Substituted effort must be rejected at the provider boundary.');
  assert.ok(!JSON.stringify(state.events).includes('UNAUTHORIZED_EFFORT_COMPLETED'));
  let observed;
  const stoppedBy = Date.now() + 8000;
  do { observed = (await request('/workspace')).runs[0]; await new Promise(resolve => setTimeout(resolve, 50)); } while (observed.status !== 'attention_required' && Date.now() < stoppedBy);
  assert.equal(observed.status, 'attention_required');
  assert.match(observed.attentionReason, /unapproved/);
  state = await observedRuntime(run.id);
  assert.equal(state.attempts.length, 1);
  assert.equal(state.attempts[0].ownership, 'stopped');
  console.log(JSON.stringify({ passed: true, policyFailures: state.events.filter((item: any) => item.event.type === 'model_policy_rejected') }));
} finally { await app.close(); }
