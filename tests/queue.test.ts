import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-queue-'));
  seedRepository(root);
  const path = join(root, 'workflow.sqlite');
  let now = Date.parse('2026-09-30T12:00:00Z');
  const stores: Workbench[] = [];
  const open = () => { const store = new Workbench(path, () => ++now); stores.push(store); return store; };
  t.after(() => { stores.forEach(store => store.close()); rmSync(root, { recursive: true, force: true }); });
  const first = open();
  const repository = saveRepositoryFixture(first, { name: 'Fixture', path: root, targetBranch: 'main' });
  const ticket = (title: string) => {
    const created = first.createTask({ title, description: '', repositoryId: repository.id });
    return first.refine(created.id, { revision: created.revision, sourceRevision: 1, brief: title, criteria: ['Retain behavior'] });
  };
  const launch = (roleId = 'coordinator') => ({ team: [{ name: 'Coordinator', roleId, coordinator: true, input: 'Coordinate the accepted brief.' }], repository: { mode: 'new' as const } });
  return { root, path, first, open, ticket, launch };
}

test('QUE-05/INT: Jira rank refresh cannot reorder locally queued plans', t => {
  const { first, launch } = fixture(t);
  const batch = (ids: string[]) => ({ siteUrl: 'https://fixture.atlassian.net', issues: ids.map(id => ({ id, key: `SIM-${id}`, fields: { summary: `Ticket ${id}`, description: 'Same source', status: { name: 'Backlog' } } })) });
  const [one, two] = first.importJira(batch(['1', '2']));
  const repositoryId = first.snapshot().repositories[0].id;
  const prepare = (task: typeof one) => {
    const selected = first.assignRepository(task.id, { repositoryId, revision: 1 });
    return first.refine(task.id, { revision: selected.revision, sourceRevision: 1, brief: task.title, criteria: ['Keep behavior'] });
  };
  const a = prepare(one); const b = prepare(two);
  const firstPlan = first.queue(b.id, { revision: b.revision, requestId: randomUUID(), launch: launch() }, defaultRoles);
  const secondPlan = first.queue(a.id, { revision: a.revision, requestId: randomUUID(), launch: launch() }, defaultRoles);
  first.importJira(batch(['2', '1']));
  assert.equal(first.getTask(b.id).rank, 0);
  assert.equal(first.getTask(a.id).rank, 1);
  assert.deepEqual(first.codingQueue().plans.map(plan => plan.id), [firstPlan.id, secondPlan.id]);
  assert.deepEqual(first.codingQueue().plans.map(plan => plan.position), [0, 1]);
});

test('QUE-05: two connections race reorder, reject stale and non-permutations without writing', t => {
  const { first, open, ticket } = fixture(t);
  const a = ticket('First'); const b = ticket('Second'); const c = ticket('Third');
  const ids = [a, b, c].map(task => first.queue(task.id, { revision: task.revision, requestId: randomUUID() }).id);
  const other = open(); const original = first.codingQueue();
  const winner = other.reorderQueue({ requestId: randomUUID(), expectedRevision: original.revision, runIds: [ids[2], ids[0], ids[1]] });
  assert.equal(winner.revision, original.revision + 1);
  assert.throws(() => first.reorderQueue({ requestId: randomUUID(), expectedRevision: original.revision, runIds: ids }), /Queue order changed/);
  assert.throws(() => first.reorderQueue({ requestId: randomUUID(), expectedRevision: winner.revision, runIds: [ids[0], ids[0], ids[1]] }), /permutation/);
  assert.throws(() => first.reorderQueue({ requestId: randomUUID(), expectedRevision: winner.revision, runIds: [ids[0], ids[1]] }), /permutation/);
  assert.deepEqual(first.codingQueue(), winner);
});

test('QUE-05: queue, launch-set, order and remove replay exact results after later changes; requeue keeps one open plan', t => {
  const { first, open, ticket, launch } = fixture(t); const other = open();
  const a = ticket('First'); const b = ticket('Second');
  const queuedRequest = randomUUID(); const queuedInput = { revision: a.revision, requestId: queuedRequest, launch: launch() };
  const plan = first.queue(a.id, queuedInput, defaultRoles);
  const bare = first.queue(b.id, { revision: b.revision, requestId: randomUUID() });
  assert.deepEqual(other.queue(a.id, queuedInput, defaultRoles), plan);
  assert.throws(() => other.queue(a.id, { ...queuedInput, launch: launch('implementer') }, defaultRoles), /different content/);
  assert.equal(bare.launch, undefined);
  const setRequest = randomUUID(); const update = { requestId: setRequest, launch: { ...launch(), repository: { mode: 'continue' as const, branch: 'feature/kept', expectedHead: 'a'.repeat(40) } } };
  const saved = other.setQueueLaunch(bare.id, update, defaultRoles);
  assert.deepEqual(first.setQueueLaunch(bare.id, update, defaultRoles), saved);
  assert.throws(() => first.setQueueLaunch(bare.id, { ...update, launch: launch() }, defaultRoles), /different content/);
  const revision = first.codingQueue().revision;
  const orderInput = { requestId: randomUUID(), expectedRevision: revision, runIds: [bare.id, plan.id] };
  const ordered = first.reorderQueue(orderInput);
  assert.deepEqual(other.reorderQueue(orderInput), ordered);
  assert.throws(() => other.reorderQueue({ ...orderInput, runIds: [plan.id, bare.id] }), /different content/);
  const remove = { requestId: randomUUID() };
  const cancelled = first.cancelQueued(plan.id, remove);
  assert.equal(cancelled.status, 'cancelled'); assert.equal(cancelled.position, undefined);
  assert.deepEqual(other.cancelQueued(plan.id, remove), cancelled);
  assert.deepEqual(other.queue(a.id, queuedInput, defaultRoles), plan, 'Original queue request replays its original result after cancellation.');
  assert.equal(first.cancelQueued(plan.id, { requestId: randomUUID() }).status, 'cancelled');
  const next = other.queue(a.id, { revision: a.revision, requestId: randomUUID(), launch: launch() }, defaultRoles);
  assert.notEqual(next.id, plan.id);
  assert.deepEqual(first.snapshot().runs.filter(run => run.taskId === a.id).map(run => run.status), ['cancelled', 'awaiting_runner']);
  assert.deepEqual(first.codingQueue().plans.map(run => run.position), [0, 1]);
  assert.equal(first.codingQueue().revision, ordered.revision + 2, 'Cancel and new queue each change revision; repeat cancel does not.');
  assert.throws(() => first.queue(a.id, { revision: a.revision, requestId: randomUUID() }), /already has a different queued run/);
});

