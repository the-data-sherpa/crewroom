import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { RuntimeSupervisor } from '../server/runtime/supervisor';
import { Messaging } from '../server/runtime/messaging';
import { defaultRoles } from '../server/roles';

test('INT-04 review repair: adoption at the registration barrier pauses admission and notifies the coordinator', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-admission-window-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Work queued for launch', description: '', repositoryId: repo.id });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Original scope', criteria: ['Handle FIFO'] });
  const run = store.queue(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() });
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const specialist = { ...role, id: 'implementer' };
  const original = Messaging.prototype.register;
  let signalEntered!: () => void; let release!: () => void;
  const entered = new Promise<void>(resolve => { signalEntered = resolve; });
  const resume = new Promise<void>(resolve => { release = resolve; });
  Messaging.prototype.register = async () => { signalEntered(); await resume; };
  const runtime = new RuntimeSupervisor(store, { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9');
  let starting: Promise<unknown> | undefined;
  try {
    starting = runtime.start(run.id, { team: [
      { name: 'Coordinator', roleId: role.id, coordinator: true, input: 'Coordinate original brief.' },
      { name: 'Implementer', roleId: specialist.id, coordinator: false, input: 'Implement original brief.', criteria: [{ index: 0, obligation: 'Implement FIFO.' }] },
    ] }, [role, specialist]);
    await entered;
    assert.equal(store.runtime(run.id).attempts.every(item => item.state === 'admitted'), true);
    assert.equal(store.getRun(run.id).desiredState, 'running');
    const revised = store.refine(task.id, { revision: accepted.revision, sourceRevision: 1, brief: 'Revised scope', criteria: ['Handle weighted priority'] });
    assert.throws(() => store.adoptRequirements(run.id, { revision: revised.revision, requestId: crypto.randomUUID() }), /reason/);
    const adopted = store.adoptRequirements(run.id, { revision: revised.revision, requestId: crypto.randomUUID(), reason: 'Change to priority before launch.' });
    assert.equal(adopted.status, 'pausing'); assert.equal(adopted.desiredState, 'paused');
    assert.equal(store.runtime(run.id).assignments.find(item => item.name === 'Implementer')?.rebriefRequired?.adoptedRevision, revised.revision);
    const notice = store.steering(run.id).find(item => item.kind === 'requirements_adoption');
    assert.ok(notice); assert.match(notice.body, /Change to priority before launch/);
    release();
    await assert.rejects(starting, /stopped during launch|Run stopped/);
    assert.equal(store.runtime(run.id).attempts.every(item => item.state === 'admitted'), true, 'No stale-brief attempt was claimed or launched.');
    assert.equal(store.getRun(run.id).status, 'paused');
  } finally {
    if (store.getRun(run.id).desiredState === 'running') store.setDesiredState(run.id, 'paused');
    release(); await starting?.catch(() => {});
    Messaging.prototype.register = original;
    await runtime.close(); store.close(); rmSync(root, { recursive: true, force: true });
  }
});
