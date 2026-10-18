import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { DatabaseSync } from 'node:sqlite';
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, watch, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { startServer } from '../server/app';
import { CodingScheduler } from '../server/coding-scheduler';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { RuntimeSupervisor } from '../server/runtime/supervisor';
import { inspectOwned, launchOwned, stopOwned } from '../server/runtime/processes';
import { defaultRoles, RoleCatalog } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import type { Role, Run, Workspace } from '../shared/types';
import type { PublicationRecord } from '../shared/publication';
import type { AssignmentModel } from '../server/runtime/model-policy';

/** Node 26 supports this API; the project's older TS lib does not yet declare it. */
function deferred<T>() {
  const constructor = Promise as PromiseConstructor & { withResolvers<U>(): { promise: Promise<U>; resolve(value: U | PromiseLike<U>): void; reject(reason?: unknown): void } };
  return constructor.withResolvers<T>();
}

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-launch-hold-'));
  const checkout = join(root, 'checkout'); mkdirSync(checkout); seedRepository(checkout);
  const path = join(root, 'workflow.sqlite');
  let clock = Date.parse('2026-09-30T12:00:00Z');
  const stores = [new Workbench(path, () => ++clock)];
  const store = stores[0];
  const open = () => { const next = new Workbench(path, () => ++clock); stores.push(next); return next; };
  t.after(() => { stores.forEach(item => item.close()); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const task = store.createTask({ title: 'Launch once', description: '', repositoryId: repo.id });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Launch once', criteria: ['One execution'] });
  const launch = { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' as const } };
  const run = store.queue(task.id, { revision: accepted.revision, requestId: randomUUID(), launch }, defaultRoles);
  const work = new RepositoryWork(store, join(root, 'worktrees'));
  const evidence = new Evidence(store, work, join(root, 'artifacts'));
  let starts = 0;

  const execute = async () => {
    starts++;
    store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate.' });
    store.startExecution(run.id);
    return {};
  };
  const runtime = { preflightStart: () => {}, start: execute, startScheduled: execute, pause: async () => store.getRun(run.id), busy: () => false } as unknown as RuntimeSupervisor;
  const scheduler = new CodingScheduler(store, work, evidence, runtime, () => [role]);
  t.after(async () => { await scheduler.close(); });
  return { root, checkout, path, store, open, repo, run, launch, role, model, work, scheduler, starts: () => starts };
}

async function mailbox(t: TestContext, holdRegistration = false) {
  const entered = deferred<void>(); const release = deferred<void>();
  if (!holdRegistration) release.resolve();
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    const value = JSON.parse(raw);
    if (value.method === 'notifications/initialized') { res.end(); return; }
    const tool = value.params?.name;
    if (tool === 'ensure_project') { entered.resolve(); await release.promise; }
    const content = tool === 'register_agent' ? { name: value.params.arguments.name }
      : tool === 'fetch_inbox_events' ? { events: [], next_cursor: 0, has_more: false } : {};
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify({ jsonrpc: '2.0', id: value.id, result: tool ? { structuredContent: content } : {} }));
  });
  const listening = deferred<void>(); server.listen(0, '127.0.0.1', listening.resolve); await listening.promise;
  t.after(async () => { release.resolve(); server.closeAllConnections(); const closed = deferred<void>(); server.close(() => closed.resolve()); await closed.promise; });
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('Mail fixture did not bind.');
  return { url: `http://127.0.0.1:${address.port}/mcp/`, entered: entered.promise, release: release.resolve };
}

function supervisor(f: { root: string; store: Workbench; role: Role; work: RepositoryWork }, url: string) {
  return new RuntimeSupervisor(f.store, { root: join(f.root, 'hosts'), socket: join(f.root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url, token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }],
    policy: {}, models: { providers: {} } }, 'http://127.0.0.1:4000', () => [f.role], f.work);
}

async function liveLeases(t: TestContext, root: string, attemptId: string) {
  const directory = join(root, 'hosts', attemptId);
  const paths = [join(directory, 'host-process'), join(directory, 'omp-process')];
  const children = paths.map(path => launchOwned(path, '/bin/sleep', ['60'], { cwd: root, env: { PATH: process.env.PATH } }));
  const stop = async () => {
    const results = await Promise.all(paths.map(path => stopOwned(path)));
    children.forEach(child => child.stdin.end());
    assert.ok(results.every(result => result.state === 'stopped'), 'Both real process leases stopped.');
  };
  t.after(async () => { await stop(); });
  for (const path of paths) {
    const owner = join(path, 'owner.json');
    const ready = deferred<void>();
    const watcher = watch(path, (_event, name) => { if (name === 'owner.json' && existsSync(owner)) ready.resolve(); });
    try { if (existsSync(owner)) ready.resolve(); await ready.promise; } finally { watcher.close(); }
  }
  const states = await Promise.all(paths.map(path => inspectOwned(path)));
  assert.ok(states.every(state => state.state === 'live'), 'Both independently leased processes became live.');
  return { directory, hostPid: states[0].owner!.pid, ompPid: states[1].owner!.pid, stop };
}

interface ContinuationFixture { root: string; store: Workbench; work: RepositoryWork; role: Role; model: AssignmentModel; run: Run;
  launch: { team: { name: string; roleId: string; coordinator: boolean; input: string }[]; repository: { mode: 'new' } } }