test('QUE-05: launch validation shares admission shape, checks current catalog and becomes immutable on admission', t => {
  const { first, ticket, launch } = fixture(t); const a = ticket('Validation');
  const input = { revision: a.revision, requestId: randomUUID() };
  assert.throws(() => first.queue(a.id, { ...input, launch: launch('missing') }, defaultRoles), /role is unavailable/);
  assert.throws(() => first.queue(a.id, { ...input, launch: { team: [launch().team[0], launch().team[0]] } }, defaultRoles), /distinct assignment names/);
  assert.throws(() => first.queue(a.id, { ...input, launch: { team: [{ ...launch().team[0], criteria: [{ index: -1, obligation: 'Invalid' }] }] } }, defaultRoles), /too_small|greater than or equal/i);
  assert.throws(() => first.queue(a.id, { ...input, launch: { team: [{ ...launch().team[0], criteria: [{ index: 9, obligation: 'Out of scope' }] }] } }, defaultRoles), /not an accepted criterion/);
  const run = first.queue(a.id, { ...input, launch: launch() }, defaultRoles);
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  first.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate.' });
  assert.equal(first.getRun(run.id).position, undefined);
  assert.equal(first.codingQueue().plans.length, 0);
  assert.throws(() => first.setQueueLaunch(run.id, { requestId: randomUUID(), launch: launch() }, defaultRoles), /unadmitted queued/);
});

test('QUE-05: removing an unadmitted prepared plan retains its repository work and releases its ticket', async t => {
  const { first, root, ticket } = fixture(t); const task = ticket('Prepared');
  const run = first.queue(task.id, { revision: task.revision, requestId: randomUUID() });
  const work = await new RepositoryWork(first, join(root, 'worktrees')).prepare(run.id, { mode: 'new' });
  assert.equal(work.state, 'ready');
  const cancelled = first.cancelQueued(run.id, { requestId: randomUUID() });
  assert.equal(cancelled.status, 'cancelled');
  assert.equal(first.repositoryWork(run.id)?.path, work.path);
  assert.deepEqual(first.codingQueue().plans, []);
  assert.notEqual(first.queue(task.id, { revision: task.revision, requestId: randomUUID() }).id, run.id);
});

test('QUE-05: reopen retains order and specs; v19 migration orders existing unadmitted coding by creation, not dead position', t => {
  const { first, open, path, ticket, launch } = fixture(t);
  const a = ticket('First'); const b = ticket('Second'); const c = ticket('Third');
  const old = first.queue(a.id, { revision: a.revision, requestId: randomUUID(), launch: launch() }, defaultRoles);
  const newer = first.queue(b.id, { revision: b.revision, requestId: randomUUID(), launch: launch() }, defaultRoles);
  const research = first.queueRefinement(c.id, { revision: c.revision, purpose: 'explore', requestId: randomUUID() });
  const ordered = first.reorderQueue({ requestId: randomUUID(), expectedRevision: first.codingQueue().revision, runIds: [newer.id, old.id] });
  assert.deepEqual(open().codingQueue(), ordered);
  for (const plan of open().codingQueue().plans) assert.equal(plan.launch?.repository.mode, 'new');
  first.close();
  const db = new DatabaseSync(path);
  db.exec('DROP TABLE queue_requests; DROP TABLE queue_state; PRAGMA user_version=19;');
  db.close();
  const migrated = open();
  assert.deepEqual(migrated.codingQueue().plans.map(run => run.id), [old.id, newer.id]);
  assert.equal(migrated.codingQueue().revision, 0);
  assert.equal(migrated.getRun(old.id).launch?.team[0].roleId, 'coordinator');
  assert.equal(migrated.getRun(research.id).position, undefined);
  assert.deepEqual(open().codingQueue(), migrated.codingQueue());
});

test('QUE-05: two connections serialize queue/cancel/requeue without duplicate open runs', t => {
  const { first, open, ticket } = fixture(t); const other = open(); const task = ticket('Same ticket');
  const one = first.queue(task.id, { revision: task.revision, requestId: randomUUID() });
  assert.throws(() => other.queue(task.id, { revision: task.revision, requestId: randomUUID() }), /different queued run/);
  other.cancelQueued(one.id, { requestId: randomUUID() });
  const two = first.queue(task.id, { revision: task.revision, requestId: randomUUID() });
  assert.throws(() => other.queue(task.id, { revision: task.revision, requestId: randomUUID() }), /different queued run/);
  assert.equal(first.snapshot().runs.filter(run => run.taskId === task.id && run.status !== 'cancelled').length, 1);
  assert.equal(first.codingQueue().plans[0].id, two.id);
});
