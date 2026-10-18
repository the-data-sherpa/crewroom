import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { RuntimeSupervisor } from '../server/runtime/supervisor';
import { inspectOwned, launchOwned, stopOwned } from '../server/runtime/processes';

const waitFor = async (check: () => boolean, label: string, timeout = 12000) => {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  assert.fail(`Timed out waiting for ${label}`);
};

test('slow and concurrent observers do not spend recovery cycles for a live owned host; confirmed exit still enters recovery', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-supervision-load-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Keep running', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Survive observation', criteria: ['Keep the owned process running'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { assignment, attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Keep working.' });
  const hostRoot = join(root, 'hosts', attempt.id);
  const hostProcess = launchOwned(join(hostRoot, 'host-process'), '/bin/sleep', ['60'], { cwd: root, env: { PATH: process.env.PATH } });
  const ticks = join(root, 'active-work');
  const ompProcess = launchOwned(join(hostRoot, 'omp-process'), '/bin/sh', ['-c', 'while :; do echo tick >> "$1"; sleep 0.1; done', 'worker', ticks], { cwd: root, env: { PATH: process.env.PATH } });
  let delay = true; let exited = false;
  const scripted = createServer(async (req, res) => {
    let body = '';
    for await (const chunk of req) body += chunk;
    const operation = JSON.parse(body).op;
    const reply = () => { if (!res.destroyed) { res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(operation === 'identity' ? { hostPid, ompPid, sessionId: 'fixture-session' } : exited ? [{ sequence: 2, event: { type: 'process_exit' } }] : [])); } };
    // This integration boundary intentionally exercises the real 3-second AbortSignal timeout.
    if (delay) setTimeout(reply, operation === 'events' ? 8200 : 3300); else reply();
  });
  await new Promise<void>(resolve => scripted.listen(0, '127.0.0.1', resolve));
  const address = scripted.address(); if (!address || typeof address === 'string') throw new Error('Scripted host has no loopback port.');
  t.after(async () => {
    await stopOwned(join(hostRoot, 'host-process')); await stopOwned(join(hostRoot, 'omp-process'));
    hostProcess.stdin.end(); ompProcess.stdin.end();
    scripted.closeAllConnections(); await new Promise<void>(resolve => scripted.close(() => resolve()));
    store.close(); rmSync(root, { recursive: true, force: true });
  });
  let hostPid: number | undefined; let ompPid: number | undefined;
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    hostPid = (await inspectOwned(join(hostRoot, 'host-process'))).owner?.pid;
    ompPid = (await inspectOwned(join(hostRoot, 'omp-process'))).owner?.pid;
    if (hostPid && ompPid) break;
    await new Promise(resolve => setTimeout(resolve, 30));
  }
  assert.ok(hostPid && ompPid, 'Both independent process owners became live.');
  store.claimAttempt(attempt.id);
  const url = `http://127.0.0.1:${address.port}/`;
  store.recordAttemptIdentity(attempt.id, { hostPid, ompPid, sessionId: 'fixture-session', hostUrl: url, pane: '%0' });
  store.recordAgentEvent(attempt.id, 1, { type: 'agent_start' });
  store.setDesiredState(run.id, 'running');
  store.startExecution(run.id);
  const config = { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused', hostPorts: [4001, 4002, 4003, 4004],
    mail: { url: `${url}mail/`, token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } };
  const supervisor = new RuntimeSupervisor(store, config, 'http://127.0.0.1:4000');
  const fixture = supervisor as unknown as { register: (host: { attemptId: string; runId: string; token: string; url: string; pane: string; cursor: number }) => void; hosts: Map<string, unknown>; stopObserver: (host: unknown) => Promise<void>; timer?: NodeJS.Timeout };
  fixture.register({ attemptId: attempt.id, runId: run.id, token: 'fixture', url, pane: '%0', cursor: 1 });
  const beforeTicks = existsSync(ticks) ? readFileSync(ticks, 'utf8').length : 0;
  let quick = 0; let slow = 0;
  const quickObserver = setInterval(() => { store.snapshot(); store.runtime(run.id); quick++; }, 25);
  const slowObserver = setInterval(() => { store.agentEventPage(run.id, 0, 2); setTimeout(() => { slow++; }, 80); }, 125);
  try { await waitFor(() => supervisor.isDegraded(attempt.id), 'host degraded after timeout'); } finally { clearInterval(quickObserver); clearInterval(slowObserver); }
  assert.ok(quick >= 10 && slow >= 5, `Both observers polled during the timeout (${quick}, ${slow}).`);
  assert.ok(readFileSync(ticks, 'utf8').length > beforeTicks, 'Owned work progressed while observers polled and host observation timed out.');
  assert.equal(store.getRun(run.id).status, 'running');
  assert.equal(store.runtime(run.id).assignments[0].automaticCycles ?? 0, 0);
  assert.equal(store.getAttempt(attempt.id).state, 'executing');
  assert.deepEqual(supervisor.snapshot(run.id).degradedAttemptIds, [attempt.id]);
  assert.equal((await inspectOwned(join(hostRoot, 'host-process'))).state, 'live');
  await fixture.stopObserver(fixture.hosts.get(attempt.id)); clearInterval(fixture.timer);
  writeFileSync(join(hostRoot, 'host.json'), JSON.stringify({ attemptId: attempt.id, backend: 'http://127.0.0.1:4000', token: 'fixture' }));
  const restarted = new RuntimeSupervisor(store, config, 'http://127.0.0.1:4000');
  await restarted.initialize();
  const restartFixture = restarted as unknown as { messages: Map<string, unknown> };
  restartFixture.messages.clear(); // Mail transport is not part of this scripted host boundary.
  assert.deepEqual(restarted.snapshot(run.id).degradedAttemptIds, [attempt.id]);
  assert.equal(store.getRun(run.id).status, 'running');
  assert.equal(store.runtime(run.id).assignments[0].automaticCycles ?? 0, 0);
  delay = false;
  await waitFor(() => !restarted.isDegraded(attempt.id), 'host observation recovered');
  assert.equal(restarted.snapshot(run.id).degradedAttemptIds.length, 0);
  assert.equal(store.getRun(run.id).status, 'running');
  assert.equal(store.getRun(run.id).desiredState, 'running');
  assert.equal(store.runtime(run.id).assignments[0].automaticCycles ?? 0, 0);
  assert.equal(assignment.currentAttemptId, attempt.id);
  exited = true;
  await stopOwned(join(hostRoot, 'host-process')); await stopOwned(join(hostRoot, 'omp-process'));
  restarted.paneExited('%0');
  await waitFor(() => store.getRun(run.id).status === 'attention_required', 'confirmed exit enters recovery');
  assert.equal(store.getRun(run.id).status, 'attention_required', 'Confirmed exit enters recovery; the fixture has no replacement mail/runner.');
  assert.equal(store.getAttempt(attempt.id).ownership, 'stopped', store.getAttempt(attempt.id).ownershipReason ?? 'No ownership reason recorded.');
});