async function executedContinuation(f: ContinuationFixture, url: string, changedTeam = false) {
  f.store.claimCodingSlot(f.run.id);
  await f.work.prepare(f.run.id, f.launch.repository);
  const coordinator = f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  if (changedTeam) f.store.admitAssignment(f.run.id, { name: 'Writer', role: f.role, model: f.model, coordinator: false, input: 'Different teammate.' });
  f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id, 'recovery'); f.store.finishPause(f.run.id);
  const runtime = supervisor(f, url);
  const seam = runtime as unknown as { launchAssignments: (id: string, items: { attempt: { id: string } }[]) => Promise<void>;
    request: (host: unknown, body: { op: string; id?: string; message?: string }) => Promise<object>; hosts: Map<string, object> };
  const inputs: { id: string; message: string }[] = [];
  seam.launchAssignments = async (id, items) => {
    for (const item of items) { f.store.claimAttempt(item.attempt.id); seam.hosts.set(item.attempt.id, { attemptId: item.attempt.id, runId: id, url: '', pane: '', token: '' }); }
  };
  seam.request = async (_host, body) => { if (body.op === 'input') inputs.push({ id: body.id!, message: body.message! }); return {}; };
  await runtime.resume(f.run.id);
  seam.hosts.clear(); await runtime.close();
  assert.equal(f.store.getRun(f.run.id).status, 'running');
  assert.ok(inputs.every(input => input.id.startsWith('continuation:')));
  return { coordinator, inputs };
}

