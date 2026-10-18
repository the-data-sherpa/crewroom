import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import { startServer } from '../server/app';
import { fixtureRepository } from './fixture-repository';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const options = { port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime };
let app = await startServer(options);
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
};
const observed = createRuntimeObserver(request);
try {
  const repo = await request('/repositories', { name: 'Provider failures', path: fixtureRepository(config.root), targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Provider failure boundary', description: '', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Retain failures and actual model identities', criteria: ['A response alone is not verified work'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Continue only with the assigned model or an explicit compatible fallback.' }] });
  let state: any; let current: any;
  const success = ['fallback', '429'].includes(config.fault);
  const deadline = Date.now() + 95000;
  do {
    state = await observed(run.id); current = (await request('/workspace')).runs.find((item: any) => item.id === run.id);
    if (success ? state.attempts.at(-1)?.state === 'idle' && JSON.stringify(state.events).includes(config.fault === 'fallback' ? 'FALLBACK_COMPLETE' : 'THROTTLE_RECOVERED')
      : current.status === 'attention_required' && state.attempts.every((item: any) => item.ownership === 'stopped')) break;
    await delay(100);
  } while (Date.now() < deadline);
  if (success) {
    assert.equal(current.status, 'running'); assert.equal(state.attempts.at(-1)?.state, 'idle');
    const marker = config.fault === 'fallback' ? 'FALLBACK_COMPLETE' : 'THROTTLE_RECOVERED';
    assert.ok(state.events.some((item: any) => item.attemptId === state.attempts.at(-1).id && item.event.type === 'message_end'
      && item.event.message?.role === 'assistant' && item.event.message.content?.some((part: any) => part.type === 'text' && part.text === marker)), `The current attempt never completed ${marker}.`);
  }
  else { assert.equal(current.status, 'attention_required', JSON.stringify(state.attempts)); assert.ok(state.attempts.every((item: any) => item.ownership === 'stopped')); }
  const expectedKind = ['401','403'].includes(config.fault) ? 'authorization' : config.fault === '429' ? 'rate_limit'
    : config.fault === 'empty' ? 'empty_final' : ['fallback','no-fallback','unavailable-fallback','exhausted'].includes(config.fault) ? 'unavailable' : config.fault;
  assert.ok(state.events.some((item: any) => item.event.type === 'provider_failure' && item.event.kind === expectedKind), `Missing ${expectedKind} classification.`);
  assert.equal(state.attempts.length, config.fault === 'fallback' ? 2 : config.fault === 'exhausted' ? 3 : 1);
  assert.equal(state.assignments[0].automaticCycles ?? 0, config.fault === 'fallback' ? 1 : config.fault === 'exhausted' ? 2 : 0);
  if (config.fault === 'fallback') {
    const [original, alternate] = state.attempts;
    assert.equal(original.model.resolved.selector, 'aperture-openai/openai.gpt-6-sol');
    assert.equal(alternate.model.resolved.selector, 'aperture-anthropic/anthropic.claude-sonnet-5');
    assert.equal(original.ownership, 'stopped'); assert.equal(alternate.fallback.fromAttemptId, original.id);
    assert.notEqual(original.sessionId, alternate.sessionId); assert.equal(alternate.recoveryMode, 'replacement');
    const history = (await request(`/runs/${run.id}/runtime`)).history;
    assert.deepEqual(history.usage.rows.map((row: any) => row.model), state.attempts.map((item: any) => item.model.resolved.selector));
    assert.ok(history.attempts[1].causes.some((cause: string) => cause.includes('Configured fallback')));
  }
  assert.equal((await request(`/runs/${run.id}/result`)).assessment.ready, false, 'Prompt completion alone never creates evidence or readiness.');
  assert.equal((await request(`/runs/${run.id}/result`)).candidates.length, 0);
  if (current.status === 'running') await request(`/runs/${run.id}/pause`, {});
  const before = state.attempts.map((item: any) => ({ id: item.id, model: item.model, fallback: item.fallback }));
  await app.close(); app = await startServer(options);
  assert.deepEqual((await request(`/runs/${run.id}/runtime`)).attempts.map((item: any) => ({ id: item.id, model: item.model, fallback: item.fallback })), before);
  await request(`/runs/${run.id}/cancel`, {});
  console.log(JSON.stringify({ passed: true, fault: config.fault, attempts: before, classified: expectedKind, noFalseReadiness: true }));
} finally { await app.close(); }
