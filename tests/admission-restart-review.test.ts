import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { startServer } from '../server/app';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

test('INT-04 review repair: interrupted admitted-only adoption settles to paused and permits continuation', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-admitted-restart-'));
  seedRepository(root);
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const db = join(root, 'workflow.sqlite');
  let store = new Workbench(db);
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Interrupted launch', description: '', repositoryId: repository.id });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Original scope', criteria: ['Handle FIFO'] });
  const run = store.queue(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() });
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate the brief.' });
  store.admitAssignment(run.id, { name: 'Implementer', role: { ...role, id: 'implementer' }, model, coordinator: false, input: 'Implement FIFO.', brief: { criteria: [{ index: 0, criterion: 'Handle FIFO', obligation: 'Implement FIFO.', checkIds: [] }], dependsOn: [] } });
  store.setDesiredState(run.id, 'running');
  const revised = store.refine(task.id, { revision: accepted.revision, sourceRevision: 1, brief: 'Revised scope', criteria: ['Handle priority'] });
  store.close(); // Simulated controller crash: admission saved, neither attempt claimed nor host started.
  const server = await startServer({ port: 0, dataDir: root, serveUi: false, runtimeConfig: { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } } });
  try {
    const response = await fetch(`${server.origin}/api/runs/${run.id}/requirements/adopt`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
      body: JSON.stringify({ revision: revised.revision, requestId: crypto.randomUUID(), reason: 'Switch to priority while no agent owns a process.' }) });
    assert.equal(response.status, 200);
    const adopted = await response.json();
    assert.equal(adopted.status, 'paused', 'An admitted-only run must not remain stuck in pausing.');
    assert.equal(adopted.desiredState, 'paused');
    const runtime = await (await fetch(`${server.origin}/api/runs/${run.id}/runtime`)).json();
    assert.equal(runtime.attempts.every((attempt: { state: string }) => attempt.state === 'admitted'), true);
    assert.equal(runtime.assignments.find((assignment: { name: string }) => assignment.name === 'Implementer')?.rebriefRequired?.adoptedRevision, revised.revision);
  } finally { await server.close(); }
  store = new Workbench(db);
  try {
    const requested = store.setDesiredState(run.id, 'running');
    assert.equal(store.prepareContinuation(run.id, requested.controlRevision!).status, 'awaiting_runner', 'The paused run can continue with its retained assignments.');
  } finally { store.close(); }
});

function admittedRun(root: string, jira = false) {
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = jira ? store.importJira({ siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '100', key: 'SIM-100', fields: { summary: 'Interrupted run', description: 'First source', status: { id: '1', name: 'To Do' } } }] })[0]
    : store.createTask({ title: 'Interrupted run', description: '', repositoryId: repository.id });
  if (jira) store.assignRepository(task.id, { revision: task.revision, repositoryId: repository.id });
  const accepted = store.refine(task.id, { revision: store.getTask(task.id).revision, sourceRevision: 1, brief: 'Original work', criteria: ['Handle FIFO'] });
  const run = store.queue(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() });
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate this work.' });
  store.setDesiredState(run.id, 'running');
  return { store, run, task, attempt };
}

function restart(root: string) {
  return startServer({ port: 0, dataDir: root, serveUi: false, runtimeConfig: { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } } });
}

test('LIF-03 review repair: startup settles orphaned adoption and operator pauses without changing steering or source attention', async t => {
  for (const variant of ['admitted', 'already-stopped', 'operator', 'cancelled', 'steering', 'source', 'uncertain'] as const) {
    const root = mkdtempSync(join(tmpdir(), 'crpa-'));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const { store, run, task, attempt } = admittedRun(root, variant === 'source');
    if (variant === 'admitted' || variant === 'already-stopped') {
      if (variant === 'already-stopped') { store.claimAttempt(attempt.id); store.recordOwnership(attempt.id, true); }
      const revised = store.refine(task.id, { revision: store.getTask(task.id).revision, sourceRevision: 1, brief: 'New priority', criteria: ['Handle priority'] });
      assert.equal(store.adoptRequirements(run.id, { revision: revised.revision, requestId: crypto.randomUUID(), reason: 'Change before a host starts.' }).status, 'pausing');
    } else if (variant === 'operator') { store.setDesiredState(run.id, 'paused'); store.beginPause(run.id); }
    else if (variant === 'cancelled') { store.setDesiredState(run.id, 'cancelled'); store.beginPause(run.id); }
    else if (variant === 'steering') store.steer(run.id, { requestId: 'note', body: 'Keep source intact.' });
    else if (variant === 'source') store.importJira({ siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '100', key: 'SIM-100', fields: { summary: 'Interrupted run', description: 'Changed source', status: { id: '1', name: 'To Do' } } }] });
    else { store.claimAttempt(attempt.id); store.recordAttemptIdentity(attempt.id, { hostPid: 98765, ompPid: 98766, sessionId: 'unverifiable', hostUrl: 'http://127.0.0.1:9', pane: '%1' }); store.setDesiredState(run.id, 'paused'); store.beginPause(run.id); }
    store.close();
    const server = await restart(root);
    try {
      const saved = (await (await fetch(`${server.origin}/api/workspace`)).json()).runs.find((item: { id: string }) => item.id === run.id);
      assert.equal(saved.status, variant === 'cancelled' ? 'cancelled' : variant === 'steering' ? 'awaiting_runner' : variant === 'source' || variant === 'uncertain' ? 'attention_required' : 'paused', variant);
      if (variant === 'admitted' || variant === 'already-stopped' || variant === 'operator') assert.equal(saved.desiredState, 'paused');
      if (variant === 'cancelled') assert.equal(saved.desiredState, 'cancelled');
      if (variant === 'steering') assert.equal((await (await fetch(`${server.origin}/api/workspace`)).json()).messages.find((item: { body: string }) => item.body === 'Keep source intact.')?.delivery, 'pending');
      if (variant === 'source') assert.match(saved.sourceConflict, /source|requirements/i);
      if (variant === 'uncertain') assert.equal(saved.status === 'paused', false, 'Unconfirmed ownership never becomes paused.');
    } finally { await server.close(); }
  }
});

test('INT-04 review repair: a keyed adoption replay settles a preexisting pausing run once', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-pause-replay-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const { store, run, task } = admittedRun(root);
  store.close();
  const server = await restart(root);
  try {
    const external = new Workbench(join(root, 'workflow.sqlite'));
    const revised = external.refine(task.id, { revision: external.getTask(task.id).revision, sourceRevision: 1, brief: 'New scope', criteria: ['Handle priority'] });
    const input = { revision: revised.revision, requestId: crypto.randomUUID(), reason: 'Adopt before any host claims work.' };
    assert.equal(external.adoptRequirements(run.id, input).status, 'pausing');
    external.close();
    const response = await fetch(`${server.origin}/api/runs/${run.id}/requirements/adopt`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(input) });
    assert.equal(response.status, 200);
    assert.equal((await response.json()).status, 'paused');
    const workspace = await (await fetch(`${server.origin}/api/workspace`)).json();
    const saved = workspace.runs.find((item: { id: string }) => item.id === run.id);
    assert.equal(saved.adoptions.length, 1);
    assert.equal(workspace.messages.filter((item: { kind: string }) => item.kind === 'requirements_adoption').length, 1);
  } finally { await server.close(); }
});