test('REVIEW-R2 queued and active Resume excludes initial scheduling through the real supervisor lane', async t => {
  const f = fixture(t); const mail = await mailbox(t, true);
  f.store.claimCodingSlot(f.run.id);
  await f.work.prepare(f.run.id, { mode: 'new' });
  f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id, 'recovery'); f.store.finishPause(f.run.id);
  const secondTask = f.store.createTask({ title: 'Must remain queued', description: '', repositoryId: f.repo.id });
  const accepted = f.store.refine(secondTask.id, { revision: 1, sourceRevision: 1, brief: 'Wait', criteria: ['Wait'] });
  const second = f.store.queue(secondTask.id, { revision: accepted.revision, launch: f.launch }, defaultRoles);
  const runtime = supervisor(f, mail.url);
  const seam = runtime as unknown as { launchAssignments: (id: string, items: { attempt: { id: string } }[]) => Promise<void>;
    request: (host: unknown, body: { op: string; id?: string }) => Promise<object>; hosts: Map<string, object> };
  let launches = 0; const inputs: string[] = [];
  seam.launchAssignments = async (id, items) => {
    launches++;
    for (const item of items) { f.store.claimAttempt(item.attempt.id); seam.hosts.set(item.attempt.id, { attemptId: item.attempt.id, runId: id, url: '', pane: '', token: '' }); }
  };
  seam.request = async (_host, body) => { if (body.op === 'input') inputs.push(body.id!); return {}; };
  const initialStart = runtime.start.bind(runtime);
  let initialEnqueues = 0; const initialQueued = deferred<'initial-enqueued'>();
  runtime.start = (...args) => { initialEnqueues++; initialQueued.resolve('initial-enqueued'); return initialStart(...args); };
  const scheduler = new CodingScheduler(f.store, f.work, new Evidence(f.store, f.work, join(f.root, 'artifacts')), runtime, () => [f.role]);
  t.after(async () => { seam.hosts.clear(); await scheduler.close(); await runtime.close(); });
  const resume = runtime.resume(f.run.id);
  try {
    assert.equal(runtime.busy(f.run.id), true, 'The queued-before-callback interval is already owned.');
    scheduler.kick(); await mail.entered;
    assert.equal(f.store.getRun(f.run.id).status, 'awaiting_runner');
    assert.equal(runtime.busy(f.run.id), true, 'Registration before first claim remains owned.');
    scheduler.kick();
    const outcome = await Promise.race([scheduler.idle().then(() => 'skipped' as const), initialQueued.promise]);
    assert.equal(outcome, 'skipped');
    mail.release(); await resume; await scheduler.idle();
    assert.equal(initialEnqueues, 0); assert.equal(launches, 1);
    assert.equal(inputs.length, 1); assert.match(inputs[0], /^continuation:/);
    assert.equal(f.store.getRun(f.run.id).status, 'running');
    assert.equal(f.store.codingSlot()?.id, f.run.id);
    assert.equal(f.store.runtime(second.id).assignments.length, 0);
  } finally { mail.release(); await Promise.allSettled([resume, scheduler.idle()]); seam.hosts.clear(); }
});
test('M6.3 re-holds released work before new queued plans after readiness regresses, without preemption or restart', async t => {
  const f = fixture(t);
  const secondTask = f.store.createTask({ title: 'Current slot holder', description: '', repositoryId: f.repo.id });
  const secondBrief = f.store.refine(secondTask.id, { revision: 1, sourceRevision: 1, brief: 'Hold the slot', criteria: ['Remain held'] });
  const second = f.store.queue(secondTask.id, { revision: secondBrief.revision, launch: f.launch }, defaultRoles);
  const thirdTask = f.store.createTask({ title: 'Queued behind holder', description: '', repositoryId: f.repo.id });
  const thirdBrief = f.store.refine(thirdTask.id, { revision: 1, sourceRevision: 1, brief: 'Wait', criteria: ['Wait'] });
  const third = f.store.queue(thirdTask.id, { revision: thirdBrief.revision, launch: f.launch }, defaultRoles);
  const candidateId = randomUUID();
  const publication: PublicationRecord = { id: randomUUID(), runId: f.run.id, attemptId: randomUUID(), candidateId,
    repositoryPath: f.checkout, repository: { uuid: `{${randomUUID()}}`, workspace: 'fixture', slug: 'repository' },
    branch: 'crewroom/first', target: 'main',
    revision: { base: 'a'.repeat(40), head: 'b'.repeat(40), tree: 'c'.repeat(40), worktreeHash: 'd'.repeat(64), dirty: false,
      requirementsHash: 'e'.repeat(64), instructionsHash: 'f'.repeat(64) },
    title: 'First', description: 'Review', policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1 }, ready: true, blockers: [] };
  f.store.claimCodingSlot(f.run.id);
  f.store.savePublication(publication);
  f.store.releaseCodingSlot(f.run.id, 'review', candidateId);
  f.store.finishPause(f.run.id);
  f.store.claimCodingSlot(second.id); f.store.startExecution(second.id);
  f.store.savePublication({ ...publication, ready: false, reviewInvalidated: true, blockers: ['A required hosted check failed.'] });

  const starts: string[] = [];
  const runtime = { preflightStart: () => {}, busy: () => false,
    start: async (runId: string) => { starts.push(runId); }, startScheduled: async (runId: string) => { starts.push(runId); },
    pause: async (runId: string) => f.store.getRun(runId) } as unknown as RuntimeSupervisor;
  const scheduler = new CodingScheduler(f.store, f.work, new Evidence(f.store, f.work, join(f.root, 'artifacts')), runtime, () => [f.role]);
  t.after(async () => { await scheduler.close(); });

  scheduler.kick(); await scheduler.idle();
  assert.equal(f.store.codingSlot()?.id, second.id, 'A readiness regression cannot preempt the current holder.');
  assert.deepEqual(starts, [], 'A readiness regression does not auto-resume agent work.');
  assert.equal(f.store.snapshot().runs.find(run => run.id === f.run.id)?.readinessHold?.state, 'pending');

  f.store.setDesiredState(second.id, 'cancelled');
  scheduler.kick(); await scheduler.idle();
  assert.equal(f.store.codingSlot()?.id, f.run.id, 'The released run re-holds before a new queued plan.');
  assert.equal(f.store.getRun(f.run.id).status, 'paused');
  assert.equal(f.store.getRun(f.run.id).desiredState, 'paused');
  assert.equal(f.store.getRun(f.run.id).pauseReason, 'validation', 'The re-held run no longer claims to await human review.');
  assert.deepEqual(starts, [], 'Re-holding stays paused until an explicit Resume.');
  assert.equal(f.store.snapshot().runs.find(run => run.id === f.run.id)?.readinessHold?.state, 'held');
  assert.equal(f.store.codingQueue().plans.some(item => item.id === third.id), true);
  f.store.savePublication({ ...publication, ready: true, reviewInvalidated: false, blockers: [] });
  scheduler.kick(); await scheduler.idle();
  assert.equal(f.store.codingSlot()?.id, third.id, 'Fresh matching readiness releases the re-held slot to the local queue.');
  assert.equal(f.store.getRun(f.run.id).status, 'paused', 'Restored readiness does not auto-resume the released run.');
  assert.equal(f.store.snapshot().runs.find(run => run.id === f.run.id)?.readinessHold, undefined);
  assert.equal(f.store.getRun(f.run.id).pauseReason, 'review', 'Restored readiness returns to the human review gate.');
  assert.deepEqual(starts, [third.id]);
  const fourthTask = f.store.createTask({ title: 'After a provider outage', description: '', repositoryId: f.repo.id });
  const fourthBrief = f.store.refine(fourthTask.id, { revision: 1, sourceRevision: 1, brief: 'Proceed', criteria: ['Proceed'] });
  const fourth = f.store.queue(fourthTask.id, { revision: fourthBrief.revision, launch: f.launch }, defaultRoles);
  f.store.savePublication({ ...publication, ready: false, reviewInvalidated: false, blockers: ['Bitbucket unavailable'] });
  f.store.setDesiredState(third.id, 'cancelled'); scheduler.kick(); await scheduler.idle();
  assert.equal(f.store.codingSlot()?.id, fourth.id, 'Provider unavailability alone does not re-hold a review-released run.');
});
test('M6.3: queued review Resume continues once the slot frees and keeps ordinary plans waiting', async t => {
  const f = fixture(t); const mail = await mailbox(t);
  f.store.claimCodingSlot(f.run.id);
  await f.work.prepare(f.run.id, f.launch.repository);
  f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  f.store.setDesiredState(f.run.id, 'running'); f.store.startExecution(f.run.id);
  const candidateId = randomUUID();
  const publication: PublicationRecord = { id: randomUUID(), runId: f.run.id, attemptId: randomUUID(), candidateId,
    repositoryPath: f.store.repositoryWork(f.run.id)!.repository.path, repository: { uuid: `{${randomUUID()}}`, workspace: 'fixture', slug: 'repository' },
    branch: f.store.repositoryWork(f.run.id)!.branch, target: 'main',
    revision: { base: 'a'.repeat(40), head: 'b'.repeat(40), tree: 'c'.repeat(40), worktreeHash: 'd'.repeat(64), dirty: false,
      requirementsHash: 'e'.repeat(64), instructionsHash: 'f'.repeat(64) },
    title: 'Released', description: 'Review', policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1, providerState: 'OPEN' }, ready: true, blockers: [],
    feedback: { state: 'partial', observedAt: '2026-10-01T00:00:00Z', reason: 'Second page unavailable', comments: [{ id: 51,
      created_on: '2026-09-28T00:00:00Z', updated_on: '2026-09-28T00:01:00Z', deleted: false, content: { raw: 'Fix the edge case' },
      inline: { path: 'sum.py', to: 2 }, currentness: 'known' }] } };
  assert.equal(publication.branch, f.store.repositoryWork(f.run.id)!.branch);
  assert.equal(publication.repositoryPath, f.store.repositoryWork(f.run.id)!.repository.path);
  f.store.savePublication(publication);
  f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id, 'review'); f.store.finishPause(f.run.id);
  f.store.releaseCodingSlot(f.run.id, 'review', candidateId);

  const holderTask = f.store.createTask({ title: 'Current slot holder', description: '', repositoryId: f.repo.id });
  const holderBrief = f.store.refine(holderTask.id, { revision: 1, sourceRevision: 1, brief: 'Hold the slot', criteria: ['Remain held'] });
  const holder = f.store.queue(holderTask.id, { revision: holderBrief.revision, launch: f.launch }, defaultRoles);
  f.store.claimCodingSlot(holder.id);
  const holderAttempt = f.store.admitAssignment(holder.id, { name: 'Holder', role: f.role, model: f.model, coordinator: true, input: 'Keep the slot.' }).attempt;
  f.store.claimAttempt(holderAttempt.id);
  f.store.setDesiredState(holder.id, 'paused'); f.store.beginPause(holder.id, 'operator'); f.store.finishPause(holder.id);
  const waitingTask = f.store.createTask({ title: 'Ordinary queued plan', description: '', repositoryId: f.repo.id });
  const waitingBrief = f.store.refine(waitingTask.id, { revision: 1, sourceRevision: 1, brief: 'Wait', criteria: ['Wait'] });
  const waiting = f.store.queue(waitingTask.id, { revision: waitingBrief.revision, requestId: randomUUID() }, defaultRoles);

  const runtime = supervisor(f, mail.url);
  const seam = runtime as unknown as { launchAssignments: (id: string, items: { attempt: { id: string } }[]) => Promise<void>;
    request: (host: unknown, body: { op: string; id?: string; message?: string }) => Promise<object>; hosts: Map<string, object> };
  let launches = 0; const inputs: string[] = []; const bodies: string[] = [];
  seam.launchAssignments = async (id, items) => {
    launches++;
    for (const item of items) { f.store.claimAttempt(item.attempt.id); seam.hosts.set(item.attempt.id, { attemptId: item.attempt.id, runId: id, url: '', pane: '', token: '' }); }
  };
  seam.request = async (_host, body) => { if (body.op === 'input') { inputs.push(body.id!); bodies.push(body.message!); } return {}; };
  const scheduler = new CodingScheduler(f.store, f.work, new Evidence(f.store, f.work, join(f.root, 'artifacts')), runtime, () => [f.role]);
  t.after(async () => { seam.hosts.clear(); await scheduler.close(); await runtime.close(); });
  runtime.onSchedulingChange(() => scheduler.kick());

  await runtime.resume(f.run.id);
  assert.equal(f.store.getRun(f.run.id).resumeRequest?.state, 'queued');
  const captured = f.store.getRun(f.run.id).feedbackContinuations!;
  assert.equal(captured.length, 1); assert.equal(captured[0].feedback.state, 'partial');
  assert.equal(captured[0].feedback.comments[0].content?.raw, 'Fix the edge case');
  const { currentness: _currentness, ...known } = publication.feedback!.comments[0];
  f.store.savePublicationFeedback(f.run.id, 1, { state: 'complete', comments: [known, { ...known, id: 52,
    updated_on: '2026-10-01T00:02:00Z', content: { raw: 'Late uncaptured instruction' } }] }, new Date().toISOString());
  await runtime.resume(f.run.id);
  assert.equal(f.store.getRun(f.run.id).feedbackContinuations?.length, 1, 'A repeated queued Resume does not capture a new assignment.');
  scheduler.kick(); await scheduler.idle();
  assert.equal(f.store.codingSlot()?.id, holder.id);
  assert.equal(launches, 0, 'Queued Resume does not preempt the current holder.');

  f.store.setDesiredState(holder.id, 'cancelled');
  scheduler.kick(); await scheduler.idle();
  assert.equal(f.store.getRun(f.run.id).resumeRequest?.state, 'queued', 'Unconfirmed previous ownership must not erase the explicit Resume.');
  assert.equal(f.store.codingSlot(), undefined, 'No replacement claims the slot until the prior owner stops.');
  f.store.recordOwnership(holderAttempt.id, true);
  await runtime.cancel(holder.id); await scheduler.idle();
  assert.equal(f.store.codingSlot()?.id, f.run.id);
  assert.equal(f.store.getRun(f.run.id).status, 'running');
  assert.equal(f.store.getRun(f.run.id).resumeRequest, undefined);
  assert.equal(launches, 1); assert.equal(inputs.length, 1); assert.match(inputs[0], /^continuation:/);
  assert.match(bodies[0], /Fix the edge case/); assert.match(bodies[0], /untrusted reviewer material/);
  assert.match(bodies[0], /partial/);
  assert.doesNotMatch(bodies[0], /Late uncaptured instruction/, 'A later comment must not become implicit continuation input.');
  const reopened = f.open();
  assert.equal(reopened.getRun(f.run.id).feedbackContinuations?.[0].state, 'handed_off');
  assert.equal(reopened.getRun(f.run.id).feedbackContinuations?.[0].feedback.comments[0].content?.raw, 'Fix the edge case');
  scheduler.kick(); await scheduler.idle();
  assert.equal(launches, 1, 'A processed request cannot start a duplicate continuation.');
  assert.equal(f.store.runtime(waiting.id).assignments.length, 0);
});


