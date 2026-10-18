import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { CodingScheduler } from '../server/coding-scheduler';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { RuntimeSupervisor } from '../server/runtime/supervisor';
import { startServer } from '../server/app';
import { defaultRoles, RoleCatalog } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-review-repair-'));
  seedRepository(root);
  const path = join(root, 'workflow.sqlite');
  const stores = [new Workbench(path)];
  const store = stores[0];
  const open = () => { const next = new Workbench(path); stores.push(next); return next; };
  t.after(() => { stores.forEach(item => item.close()); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const queue = (title: string, launch = true) => {
    const task = store.createTask({ title, description: '', repositoryId: repo.id });
    const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['Accepted'] });
    return store.queue(task.id, { revision: accepted.revision, requestId: randomUUID(), ...(launch ? { launch: { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' } } } : {}) }, defaultRoles);
  };
  return { root, path, store, open, repo, role, model, queue };
}

test('REVIEW-P1 migration: two retained paused v19 coding runs reopen without fabricating a holder', t => {
  const f = fixture(t);
  const a = f.queue('Retained A'); const b = f.queue('Retained B');
  for (const run of [a, b]) {
    f.store.admitAssignment(run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Retained.' });
    f.store.setDesiredState(run.id, 'paused'); f.store.beginPause(run.id); f.store.finishPause(run.id);
  }
  f.store.close();
  const db = new DatabaseSync(f.path);
  db.exec('DROP TABLE coding_slot; DROP TABLE queue_requests; DROP TABLE queue_state; PRAGMA user_version=19;'); db.close();
  const migrated = f.open();
  assert.equal(migrated.codingSlot(), undefined);
  assert.deepEqual([migrated.getRun(a.id).status, migrated.getRun(b.id).status], ['paused', 'paused']);
  assert.equal(migrated.codingQueue().plans.length, 0);
  migrated.close();
  const reopened = f.open();
  assert.equal(reopened.codingSlot(), undefined);
  reopened.reclaimCodingSlot(a.id);
  assert.equal(reopened.codingSlot()?.id, a.id);
  assert.throws(() => reopened.reclaimCodingSlot(b.id), new RegExp(a.taskSnapshot.key));
});

test('REVIEW-P1 migration: a sole retained run holds the slot, while among several only claimed ownership wins', t => {
  for (const active of [false, true]) {
    const f = fixture(t);
    const first = f.queue(active ? 'Active A' : 'Sole A'); const second = active && f.queue('Paused B');
    for (const run of [first, ...(second ? [second] : [])]) {
      const { attempt } = f.store.admitAssignment(run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Retained.' });
      if (active && run.id === first.id) f.store.claimAttempt(attempt.id);
      f.store.setDesiredState(run.id, 'paused'); f.store.beginPause(run.id); f.store.finishPause(run.id);
    }
    f.store.close();
    const db = new DatabaseSync(f.path);
    db.exec('DROP TABLE coding_slot; DROP TABLE queue_requests; DROP TABLE queue_state; PRAGMA user_version=19;'); db.close();
    const migrated = f.open();
    assert.equal(migrated.codingSlot()?.id, first.id);
    if (second) {
      assert.equal(migrated.getRun(second.id).status, 'paused');
      assert.throws(() => migrated.reclaimCodingSlot(second.id), new RegExp(first.taskSnapshot.key));
    }
  }
});

test('REVIEW-P1 migration: prepared-only paused work resumes into manual start without inventing an agent', async t => {
  const f = fixture(t); const run = f.queue('Prepared without team', false);
  await new RepositoryWork(f.store, join(f.root, 'worktrees')).prepare(run.id, { mode: 'new' });
  f.store.setDesiredState(run.id, 'paused'); f.store.beginPause(run.id); f.store.finishPause(run.id);
  f.store.close();
  const db = new DatabaseSync(f.path);
  db.exec('DROP TABLE coding_slot; DROP TABLE queue_requests; DROP TABLE queue_state; PRAGMA user_version=19;'); db.close();
  const server = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath: join(f.root, 'roles.json'),
    runtimeConfig: { root: join(f.root, 'hosts'), socket: join(f.root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [], policy: {}, models: { providers: {} } } });
  t.after(server.close);
  const response = await fetch(`${server.origin}/api/runs/${run.id}/resume`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: '{}' });
  assert.equal(response.status, 200);
  const migrated = f.open();
  assert.equal(migrated.getRun(run.id).status, 'awaiting_runner');
  assert.equal(migrated.codingSlot()?.id, run.id);
  assert.equal(migrated.runtime(run.id).assignments.length, 0);
});

test('REVIEW-P1 scheduler: restart after a claimed initial attempt replays current admitted attempt through dispatcher', async t => {
  const f = fixture(t); const plan = f.queue('Interrupted host');
  f.store.claimCodingSlot(plan.id);
  await new RepositoryWork(f.store, join(f.root, 'worktrees')).prepare(plan.id, { mode: 'new' });
  const { attempt } = f.store.admitAssignment(plan.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  f.store.setDesiredState(plan.id, 'running'); f.store.claimAttempt(attempt.id); f.store.recordOwnership(attempt.id, true);
  f.store.beginPause(plan.id, 'recovery'); f.store.finishPause(plan.id);
  f.store.close();
  const store = f.open(); store.restartScheduledLaunch(plan.id);
  let starts = 0;
  // Only the host process boundary is replaced. Real store, worktree, evidence and scheduler reconciliation run unchanged.
  const execute = async () => { starts++; store.startExecution(plan.id); return {}; };
  const runtime = { start: execute, startScheduled: execute, busy: () => false, pause: async () => store.getRun(plan.id) } as unknown as RuntimeSupervisor;
  const work = new RepositoryWork(store, join(f.root, 'worktrees'));
  const scheduler = new CodingScheduler(store, work, new Evidence(store, work, join(f.root, 'evidence')), runtime, () => [f.role]);
  scheduler.kick(); await scheduler.idle(); await scheduler.close();
  assert.equal(starts, 1);
  assert.equal(store.getRun(plan.id).launchCompletedAt !== undefined, true);
  assert.equal(store.runtime(plan.id).assignments.length, 1);
  assert.deepEqual(store.runtime(plan.id).attempts.map(item => [item.generation, item.ownership ?? 'admitted']), [[1, 'stopped'], [2, 'admitted']]);
});

test('REVIEW-P2 manual start: invalid team does not claim a slot and corrected retry reaches admission', async t => {
  const f = fixture(t); const run = f.queue('Manual retry', false); f.store.close();
  const rolePath = join(f.root, 'roles.json');
  const roles = new RoleCatalog(rolePath);
  roles.update('coordinator', { ...f.role, revision: 1 });
  const server = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath,
    runtimeConfig: { root: join(f.root, 'hosts'), socket: join(f.root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], policy: {}, models: { providers: {} } } });
  t.after(server.close);
  const post = async (body: unknown) => fetch(`${server.origin}/api/runs/${run.id}/start`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const rejected = await post({ team: [{ name: 'Writer', roleId: 'coordinator', coordinator: false, input: 'Wrong team.' }] });
  assert.ok(rejected.status >= 400);
  let state = new Workbench(f.path);
  assert.equal(state.codingSlot(), undefined); assert.equal(state.getRun(run.id).position, 0);
  state.close();
  await post({ team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Corrected.' }] });
  state = new Workbench(f.path);
  assert.equal(state.codingSlot()?.id, run.id);
  assert.equal(state.runtime(run.id).assignments.length, 1, 'Corrected input advanced to the real admission path.');
  state.close();
});

test('REVIEW-P2 queued launch: model disappears before scheduler admission, leaving a retryable plan with an explained hold', async t => {
  const f = fixture(t); const rolePath = join(f.root, 'roles.json');
  const roles = new RoleCatalog(rolePath); roles.update('coordinator', { ...f.role, revision: 1 });
  const run = f.queue('Model became unavailable');
  roles.update('coordinator', { ...f.role, revision: 2, model: '' });
  f.store.close();
  const server = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath,
    runtimeConfig: { root: join(f.root, 'hosts'), socket: join(f.root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], policy: {}, models: { providers: {} } } });
  t.after(server.close);
  const deadline = Date.now() + 4000; let workspace;
  do {
    workspace = await (await fetch(`${server.origin}/api/workspace`)).json();
    if (workspace.queue.plans[0]?.holdReason?.includes('model')) break;
    await new Promise(resolve => setTimeout(resolve, 20));
  } while (Date.now() < deadline);
  assert.equal(workspace.queue.slot, undefined);
  assert.equal(workspace.queue.plans[0]?.id, run.id);
  assert.match(workspace.queue.plans[0].holdReason, /model|effort/i);
  assert.equal(workspace.queue.plans[0].launchHold?.source, 'preflight');
  const heldRevision = workspace.queue.revision;
  const post = await fetch(`${server.origin}/api/roles/coordinator`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ ...f.role, revision: 3 }) });
  assert.equal(post.status, 200);
  let admitted = false;
  const retryDeadline = Date.now() + 8000;
  do {
    const state = new Workbench(f.path);
    admitted = state.runtime(run.id).assignments.length === 1;
    if (admitted) {
      assert.equal(state.codingSlot()?.id, run.id);
      assert.equal(state.runtime(run.id).assignments[0].role.revision, 4, 'Queued launch resolves the role saved after it was queued.');
      assert.equal(state.runtime(run.id).assignments[0].model.resolved.selector, 'fixture/model');
    }
    state.close();
    if (!admitted) await new Promise(resolve => setTimeout(resolve, 20)); // Real HTTP scheduler signal, not a fixed delay.
  } while (!admitted && Date.now() < retryDeadline);
  assert.equal(heldRevision, (await (await fetch(`${server.origin}/api/workspace`)).json()).queue.revision - 1, 'Only actual queue admission changes order revision.');
  assert.equal(admitted, true, 'A repaired role wakes the held plan and admits its coordinator.');
});

test('REVIEW-P2: unadmitted decision restores original queue priority without a second worktree or assignments', t => {
  const f = fixture(t); const first = f.queue('Previously first'), second = f.queue('Originally second');
  f.store.claimCodingSlot(first.id);
  const restored = f.store.deferUnadmittedLaunch(first.id, 'Selected model is unavailable.');
  assert.equal(restored.schedule, undefined);
  assert.equal(f.store.codingSlot(), undefined);
  assert.deepEqual(f.store.codingQueue().plans.map(item => [item.id, item.position]), [[first.id, 0], [second.id, 1]]);
  assert.match(f.store.codingQueue().plans[0].holdReason!, /model/);
  assert.equal(f.store.repositoryWork(first.id), undefined);
  assert.equal(f.store.runtime(first.id).assignments.length, 0);
  f.store.setQueueLaunch(first.id, { requestId: randomUUID(), launch: first.launch }, defaultRoles);
  assert.equal(f.store.claimNextCodingSlot()?.id, first.id);
});

test('REVIEW-P3 queue holds name unstopped owner and the exact pause reason', t => {
  const f = fixture(t); const first = f.queue('Old owner'); const next = f.queue('Next eligible');
  f.store.claimCodingSlot(first.id);
  const { attempt } = f.store.admitAssignment(first.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Work.' });
  f.store.claimAttempt(attempt.id);
  f.store.setDesiredState(first.id, 'cancelled');
  assert.equal(f.store.codingSlot(), undefined);
  assert.match(f.store.codingQueue().plans[0].holdReason!, new RegExp(`${first.taskSnapshot.key}.*(stop|ownership)`));
  f.store.recordOwnership(attempt.id, true);
  f.store.claimCodingSlot(next.id);
  for (const [reason, label] of [['budget', 'active-time limit'], ['recovery', 'recovery'], ['requirements', 'requirements'], ['source', 'source'], ['operator', 'operator']] as const) {
    f.store.beginPause(next.id, reason);
    assert.match(f.store.codingQueue().slot!.reason, new RegExp(label));
    assert.match(f.store.codingQueue().slot!.reason, /^pausing/);
  }
  f.store.finishPause(next.id);
  assert.equal(f.store.codingQueue().slot!.reason, 'paused by operator');
});
