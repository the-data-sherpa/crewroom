import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { startServer } from '../server/app';
import { gitLatch } from './fixtures/git-latch';
import { seedRepository } from './fixtures/repository';
import { RepositoryConfigurationChanged, Workbench } from '../server/workbench';
import { validateRepository } from '../server/repository-configuration';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { CodingScheduler } from '../server/coding-scheduler';
import type { RuntimeSupervisor } from '../server/runtime/supervisor';
import { defaultRoles } from '../server/roles';
import { randomUUID } from 'node:crypto';

test('REP-01: real server restart retains the isolated branch, edits and revision observations', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-git-http-')); const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'answer'), 'one'); git('add', '.'); git('commit', '-m', 'fixture');
  const options = { port: 0, dataDir: join(root, 'server'), serveUi: false, rolePath: join(root, 'roles.json') };
  let server = await startServer(options);
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const request = async (path: string, body?: unknown) => {
    const response = await fetch(server.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
  };
  const repository = await request('/repositories', { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Own a patch', description: '', repositoryId: repository.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Own an isolated patch', criteria: ['Preserve operator work'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  const owned = await request(`/runs/${run.id}/repository`, { mode: 'new' });
  writeFileSync(join(owned.path, 'answer'), 'pending edits');
  await server.close(); server = await startServer(options);
  const retained = await request(`/runs/${run.id}/repository`);
  assert.equal(retained.path, owned.path); assert.equal(retained.drift.worktree, true);
  assert.equal(readFileSync(join(checkout, 'answer'), 'utf8'), 'one');
  assert.equal(readFileSync(join(owned.path, 'answer'), 'utf8'), 'pending edits');
  assert.equal((await request(`/runs/${run.id}/repository`, { mode: 'new' })).branch, owned.branch);
});

test('repository observation updates its cached revision when the same dirty path changes', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-git-status-')); const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'answer'), 'one'); git('add', '.'); git('commit', '-m', 'fixture');
  const server = await startServer({ port: 0, dataDir: join(root, 'server'), serveUi: false, rolePath: join(root, 'roles.json') });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const request = async (path: string, body?: unknown) => {
    const response = await fetch(server.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    assert.ok(response.ok);
    return response.json();
  };
  const repository = await request('/repositories', { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Observe edits', description: '', repositoryId: repository.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Observe', criteria: ['Same path changes'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  const owned = await request(`/runs/${run.id}/repository`, { mode: 'new' });
  const current = await request(`/runs/${run.id}/repository`);
  assert.deepEqual((await request(`/runs/${run.id}/repository`)).revision, current.revision);
  writeFileSync(join(owned.path, 'answer'), 'two');
  const first = await request(`/runs/${run.id}/repository`);
  assert.equal(first.drift.worktree, true);
  writeFileSync(join(owned.path, 'answer'), 'six');
  const second = await request(`/runs/${run.id}/repository`);
  assert.notEqual(second.revision.worktreeHash, first.revision.worktreeHash);
  assert.equal(second.drift.worktree, true);
});

test('R3: repository observation in flight during a refinement reports requirements drift', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-git-refinement-')); const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'answer'), 'one'); git('add', '.'); git('commit', '-m', 'fixture');
  const latch = gitLatch(root);
  const server = await startServer({ port: 0, dataDir: join(root, 'server'), serveUi: false, rolePath: join(root, 'roles.json') });
  t.after(async () => { latch.restore(); await server.close(); rmSync(root, { recursive: true, force: true }); });
  const request = async (path: string, body?: unknown) => {
    const response = await fetch(server.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
  };
  const repository = await request('/repositories', { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Observe refinements', description: '', repositoryId: repository.id });
  const refined = await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Observe', criteria: ['Accepted criterion'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: refined.revision });
  const owned = await request(`/runs/${run.id}/repository`, { mode: 'new' });
  assert.equal((await request(`/runs/${run.id}/repository`)).drift.requirements, false);
  // A worktree edit forces a fresh observation, so the next request reaches the latched merge-base probe.
  writeFileSync(join(owned.path, 'answer'), 'two');
  latch.arm(); const observing = request(`/runs/${run.id}/repository`); await latch.blocked();
  await request(`/tasks/${task.id}/refinement`, { revision: refined.revision, sourceRevision: refined.sourceRevision, brief: 'Observe', criteria: ['Accepted criterion', 'Changed criterion'] });
  latch.release();
  assert.equal((await observing).drift.requirements, true, 'The response returned after the refinement cannot report the old criteria as current.');
  assert.equal((await request(`/runs/${run.id}/repository`)).drift.requirements, true);
});

test('workspace publication availability matches only configured repository path without exposing credentials', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-config-availability-'));
  const first = join(root, 'first'), second = join(root, 'second'); seedRepository(first); seedRepository(second);
  const secret = 'fixture-private-credential';
  const server = await startServer({ port: 0, dataDir: root, serveUi: false, rolePath: join(root, 'roles.json'), publication: [{ repositoryPath: first,
    repository: { workspace: 'fixture', slug: 'first', uuid: '{00000000-0000-4000-8000-000000000011}' },
    policy: { statusKeys: ['unit'], pipelineSteps: ['Unit'], optionalSteps: [] }, credentials: async () => secret }] });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  for (const [name, path] of [['First', first], ['Second', second]]) {
    const response = await fetch(`${server.origin}/api/repositories`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ name, path, targetBranch: 'main' }) });
    assert.equal(response.status, 201);
  }
  const response = await fetch(`${server.origin}/api/workspace`);
  const text = await response.text(); assert.equal(text.includes(secret), false);
  const workspace = JSON.parse(text);
  assert.deepEqual(workspace.repositories.map((repository: { publicationAvailable: boolean }) => repository.publicationAvailable), [true, false]);
});

test('repository creation leaves the workspace responsive while its Git target probe waits', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-config-async-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const server = await startServer({ port: 0, dataDir: root, serveUi: false, rolePath: join(root, 'roles.json') });
  const latch = gitLatch(root, 'rev-parse');
  t.after(async () => { latch.restore(); await server.close(); rmSync(root, { recursive: true, force: true }); });
  const input = { name: 'Fixture', path: checkout, targetBranch: 'main', requestId: randomUUID() };
  const post = () => fetch(`${server.origin}/api/repositories`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(input) });
  latch.arm();
  try {
    const creating = post();
    await latch.blocked();
    const workspace = await fetch(`${server.origin}/api/workspace`, { signal: AbortSignal.timeout(1000) });
    assert.equal(workspace.status, 200, 'A pending Git probe must not stall observation requests.');
    assert.equal((await workspace.json()).repositories.length, 0);
    latch.release();
    const response = await creating;
    assert.equal(response.status, 201);
    const repository = await response.json();
    assert.equal((await post()).status, 200, 'A keyed replay returns the saved record.');
    assert.equal((await (await fetch(`${server.origin}/api/workspace`)).json()).repositories[0].id, repository.id);
  } finally { latch.release(); }
});

test('REP-02: HTTP edits revalidate Git asynchronously, reject stale revision and retired selections', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-http-edit-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const server = await startServer({ port: 0, dataDir: join(root, 'server'), serveUi: false, rolePath: join(root, 'roles.json') });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const post = async (path: string, body: unknown) => {
    const response = await fetch(`${server.origin}/api${path}`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    return { status: response.status, data: await response.json() };
  };
  const created = await post('/repositories', { name: 'Original', path: checkout, targetBranch: 'main' });
  assert.equal(created.status, 201);
  const repo = created.data;
  const input = { name: 'Updated', path: checkout, targetBranch: 'main', checks: [], revision: 1, requestId: randomUUID() };
  assert.match((await post(`/repositories/${repo.id}`, { ...input, targetBranch: 'missing' })).data.error, /Target branch must resolve/);
  const latch = gitLatch(root, 'rev-parse');
  try {
    latch.arm(); const pending = post(`/repositories/${repo.id}`, input);
    await latch.blocked();
    const workspace = await fetch(`${server.origin}/api/workspace`, { signal: AbortSignal.timeout(1000) });
    assert.equal(workspace.status, 200, 'An edit Git preflight cannot stall observation.');
    assert.equal((await workspace.json()).repositories[0].name, 'Original');
    latch.release();
    const saved = await pending;
    assert.equal(saved.status, 200); assert.equal(saved.data.revision, 2);
    assert.deepEqual((await post(`/repositories/${repo.id}`, input)).data, saved.data);
    const stale = await post(`/repositories/${repo.id}`, { ...input, name: 'Lost update', requestId: randomUUID() });
    assert.equal(stale.status, 409); assert.match(stale.data.error, /revision changed/);
    assert.equal((await (await fetch(`${server.origin}/api/workspace`)).json()).repositories[0].name, 'Updated');
    const retired = await post(`/repositories/${repo.id}/retire`, { revision: 2, requestId: randomUUID() });
    assert.equal(retired.status, 200); assert.ok(retired.data.retiredAt);
    const task = await post('/tasks', { title: 'Needs new target', description: '' });
    assert.equal((await post('/tasks', { title: 'Cannot create selected', description: '', repositoryId: repo.id })).status, 409);
    assert.equal((await post(`/tasks/${task.data.id}/repository`, { repositoryId: repo.id, revision: 1 })).status, 409);
    assert.equal((await post(`/repositories/${repo.id}/retire`, { revision: 1, requestId: randomUUID() })).status, 409);
  } finally { latch.release(); latch.restore(); }
});

test('REP-02: a permitted edit during asynchronous preparation retries Git and snapshots the edited configuration', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-race-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = store.saveRepository(await validateRepository({ name: 'Before', path: checkout, targetBranch: 'main' }, () => false));
  const task = store.createTask({ title: 'Prepare after edit', description: '', repositoryId: repository.id });
  const refined = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Prepare', criteria: ['Use latest prefix'] });
  const launch = { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate' }], repository: { mode: 'new' as const } };
  const run = store.queue(task.id, { revision: refined.revision, launch }, defaultRoles);
  const work = new RepositoryWork(store, join(root, 'owned'));
  const scheduler = new CodingScheduler(store, work, new Evidence(store, work, join(root, 'evidence')),
    { busy: () => false, startScheduled: async () => ({}) } as unknown as RuntimeSupervisor, () => defaultRoles);
  const latch = gitLatch(root, 'rev-parse');
  t.after(async () => { latch.release(); latch.restore(); await scheduler.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  const claimed = store.claimCodingSlot(run.id, true);
  latch.arm();
  const preparing = scheduler.launch(claimed);
  await latch.blocked();
  const update = { name: 'After', path: checkout, targetBranch: 'main', branchPrefix: 'after/', checks: [], revision: 1, requestId: randomUUID() };
  const { revision, ...details } = update;
  store.updateRepository(repository.id, await validateRepository(details, () => false), update);
  latch.release();
  await preparing;
  assert.equal(store.repositoryWork(run.id)?.repository.name, 'After');
  assert.equal(store.repositoryWork(run.id)?.branch, `after/${run.id}`);
  assert.equal(store.getRun(run.id).launchHold, undefined, 'A raced edit must not require an operator to retry launch.');
});

test('REP-02: repeated configuration mismatches release the coding slot with a visible preparation hold', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-bounded-race-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = store.saveRepository(await validateRepository({ name: 'Bounded race', path: checkout, targetBranch: 'main' }, () => false));
  const task = store.createTask({ title: 'Bounded preparation', description: '', repositoryId: repo.id });
  const refined = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Bound the retry', criteria: ['Release slot on persistent mismatch'] });
  const run = store.queue(task.id, { revision: refined.revision, launch: { team: [
    { name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate' },
  ], repository: { mode: 'new' } } }, defaultRoles);
  class PersistentRaceWork extends RepositoryWork {
    attempts = 0;
    override async prepare(_runId: string, _input: unknown): Promise<never> {
      this.attempts++;
      throw new RepositoryConfigurationChanged('Repository configuration changed during preparation.');
    }
  }
  const work = new PersistentRaceWork(store, join(root, 'owned'));
  const scheduler = new CodingScheduler(store, work, new Evidence(store, work, join(root, 'evidence')),
    { busy: () => false, startScheduled: async () => { throw new Error('No admission is allowed after a persistent mismatch.'); } } as unknown as RuntimeSupervisor, () => defaultRoles);
  t.after(async () => scheduler.close());
  await scheduler.launch(store.claimCodingSlot(run.id, true));
  assert.equal(work.attempts, 2, 'One immediate retry, never an unbounded re-kick.');
  assert.equal(store.codingSlot(), undefined, 'A held plan cannot monopolize the coding slot.');
  assert.match(store.codingQueue().plans[0].launchHold?.reason ?? '', /configuration changed again.*Save launch details/i);
  assert.equal(store.codingQueue().plans[0].launchHold?.source, 'preparation');
});

test('REP-02: research queue refuses a source pinned before a repository edit, then retries from current Git', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-research-pin-race-'));
  const first = join(root, 'first'), second = join(root, 'second'); seedRepository(first); seedRepository(second);
  const server = await startServer({ port: 0, dataDir: join(root, 'server'), serveUi: false, rolePath: join(root, 'roles.json') });
  const latch = gitLatch(root, 'rev-parse');
  t.after(async () => { latch.release(); latch.restore(); await server.close(); rmSync(root, { recursive: true, force: true }); });
  const post = async (path: string, body: unknown) => {
    const response = await fetch(`${server.origin}/api${path}`, { method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    return { status: response.status, data: await response.json() };
  };
  const repo = (await post('/repositories', { name: 'Research source', path: first, targetBranch: 'main' })).data;
  const task = (await post('/tasks', { title: 'Pinned design', description: '', repositoryId: repo.id })).data;
  const refined = (await post(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Read Git', criteria: ['Pin current checkout'] })).data;
  const request = { revision: refined.revision, requestId: randomUUID() };
  latch.arm();
  try {
    const pending = post(`/tasks/${task.id}/research-runs`, request);
    await latch.blocked();
    const updated = await post(`/repositories/${repo.id}`, { name: 'Research source', path: second, targetBranch: 'main', checks: [], revision: 1, requestId: randomUUID() });
    assert.equal(updated.status, 200);
    latch.release();
    const stale = await pending;
    assert.equal(stale.status, 409); assert.match(stale.data.error, /repository changed during Git preflight/i);
    const workspace = await (await fetch(`${server.origin}/api/workspace`)).json();
    assert.equal(workspace.runs.some((run: { taskId: string }) => run.taskId === task.id), false);
    const retried = await post(`/tasks/${task.id}/research-runs`, request);
    assert.equal(retried.status, 201);
    assert.equal(retried.data.researchSource.repositoryRevision, 2);
    assert.equal(retried.data.researchSource.path, updated.data.path);
  } finally { latch.release(); }
});