test('REVIEW-R2 a new control revision during awaited worktree preparation suppresses stale scheduled start', async t => {
  const f = fixture(t); f.store.claimCodingSlot(f.run.id);
  f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  const entered = deferred<void>(); const release = deferred<void>();
  const realPrepare = f.work.prepare.bind(f.work);
  f.work.prepare = async (runId, input) => { entered.resolve(); await release.promise; return realPrepare(runId, input); };
  const scheduler = f.scheduler;
  scheduler.kick(); await entered.promise;
  f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id, 'operator'); f.store.finishPause(f.run.id);
  const resumed = f.store.setDesiredState(f.run.id, 'running'); f.store.prepareContinuation(f.run.id, resumed.controlRevision!);
  release.resolve(); await scheduler.idle();
  assert.equal(f.starts(), 0);
  assert.equal(f.store.getRun(f.run.id).status, 'awaiting_runner');
  assert.equal(f.store.getRun(f.run.id).attentionReason, undefined);
  assert.equal(f.store.codingSlot()?.id, f.run.id);
});

test('REVIEW-R2 execution is fenced before the initial host input acknowledgement', async t => {
  const f = fixture(t); const mail = await mailbox(t);
  const decision = f.store.claimCodingSlot(f.run.id);
  await f.work.prepare(f.run.id, f.launch.repository);
  const runtime = supervisor(f, mail.url);
  const seam = runtime as unknown as { launchAssignments: (id: string, items: { attempt: { id: string } }[]) => Promise<void>;
    request: (host: unknown, body: { op: string; id?: string }) => Promise<object>; hosts: Map<string, object> };
  const inputEntered = deferred<void>(); const inputReply = deferred<void>();
  seam.launchAssignments = async (id, items) => {
    for (const item of items) {
      f.store.claimAttempt(item.attempt.id);
      seam.hosts.set(item.attempt.id, { attemptId: item.attempt.id, runId: id, url: '', pane: '', token: '' });
    }
  };
  seam.request = async (_host, body) => {
    if (body.id?.startsWith('initial:')) { inputEntered.resolve(); await inputReply.promise; }
    return {};
  };
  t.after(async () => { inputReply.resolve(); seam.hosts.clear(); await runtime.close(); });
  const starting = runtime.startScheduled(f.run.id, decision.schedule!.id, decision.controlRevision ?? 0, { team: f.launch.team }, [f.role]);
  try {
    await inputEntered.promise;
    assert.equal(f.store.getRun(f.run.id).status, 'running');
    const persisted = f.open();
    assert.ok(persisted.getRun(f.run.id).launchCompletedAt, 'The fence precedes acknowledgement, not wrapper completion.');
    assert.equal(persisted.canReplayInitialLaunch(f.run.id, decision.schedule!.id, decision.controlRevision ?? 0), false);
    const work = new RepositoryWork(persisted, join(f.root, 'worktrees'));
    let replayed = 0;
    const fake = { busy: () => false, startScheduled: async () => { replayed++; return {}; } } as unknown as RuntimeSupervisor;
    const scheduler = new CodingScheduler(persisted, work, new Evidence(persisted, work, join(f.root, 'artifacts')), fake, () => [f.role]);
    scheduler.kick(); await scheduler.idle(); await scheduler.close();
    assert.equal(replayed, 0, 'An interrupted acknowledgement cannot authorize a second initial turn.');
  } finally { inputReply.resolve(); await Promise.allSettled([starting]); seam.hosts.clear(); }
});

