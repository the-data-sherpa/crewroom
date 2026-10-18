import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { defaultRoles } from '../server/roles';
import type { PublicationRecord } from '../shared/publication';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-dependencies-'));
  seedRepository(root);
  const db = join(root, 'workflow.sqlite');
  const stores = [new Workbench(db)]; const store = stores[0];
  const reopen = () => { const next = new Workbench(db); stores.push(next); return next; };
  t.after(() => { stores.forEach(item => item.close()); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = (title: string) => {
    const created = store.createTask({ title, description: '', repositoryId: repo.id });
    return store.refine(created.id, { revision: created.revision, sourceRevision: 1, brief: title, criteria: ['Accepted'] });
  };
  const queue = (id: string) => store.queue(id, { revision: store.getTask(id).revision, requestId: randomUUID(),
    launch: { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' } } }, defaultRoles);
  const git = (...args: string[]) => execFileSync('git', ['-C', root, ...args], { encoding: 'utf8' }).trim();
  return { root, store, reopen, task, queue, git };
}

function publication(runId: string, root: string): PublicationRecord {
  return { id: randomUUID(), runId, attemptId: randomUUID(), candidateId: randomUUID(), repositoryPath: root,
    repository: { uuid: `{${randomUUID()}}`, workspace: 'fixture', slug: 'repo' }, branch: 'feature/a', target: 'main',
    revision: { base: 'a'.repeat(40), head: 'b'.repeat(40), tree: 'c'.repeat(40), worktreeHash: 'd'.repeat(64), dirty: false,
      requirementsHash: 'e'.repeat(64), instructionsHash: 'f'.repeat(64) }, title: 'A', description: 'Review',
    policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1, providerState: 'OPEN' },
    ready: false, blockers: ['CI pending'] };
}

test('QUE-04: declaration rejects self, cycles, unknown and stale revision; replay and Jira reimport preserve local graph', t => {
  const f = fixture(t); const a = f.task('A'), b = f.task('B'), c = f.task('C');
  const other = f.reopen(); const requestId = randomUUID();
  const change = { requestId, dependencyRevision: b.dependencyRevision ?? 0, dependsOn: [a.id] };
  const saved = f.store.setDependencies(b.id, change);
  assert.equal(saved.revision, b.revision); assert.equal(saved.dependencyRevision, 1);
  assert.deepEqual(other.setDependencies(b.id, change), saved);
  assert.throws(() => other.setDependencies(b.id, { ...change, dependsOn: [c.id] }), /different content/);
  assert.throws(() => other.setDependencies(b.id, { dependencyRevision: 0, dependsOn: [], requestId: randomUUID() }), /changed/);
  assert.throws(() => f.store.setDependencies(a.id, { dependencyRevision: 0, dependsOn: [a.id], requestId: randomUUID() }), /itself/);
  assert.throws(() => f.store.setDependencies(a.id, { dependencyRevision: 0, dependsOn: [randomUUID()], requestId: randomUUID() }), /not found/);
  f.store.setDependencies(c.id, { dependencyRevision: 0, dependsOn: [b.id], requestId: randomUUID() });
  assert.throws(() => f.store.setDependencies(a.id, { dependencyRevision: 0, dependsOn: [c.id], requestId: randomUUID() }), /cycle/);
  const batch = { siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '123', key: 'SIM-123', fields: { summary: 'Imported', description: 'Original', status: { name: 'Open' } } }] };
  const [jira] = f.store.importJira(batch);
  const linked = f.store.setDependencies(jira.id, { dependencyRevision: 0, dependsOn: [a.id], requestId: randomUUID() });
  f.store.importJira({ ...batch, issues: [{ ...batch.issues[0], fields: { ...batch.issues[0].fields, summary: 'Changed in Jira' } }] });
  assert.deepEqual(f.reopen().getTask(jira.id).dependsOn, [a.id]);
  assert.equal(f.store.getTask(jira.id).revision, linked.revision);
  assert.equal(linked.dependencyRevision, 1);
});

test('QUE-04: only observed merge releases; same repository requires checkout target containing merge before worktree', async t => {
  const f = fixture(t); const a = f.task('A'), c = f.task('C');
  const source = f.queue(a.id); const dependent = f.queue(c.id);
  f.store.setDependencies(c.id, { dependencyRevision: 0, dependsOn: [a.id], requestId: randomUUID() });
  const current = () => f.store.codingQueue().plans.find(item => item.id === dependent.id)!.holdReason!;
  assert.match(current(), /coding run has no PR yet/); assert.ok(current().includes(a.key));
  assert.match(f.reopen().codingQueue().plans.find(item => item.id === dependent.id)!.holdReason!, /coding run has no PR yet/);
  assert.equal(f.store.claimNextCodingSlot()?.id, source.id);
  // A local branch without a PR is not a delivery receipt.
  f.git('branch', 'unpublished-local-branch');
  assert.match(current(), /local branch or worktree cannot release/);
  assert.equal(f.store.publication(source.id), undefined);
  const pr = publication(source.id, f.root);
  f.store.savePublication(pr); assert.match(current(), /PR open/);
  f.store.savePublication({ ...pr, ready: true, blockers: [] }); assert.match(current(), /ready for review.*not merged/);
  f.store.savePublication({ ...pr, pr: { ...pr.pr, providerState: 'DECLINED' } }); assert.match(current(), /PR closed without merge/);
  f.store.savePublication({ ...pr, pr: { ...pr.pr, providerState: undefined } }); assert.match(current(), /provider.*unknown/);
  const work = new RepositoryWork(f.store, join(f.root, 'worktrees'));
  const merge = f.git('rev-parse', 'HEAD');
  f.store.savePublication({ ...pr, merged: { commit: merge, observedAt: new Date().toISOString() }, pr: { ...pr.pr, providerState: 'MERGED' } });
  assert.equal(current(), undefined, 'Verified merge releases the provider hold.');
  assert.equal(await work.dependencyBaseHold(dependent.id), undefined);
  f.git('checkout', '-b', 'future');
  f.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '-m', 'merge');
  const futureMerge = f.git('rev-parse', 'HEAD');
  f.store.savePublication({ ...pr, merged: { commit: futureMerge, observedAt: new Date().toISOString() }, pr: { ...pr.pr, providerState: 'MERGED' } });
  assert.match((await work.dependencyBaseHold(dependent.id))!, /verified merge.*update the target branch/);
  await assert.rejects(work.prepare(dependent.id, { mode: 'new' }), /verified merge/);
  assert.equal(f.store.repositoryWork(dependent.id), undefined);
  f.git('checkout', 'main'); f.git('merge', '--ff-only', 'future');
  assert.equal(await work.dependencyBaseHold(dependent.id), undefined);
  assert.equal(f.reopen().eligibilityHold(dependent), undefined);
  assert.equal(f.store.claimNextCodingSlot()?.id, dependent.id);
  assert.equal((await work.prepare(dependent.id, { mode: 'new' })).base, futureMerge);
});

