import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { startServer } from '../server/app';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import type { PublicationRecord } from '../shared/publication';

test('QUE-01/05: manual coding start names the durable slot holder and cannot displace it after restart', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-scheduler-http-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const queue = (title: string, withLaunch = false) => {
    const task = store.createTask({ title, description: '', repositoryId: repository.id });
    const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['One result'] });
    return store.queue(task.id, { revision: accepted.revision, requestId: randomUUID(), ...(withLaunch ? { launch: { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' } } } : {}) }, defaultRoles);
  };
  const first = queue('First'), second = queue('Second', true);
  store.claimCodingSlot(first.id, true);
  store.close();
  const server = await startServer({ port: 0, dataDir: root, serveUi: false, rolePath: join(root, 'roles.json'),
    runtimeConfig: { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [], policy: {}, models: { providers: {} } } });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const response = await fetch(`${server.origin}/api/runs/${second.id}/start`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }] }) });
  assert.equal(response.status, 409);
  assert.match((await response.json()).error, new RegExp(`${first.taskSnapshot.key} holds the coding slot`));
  const workspace = await (await fetch(`${server.origin}/api/workspace`)).json();
  assert.equal(workspace.queue.slot.runId, first.id);
  assert.equal(workspace.queue.plans.length, 1);
  assert.match(workspace.queue.plans[0].holdReason, new RegExp(first.taskSnapshot.key));
  assert.equal(workspace.runs.find((run: { id: string }) => run.id === second.id).schedule, undefined);
});
test('M6.3: Resume of readiness-released work queues durably behind the current coding slot holder', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-resume-queue-'));
  seedRepository(root);
  const path = join(root, 'workflow.sqlite');
  const store = new Workbench(path);
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const queue = (title: string) => {
    const task = store.createTask({ title, description: '', repositoryId: repository.id });
    const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['One result'] });
    return store.queue(task.id, { revision: accepted.revision, requestId: randomUUID(), launch: {
      team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' },
    } }, defaultRoles);
  };
  const released = queue('Released for review'); const holder = queue('Current holder');
  store.claimCodingSlot(released.id, true);
  const prepared = await new RepositoryWork(store, join(root, 'owned')).prepare(released.id, { mode: 'new' });
  const candidateId = randomUUID();
  const publication: PublicationRecord = { id: randomUUID(), runId: released.id, attemptId: randomUUID(), candidateId,
    repositoryPath: prepared.repository.path, repository: { uuid: `{${randomUUID()}}`, workspace: 'fixture', slug: 'repository' },
    branch: prepared.branch, target: 'main',
    revision: { base: 'a'.repeat(40), head: 'b'.repeat(40), tree: 'c'.repeat(40), worktreeHash: 'd'.repeat(64), dirty: false,
      requirementsHash: 'e'.repeat(64), instructionsHash: 'f'.repeat(64) },
    title: 'Released', description: 'Review', policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1, providerState: 'OPEN' }, ready: true, blockers: [],
    feedback: { state: 'unavailable', observedAt: '2026-10-01T00:00:00Z', reason: 'Provider unavailable', comments: [{ id: 17,
      created_on: '2026-09-30T00:00:00Z', updated_on: '2026-09-30T00:00:00Z', deleted: false,
      content: { raw: 'Preserve the recorded boundary' }, currentness: 'unknown' }] } };
  store.savePublication(publication);
  store.releaseCodingSlot(released.id, 'review', candidateId);
  store.finishPause(released.id);
  store.claimCodingSlot(holder.id, true);
  store.setDesiredState(holder.id, 'paused'); store.beginPause(holder.id, 'operator'); store.finishPause(holder.id);
  store.close();

  const start = () => startServer({ port: 0, dataDir: root, serveUi: false, rolePath: join(root, 'roles.json'),
    runtimeConfig: { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [], policy: {}, models: { providers: {} } } });
  let server = await start();
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const resume = () => fetch(`${server.origin}/api/runs/${released.id}/resume`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: '{}' });
  const workspace = async () => (await (await fetch(`${server.origin}/api/workspace`)).json());

  const response = await resume();
  assert.equal(response.status, 200);
  const first = await workspace();
  const queued = first.runs.find((run: { id: string }) => run.id === released.id);
  assert.equal(queued.status, 'paused'); assert.equal(queued.resumeRequest?.state, 'queued');
  assert.equal(first.queue.slot.runId, holder.id);
  assert.equal(queued.feedbackContinuations.length, 1);
  assert.equal(queued.feedbackContinuations[0].feedback.state, 'unavailable');

  const requestedAt = queued.resumeRequest.requestedAt;
  await server.close(); server = await start();
  const reopened = await workspace();
  const persisted = reopened.runs.find((run: { id: string }) => run.id === released.id);
  assert.equal(persisted.resumeRequest?.state, 'queued'); assert.equal(persisted.resumeRequest.requestedAt, requestedAt);
  assert.deepEqual(persisted.feedbackContinuations, queued.feedbackContinuations, 'The exact feedback input survives the controller restart.');
  assert.equal(reopened.queue.slot.runId, holder.id);
  const paused = await fetch(`${server.origin}/api/runs/${released.id}/pause`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: '{}' });
  assert.equal(paused.status, 200);
  const held = (await workspace()).runs.find((run: { id: string }) => run.id === released.id);
  assert.equal(held.resumeRequest, undefined); assert.equal(held.feedbackContinuations[0].state, 'held');
  assert.equal((await resume()).status, 200);
  const retried = (await workspace()).runs.find((run: { id: string }) => run.id === released.id);
  assert.equal(retried.feedbackContinuations.length, 1, 'The same pinned intent is reused after a newer Pause.');
  assert.equal(retried.feedbackContinuations[0].state, 'queued');
  const cancelled = await fetch(`${server.origin}/api/runs/${released.id}/cancel`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: '{}' });
  assert.equal(cancelled.status, 200);
  const afterCancel = await workspace();
  const cancelledRun = afterCancel.runs.find((run: { id: string }) => run.id === released.id);
  assert.equal(cancelledRun.status, 'cancelled'); assert.equal(cancelledRun.resumeRequest, undefined);
  assert.equal(cancelledRun.feedbackContinuations[0].feedback.comments[0].content.raw, 'Preserve the recorded boundary');
  assert.equal(cancelledRun.feedbackContinuations[0].state, 'cancelled', 'A canceled queued intent must not remain labeled queued.');
  assert.equal(afterCancel.queue.slot.runId, holder.id, 'Cancelling a queued Resume does not release another run\'s slot.');
});