test('REVIEW-R2 restart adopts a live resumed host with execution fence, not initial team input', async t => {
  const f = fixture(t); const mail = await mailbox(t);
  const { coordinator } = await executedContinuation(f, mail.url);
  const lease = await liveLeases(t, f.root, coordinator.attempt.id);
  f.store.recordAttemptIdentity(coordinator.attempt.id, { hostPid: lease.hostPid, ompPid: lease.ompPid,
    sessionId: 'resumed-session', hostUrl: 'http://127.0.0.1:4001/', pane: '%1' });
  writeFileSync(join(lease.directory, 'host.json'), JSON.stringify({ attemptId: coordinator.attempt.id, backend: 'http://127.0.0.1:4000', token: 'fixture' }));
  f.store.close();
  const old = new DatabaseSync(f.path); old.prepare("UPDATE runs SET data=json_remove(data,'$.launchCompletedAt') WHERE id=?").run(f.run.id); old.close();
  const store = f.open(); const work = new RepositoryWork(store, join(f.root, 'worktrees'));
  const runtime = supervisor({ ...f, store, work }, mail.url);
  const seam = runtime as unknown as { request: (host: unknown, body: { op: string }) => Promise<object>;
    register: (host: { attemptId: string }) => void; hosts: Map<string, object> };
  let adopted = 0;
  seam.request = async (_host, body) => body.op === 'identity'
    ? { hostPid: lease.hostPid, ompPid: lease.ompPid, sessionId: 'resumed-session' } : {};
  seam.register = host => { adopted++; seam.hosts.set(host.attemptId, host); };
  try {
    await runtime.initialize();
    assert.ok(store.getRun(f.run.id).launchCompletedAt);
    assert.equal(adopted, 1); assert.equal(store.runtime(f.run.id).attempts.length, 1);
    assert.equal((await inspectOwned(join(lease.directory, 'omp-process'))).state, 'live');
    assert.equal(store.getRun(f.run.id).attentionReason, undefined);
  } finally { await runtime.close(); }
});