test('QUE-04: editing declarations after execution starts does not retroactively stop that run', async t => {
  const f = fixture(t); const a = f.task('Upstream'), c = f.task('Already started');
  const run = f.queue(c.id);
  f.store.claimCodingSlot(run.id);
  const work = await new RepositoryWork(f.store, join(f.root, 'worktrees')).prepare(run.id, { mode: 'new' });
  f.store.startExecution(run.id);
  f.store.setDependencies(c.id, { dependencyRevision: 0, dependsOn: [a.id], requestId: randomUUID() });
  assert.equal(f.store.getRun(run.id).status, 'running');
  assert.equal((await new RepositoryWork(f.store, join(f.root, 'worktrees')).prepare(run.id, { mode: 'new' })).path, work.path);
  assert.equal(work.state, 'ready');
});

test('REVIEW-P2: dependency edit between accepted brief and run adoption does not alter requirement identity', t => {
  const f = fixture(t); const upstream = f.task('Upstream'), dependent = f.task('Dependent');
  const run = f.queue(dependent.id);
  const edited = f.store.refine(dependent.id, { revision: dependent.revision, sourceRevision: 1, brief: 'Accepted new brief', criteria: ['Accepted'] });
  const saved = f.store.setDependencies(dependent.id, { dependencyRevision: 0, dependsOn: [upstream.id], requestId: randomUUID() });
  assert.equal(saved.revision, edited.revision);
  assert.equal(saved.dependencyRevision, 1);
  assert.equal(f.store.requirementRevisions(dependent.id).at(-1)?.revision, edited.revision);
  assert.equal(f.store.codingQueue().plans.find(plan => plan.id === run.id)?.holdReason, 'Requirements changed');
  const adopted = f.store.adoptRequirements(run.id, { revision: edited.revision, requestId: randomUUID() });
  assert.equal(adopted.taskRevision, edited.revision);
  assert.deepEqual(f.store.getTask(dependent.id).dependsOn, [upstream.id]);
  assert.equal(f.store.codingQueue().plans.find(plan => plan.id === run.id)?.holdReason?.includes('Requirements changed'), false);
});