test('REVIEW-R2 restart replaces a confirmed-dead resumed host with history-linked continuation, not the saved initial team', async t => {
  const f = fixture(t); const mail = await mailbox(t);
  const { coordinator } = await executedContinuation(f, mail.url, true);
  const oldCoordinator = coordinator.attempt.id;
  const writer = f.store.runtime(f.run.id).assignments.find(item => item.name === 'Writer')!;
  const writerAttempt = writer.currentAttemptId;
  const lease = await liveLeases(t, f.root, oldCoordinator);
  f.store.recordAttemptIdentity(oldCoordinator, { hostPid: lease.hostPid, ompPid: lease.ompPid,
    sessionId: 'resumed-session', hostUrl: 'http://127.0.0.1:4001/', pane: '%1' });
  writeFileSync(join(lease.directory, 'host.json'), JSON.stringify({ attemptId: oldCoordinator, backend: 'http://127.0.0.1:4000', token: 'fixture' }));
  writeFileSync(join(lease.directory, 'inputs.jsonl'), JSON.stringify({ id: `continuation:${oldCoordinator}`, message: 'Previously received.', disposition: 'completed' }) + '\n');
  const writerDir = join(f.root, 'hosts', writerAttempt); mkdirSync(writerDir, { recursive: true });
  writeFileSync(join(writerDir, 'inputs.jsonl'), JSON.stringify({ id: `continuation:${writerAttempt}`, message: 'Pending writer.', disposition: 'received' }) + '\n');
  await lease.stop();
  f.store.close();
  const old = new DatabaseSync(f.path); old.prepare("UPDATE runs SET data=json_remove(data,'$.launchCompletedAt') WHERE id=?").run(f.run.id); old.close();
  const store = f.open(); const work = new RepositoryWork(store, join(f.root, 'worktrees'));
  const runtime = supervisor({ ...f, store, work }, mail.url);
  const seam = runtime as unknown as { launchAssignments: (id: string, items: { attempt: { id: string } }[]) => Promise<void>;
    request: (host: unknown, body: { op: string; id?: string; message?: string }) => Promise<object>; hosts: Map<string, object> };
  const inputs: { id: string; message: string }[] = [];
  seam.launchAssignments = async (id, items) => {
    for (const item of items) { store.claimAttempt(item.attempt.id); seam.hosts.set(item.attempt.id, { attemptId: item.attempt.id, runId: id, url: '', pane: '', token: '' }); }
  };
  seam.request = async (_host, body) => { if (body.op === 'input') inputs.push({ id: body.id!, message: body.message! }); return {}; };
  try {
    await runtime.initialize();
    const run = store.getRun(f.run.id);
    assert.equal(run.status, 'running'); assert.equal(run.attentionReason, undefined);
    assert.ok(run.launchCompletedAt);
    const assignments = store.runtime(f.run.id).assignments;
    assert.equal(assignments.length, 2, 'The changed team was not replaced with the original one-person launch spec.');
    const current = store.getAttempt(assignments.find(item => item.coordinator)!.currentAttemptId);
    assert.equal(current.generation, 2); assert.equal(current.previousAttemptId, oldCoordinator);
    assert.equal(store.getAttempt(assignments.find(item => item.name === 'Writer')!.currentAttemptId).previousAttemptId, writerAttempt);
    assert.ok(inputs.length >= 1 && inputs.every(input => input.id.startsWith('continuation:')));
    const history = JSON.parse(inputs.find(input => input.id === `continuation:${current.id}`)!.message).inputHistory;
    assert.ok(history.some((item: { id: string; attemptId: string }) => item.id === `continuation:${oldCoordinator}` && item.attemptId === oldCoordinator));
    let initial = 0;
    const start = runtime.startScheduled.bind(runtime);
    runtime.startScheduled = (...args) => { initial++; return start(...args); };
    const scheduler = new CodingScheduler(store, work, new Evidence(store, work, join(f.root, 'artifacts')), runtime, () => [f.role]);
    scheduler.kick(); await scheduler.idle(); await scheduler.close();
    assert.equal(initial, 0);
  } finally { seam.hosts.clear(); await runtime.close(); }
});

test('REVIEW-R2 startup re-reads agent_start collected during stop before selecting recovery', async t => {
  const f = fixture(t); const mail = await mailbox(t);
  f.store.claimCodingSlot(f.run.id);
  await f.work.prepare(f.run.id, f.launch.repository);
  const { attempt } = f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  f.store.setDesiredState(f.run.id, 'running'); f.store.claimAttempt(attempt.id);
  const host = join(f.root, 'hosts', attempt.id); mkdirSync(host, { recursive: true });
  writeFileSync(join(host, 'events.jsonl'), JSON.stringify({ sequence: 1, event: { type: 'agent_start' } }) + '\n');
  assert.equal(f.store.getRun(f.run.id).launchCompletedAt, undefined);
  const runtime = supervisor(f, mail.url);
  const seam = runtime as unknown as { launchAssignments: (id: string, items: { attempt: { id: string } }[]) => Promise<void>;
    request: (host: unknown, body: { op: string; id?: string }) => Promise<object>; hosts: Map<string, object> };
  const inputs: string[] = [];
  seam.launchAssignments = async (id, items) => {
    for (const item of items) { f.store.claimAttempt(item.attempt.id); seam.hosts.set(item.attempt.id, { attemptId: item.attempt.id, runId: id, url: '', pane: '', token: '' }); }
  };
  seam.request = async (_host, body) => { if (body.op === 'input') inputs.push(body.id!); return {}; };
  try {
    await runtime.initialize();
    assert.ok(f.store.getRun(f.run.id).launchCompletedAt);
    const current = f.store.getAttempt(f.store.runtime(f.run.id).assignments[0].currentAttemptId);
    assert.equal(current.previousAttemptId, attempt.id);
    assert.deepEqual(inputs, [`continuation:${current.id}`]);
    assert.equal(f.store.getRun(f.run.id).status, 'running');
  } finally { seam.hosts.clear(); await runtime.close(); }
});

test('REVIEW-R2 execution fence: claim is not execution; start and observed agent_start persist one fence across reopen and continuation', t => {
  for (const entry of ['start', 'event'] as const) {
    const f = fixture(t);
    f.store.claimCodingSlot(f.run.id);
    const { assignment, attempt } = f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
    f.store.setDesiredState(f.run.id, 'running'); f.store.claimAttempt(attempt.id);
    assert.equal(f.store.getRun(f.run.id).launchCompletedAt, undefined, 'Claim alone is not execution.');
    if (entry === 'start') f.store.startExecution(f.run.id);
    else f.store.recordAgentEvent(attempt.id, 1, { type: 'agent_start' });
    const first = f.store.getRun(f.run.id).launchCompletedAt;
    assert.ok(first, `${entry} enters execution and records the fence.`);
    f.store.close(); const opened = f.open();
    assert.equal(opened.getRun(f.run.id).launchCompletedAt, first);
    opened.setDesiredState(f.run.id, 'paused'); opened.beginPause(f.run.id); opened.recordOwnership(attempt.id, true); opened.finishPause(f.run.id);
    const continuation = opened.setDesiredState(f.run.id, 'running');
    opened.replaceAttempt(assignment.id, { automatic: false }); opened.prepareContinuation(f.run.id, continuation.controlRevision!);
    assert.equal(opened.getRun(f.run.id).launchCompletedAt, first);
    assert.throws(() => opened.restartScheduledLaunch(f.run.id), /not safely stopped/);
  }
});

test('REVIEW-R2 first claim without execution remains eligible for interrupted initial replay', t => {
  const f = fixture(t); f.store.claimCodingSlot(f.run.id);
  const { attempt } = f.store.admitAssignment(f.run.id, { name: 'Coordinator', role: f.role, model: f.model, coordinator: true, input: 'Coordinate.' });
  f.store.setDesiredState(f.run.id, 'running'); f.store.claimAttempt(attempt.id); f.store.recordOwnership(attempt.id, true);
  f.store.beginPause(f.run.id, 'recovery'); f.store.finishPause(f.run.id);
  f.store.close(); const reopened = f.open();
  assert.equal(reopened.getRun(f.run.id).launchCompletedAt, undefined);
  reopened.restartScheduledLaunch(f.run.id);
  assert.deepEqual(reopened.runtime(f.run.id).attempts.map(item => item.generation), [1, 2]);
});

test('REVIEW-R2 preparation hold survives generic kicks, reorder and restart; a fresh launch request alone retries', async t => {
  const f = fixture(t);
  const gone = join(f.root, 'checkout-away'); renameSync(f.checkout, gone);
  f.scheduler.kick(); await f.scheduler.idle();
  const initial = f.store.codingQueue();
  assert.equal(initial.plans[0].id, f.run.id);
  assert.equal(initial.plans[0].launchHold?.source, 'preparation');
  assert.match(initial.plans[0].holdReason!, /checkout.*unavailable.*Save launch details/i);
  assert.doesNotMatch(initial.plans[0].holdReason!, /ENOENT|\/private\/|crewroom-launch-hold-/);
  const hold = initial.plans[0].launchHold!;
  for (let index = 0; index < 3; index++) {
    f.scheduler.kick(); await f.scheduler.idle();
    assert.equal(f.store.codingQueue().revision, initial.revision);
    assert.deepEqual(f.store.codingQueue().plans[0].launchHold, hold);
    assert.equal(f.store.codingSlot(), undefined);
  }
  assert.equal(f.store.reorderQueue({ requestId: randomUUID(), expectedRevision: initial.revision, runIds: [f.run.id] }).plans[0].id, f.run.id);
  renameSync(gone, f.checkout);
  f.scheduler.kick(); await f.scheduler.idle();
  assert.equal(f.store.codingQueue().plans[0].launchHold?.at, hold.at, 'Restoring the checkout is not automatic retry consent.');
  f.store.close(); const reopened = f.open();
  const work = new RepositoryWork(reopened, join(f.root, 'worktrees'));
  const execute = async () => { reopened.startExecution(f.run.id); return {}; };
  const runtime = { preflightStart: () => {}, start: execute, startScheduled: execute, pause: async () => reopened.getRun(f.run.id), busy: () => false } as unknown as RuntimeSupervisor;
  const scheduler = new CodingScheduler(reopened, work, new Evidence(reopened, work, join(f.root, 'artifacts')), runtime, () => [f.role]);
  scheduler.kick(); await scheduler.idle();
  assert.equal(reopened.codingQueue().plans[0].launchHold?.at, hold.at);
  const requestId = randomUUID();
  reopened.setQueueLaunch(f.run.id, { requestId, launch: f.launch }, defaultRoles);
  assert.equal(reopened.codingQueue().plans[0].launchHold, undefined);
  scheduler.kick(); await scheduler.idle(); await scheduler.close();
  assert.equal(reopened.codingSlot()?.id, f.run.id);
  assert.equal(reopened.repositoryWork(f.run.id)?.state, 'ready');
});

test('REVIEW-R2 branch and destination preparation holds explain reconciliation without leaking internal paths', async t => {
  for (const conflict of ['branch', 'destination'] as const) {
    const f = fixture(t);
    if (conflict === 'branch') execFileSync('git', ['-C', f.checkout, 'branch', `codex/crewroom/${f.run.id}`]);
    else mkdirSync(join(f.root, 'worktrees', f.run.id), { recursive: true });
    f.scheduler.kick(); await f.scheduler.idle();
    const first = f.store.codingQueue();
    assert.equal(first.plans[0].launchHold?.source, 'preparation');
    assert.match(first.plans[0].holdReason!, conflict === 'branch' ? /branch.*(inspect|reconcile)/i : /destination.*(inspect|reconcile)/i);
    assert.doesNotMatch(first.plans[0].holdReason!, /ENOENT|\/private\/|crewroom-launch-hold-/);
    f.scheduler.kick(); await f.scheduler.idle();
    assert.equal(f.store.codingQueue().revision, first.revision);
    assert.equal(f.store.codingQueue().plans[0].launchHold?.at, first.plans[0].launchHold?.at);
  }
});

test('REVIEW-R2 a replayed launch request cannot clear a newer preparation hold; manual failure renews it', async t => {
  const f = fixture(t); const gone = join(f.root, 'checkout-away'); renameSync(f.checkout, gone);
  f.scheduler.kick(); await f.scheduler.idle();
  const original = f.store.codingQueue().plans[0].launchHold!;
  const retryId = randomUUID();
  const receipt = f.store.setQueueLaunch(f.run.id, { requestId: retryId, launch: f.launch }, defaultRoles);
  assert.equal(receipt.launchHold, undefined);
  f.scheduler.kick(); await f.scheduler.idle();
  const current = f.store.codingQueue();
  assert.notEqual(current.plans[0].launchHold!.at, original.at);
  const replay = f.store.setQueueLaunch(f.run.id, { requestId: retryId, launch: f.launch }, defaultRoles);
  assert.deepEqual(replay, receipt);
  f.scheduler.kick(); await f.scheduler.idle();
  assert.equal(f.store.codingQueue().revision, current.revision);
  assert.deepEqual(f.store.codingQueue().plans[0].launchHold, current.plans[0].launchHold);
  assert.equal(f.store.setLaunchHold(f.run.id, f.run.launch!, undefined), false, 'A preflight result cannot clear a preparation hold.');
  renameSync(gone, f.checkout);
  f.scheduler.kick(); await f.scheduler.idle();
  assert.deepEqual(f.store.codingQueue().plans[0].launchHold, current.plans[0].launchHold);
  f.store.setQueueLaunch(f.run.id, { requestId: randomUUID(), launch: f.launch }, defaultRoles);
  f.scheduler.kick(); await f.scheduler.idle();
  assert.equal(f.starts(), 1);
  assert.equal(f.store.repositoryWork(f.run.id)?.state, 'ready');

  const manual = fixture(t); const goneAgain = join(manual.root, 'checkout-away'); renameSync(manual.checkout, goneAgain);
  const first = manual.store.claimCodingSlot(manual.run.id, true);
  await assert.rejects(manual.scheduler.launch(first, { team: manual.launch.team }), /checkout is unavailable/);
  const failed = manual.store.codingQueue().plans[0].launchHold!;
  assert.equal(failed.source, 'preparation');
  const second = manual.store.claimCodingSlot(manual.run.id, true);
  await assert.rejects(manual.scheduler.launch(second, { team: manual.launch.team }), /checkout is unavailable/);
  assert.notEqual(manual.store.codingQueue().plans[0].launchHold!.at, failed.at);
  assert.equal(manual.store.repositoryWork(manual.run.id), undefined);
});

test('REVIEW-R2 legacy unclassified launch holds stay owned by preparation until explicit retry', async t => {
  const f = fixture(t); f.store.claimCodingSlot(f.run.id);
  f.store.deferUnadmittedLaunch(f.run.id, 'Inspect the repository and Save launch details to retry.');
  f.store.close();
  const database = new DatabaseSync(f.path);
  database.prepare("UPDATE runs SET data=json_remove(data,'$.launchHold.source') WHERE id=?").run(f.run.id); database.close();
  const reopened = f.open(); const work = new RepositoryWork(reopened, join(f.root, 'worktrees'));
  const runtime = { preflightStart: () => {}, busy: () => false } as unknown as RuntimeSupervisor;
  const scheduler = new CodingScheduler(reopened, work, new Evidence(reopened, work, join(f.root, 'artifacts')), runtime, () => [f.role]);
  const held = reopened.codingQueue();
  scheduler.kick(); await scheduler.idle();
  assert.equal(reopened.codingQueue().revision, held.revision);
  assert.deepEqual(reopened.codingQueue().plans[0].launchHold, held.plans[0].launchHold);
  reopened.setQueueLaunch(f.run.id, { requestId: randomUUID(), launch: f.launch }, defaultRoles);
  assert.equal(reopened.codingQueue().plans[0].launchHold, undefined);
  await scheduler.close();
});

test('REVIEW-R2 HTTP Save launch details retries once per fresh requestId, not on replay or role notifications', async t => {
  const f = fixture(t); const gone = join(f.root, 'checkout-away'); renameSync(f.checkout, gone);
  f.store.close(); await f.scheduler.close();
  const rolePath = join(f.root, 'roles.json'); new RoleCatalog(rolePath).update('coordinator', { ...f.role, revision: 1 });
  const server = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath,
    runtimeConfig: { root: join(f.root, 'hosts'), socket: join(f.root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], policy: {}, models: { providers: {} } } });
  const workspace = async () => (await (await fetch(`${server.origin}/api/workspace`)).json());
  const post = async (path: string, body: unknown) => {
    const response = await fetch(`${server.origin}/api${path}`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    assert.equal(response.status, 200); return response.json();
  };
  const until = async (predicate: (data: Workspace) => boolean) => {
    const deadline = Date.now() + 8000;
    do {
      const data = await workspace();
      if (predicate(data)) return data;
      await new Promise<void>(resolve => setImmediate(resolve)); // Observe the real HTTP event, not a fixed-duration sleep.
    } while (Date.now() < deadline);
    assert.fail('Scheduler did not reach the expected durable state.');
  };
  try {
    const first = await until(data => data.queue.plans[0]?.launchHold?.source === 'preparation');
    const initialRevision = first.queue.revision;
    const id = randomUUID();
    const receipt = await post(`/runs/${f.run.id}/launch`, { requestId: id, launch: f.launch });
    assert.equal(receipt.launchHold, undefined);
    const second = await until(data => data.queue.revision === initialRevision + 2 && data.queue.plans[0]?.launchHold?.source === 'preparation');
    assert.notEqual(second.queue.plans[0].launchHold.at, first.queue.plans[0].launchHold.at);
    assert.deepEqual(await post(`/runs/${f.run.id}/launch`, { requestId: id, launch: f.launch }), receipt);
    assert.equal((await workspace()).queue.revision, second.queue.revision);
    renameSync(gone, f.checkout);
    await post('/roles/coordinator', { ...f.role, revision: 2 });
    assert.deepEqual((await workspace()).queue.plans[0].launchHold, second.queue.plans[0].launchHold);
    await post(`/runs/${f.run.id}/launch`, { requestId: randomUUID(), launch: f.launch });
    await until(data => data.queue.slot?.runId === f.run.id && data.activity.some(agent => agent.runId === f.run.id));
    const observed = f.open();
    assert.equal(observed.repositoryWork(f.run.id)?.state, 'ready');
    assert.equal(observed.runtime(f.run.id).assignments.length, 1);
  } finally { await server.close(); }
});
