import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, realpathSync, renameSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { validateRepository } from '../server/repository-configuration';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { defaultRoles } from '../server/roles';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';

const check = { id: 'unit', image: `sha256:${'a'.repeat(64)}`, command: '/bin/sh', args: ['-c', 'exit 0'], timeoutMs: 1000, memoryMiB: 256, required: true };

test('REP-01/QUE-05: distinct repository targets are selectable; checks and prefix reach owned work and evidence', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-config-'));
  const first = join(root, 'first'), second = join(root, 'second');
  seedRepository(first); seedRepository(second);
  execFileSync('git', ['-C', second, 'update-ref', 'refs/remotes/origin/release', 'HEAD']);
  let store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const alpha = saveRepositoryFixture(store, { name: 'Alpha', path: first, targetBranch: 'main' });
  const beta = saveRepositoryFixture(store, { name: 'Beta', path: second, targetBranch: 'origin/release', branchPrefix: 'feature/crewroom/', checks: [check] });
  const one = store.createTask({ title: 'First task', description: '' });
  const two = store.createTask({ title: 'Second task', description: '' });
  store.assignRepository(one.id, { repositoryId: alpha.id, revision: one.revision });
  store.assignRepository(two.id, { repositoryId: beta.id, revision: two.revision });
  assert.deepEqual([store.getTask(one.id).repositoryId, store.getTask(two.id).repositoryId], [alpha.id, beta.id]);
  store.close(); store = new Workbench(join(root, 'workflow.sqlite'));
  assert.deepEqual(store.snapshot().repositories.find(item => item.id === beta.id)?.checks, [check]);
  store.refine(two.id, { revision: 2, sourceRevision: 1, brief: 'Implement release change', criteria: ['Check release result'] });
  const run = store.queue(two.id, { revision: 3 });
  const work = new RepositoryWork(store, join(root, 'owned'));
  const owned = await work.prepare(run.id, { mode: 'new' });
  assert.equal(owned.repository.id, beta.id);
  assert.equal(owned.branch, `feature/crewroom/${run.id}`);
  assert.equal(owned.targetRef, 'refs/remotes/origin/release');
  assert.throws(() => saveRepositoryFixture(store, { name: 'Owned result', path: owned.path, targetBranch: 'main' }), /Crewroom-owned linked worktree/i);
  const evidence = new Evidence(store, work, join(root, 'artifacts'));
  assert.deepEqual(evidence.configure(run.id, owned.repository.checks ?? []).requiredChecks, [check]);
});

test('REP-01: duplicate canonical path and target, invalid Git root and unresolvable target fail at save', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-config-reject-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  saveRepositoryFixture(store, { name: 'First', path: checkout, targetBranch: 'main' });
  const alias = join(root, 'alias'); symlinkSync(checkout, alias);
  assert.throws(() => saveRepositoryFixture(store, { name: 'Alias', path: alias, targetBranch: 'main' }), /already configured/i);
  assert.throws(() => saveRepositoryFixture(store, { name: 'Missing target', path: checkout, targetBranch: 'unknown' }), /Target branch must resolve/i);
  const plain = join(root, 'plain'); mkdirSync(plain);
  assert.throws(() => saveRepositoryFixture(store, { name: 'Not Git', path: plain, targetBranch: 'main' }), /Git root checkout/i);
  const nested = join(checkout, 'nested'); mkdirSync(nested);
  assert.throws(() => saveRepositoryFixture(store, { name: 'Nested', path: nested, targetBranch: 'main' }), /top level/i);
  const linked = join(root, 'linked'); execFileSync('git', ['-C', checkout, 'worktree', 'add', '--detach', linked]);
  const external = saveRepositoryFixture(store, { name: 'Independent linked checkout', path: linked, targetBranch: 'main' });
  assert.equal(external.targetBranch, 'main');
  assert.equal(store.snapshot().repositories.length, 2);
});

test('REP-01: requestId replays identical configuration across restart and rejects changed content', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-config-replay-')); const checkout = join(root, 'checkout'); seedRepository(checkout);
  const db = join(root, 'workflow.sqlite'); let store = new Workbench(db);
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const input = { name: 'Target', path: checkout, targetBranch: 'main', branchPrefix: 'feature/', checks: [check], requestId: randomUUID() };
  const created = saveRepositoryFixture(store, input);
  store.close(); store = new Workbench(db);
  assert.deepEqual(saveRepositoryFixture(store, input), created);
  execFileSync('git', ['-C', checkout, 'switch', '-c', 'replacement']);
  execFileSync('git', ['-C', checkout, 'branch', '-D', 'main']);
  assert.deepEqual(saveRepositoryFixture(store, input), created, 'A replay returns its record even when the saved target later moves.');
  assert.throws(() => saveRepositoryFixture(store, { ...input, name: 'Different' }), /requestId.*different/i);
  assert.equal(store.snapshot().repositories.length, 1);
});

test('REP-02: edits persist, replay once, reject duplicate targets and stale revisions without changing the row', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-edit-'));
  const first = join(root, 'first'), second = join(root, 'second'); seedRepository(first); seedRepository(second);
  const db = join(root, 'workflow.sqlite'); let store = new Workbench(db);
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Original', path: first, targetBranch: 'main', checks: [check] });
  const other = saveRepositoryFixture(store, { name: 'Other', path: second, targetBranch: 'main' });
  const update = async (input: { name: string; path: string; targetBranch: string; branchPrefix?: string; checks?: typeof check[]; revision: number; requestId: string }) => {
    const { revision, ...config } = input;
    return store.updateRepository(repo.id, await validateRepository(config, path => store.isOwnedRepositoryWorktree(path)), input);
  };
  const input = { name: 'Renamed', path: first, targetBranch: 'main', branchPrefix: 'feature/', checks: [check], revision: 1, requestId: randomUUID() };
  const saved = await update(input);
  assert.equal(saved.revision, 2);
  store.close(); store = new Workbench(db);
  assert.equal(store.snapshot().repositories.find(item => item.id === repo.id)?.name, 'Renamed');
  assert.deepEqual(store.repositoryUpdateReplay(repo.id, input), saved);
  assert.deepEqual(await update(input), saved);
  await assert.rejects(update({ ...input, name: 'Stale', requestId: randomUUID() }), /revision changed/i);
  await assert.rejects(update({ ...input, path: second, revision: 2, requestId: randomUUID() }), /already configured/i);
  assert.equal(store.snapshot().repositories.find(item => item.id === repo.id)?.name, 'Renamed');
  const clearedInput = { name: 'Renamed', path: first, targetBranch: 'main', checks: [check], revision: 2, requestId: randomUUID() };
  const cleared = await update(clearedInput);
  assert.equal(cleared.branchPrefix, undefined, 'Omitting the prefix restores the default instead of retaining the old field.');
  store.close(); store = new Workbench(db);
  assert.equal(store.snapshot().repositories.find(item => item.id === repo.id)?.branchPrefix, undefined);
  store.retireRepository(other.id, { revision: 1, requestId: randomUUID() });
  const reused = await update({ ...clearedInput, path: second, revision: 3, requestId: randomUUID() });
  assert.equal(reused.path, realpathSync(second), 'An edited active configuration can reuse a retired target.');
});

test('REP-02: open queued launch references guard checkout, target, referenced checks and retirement; terminal history permits edit', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-guard-'));
  const first = join(root, 'first'), second = join(root, 'second'); seedRepository(first); seedRepository(second);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Guarded', path: first, targetBranch: 'main', checks: [check] });
  const task = store.createTask({ title: 'Guard ticket', description: '', repositoryId: repo.id });
  const refined = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Check reference', criteria: ['Check the output'] });
  const launch = { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate', criteria: [{ index: 0, obligation: 'Check output', checkIds: ['unit'] }] }], repository: { mode: 'new' as const } };
  const run = store.queue(task.id, { revision: refined.revision, launch }, defaultRoles);
  const otherTask = store.createTask({ title: 'Another queued ticket', description: '', repositoryId: repo.id });
  const otherBrief = store.refine(otherTask.id, { revision: 1, sourceRevision: 1, brief: 'Protect other work', criteria: ['Keep it queued'] });
  const otherRun = store.queue(otherTask.id, { revision: otherBrief.revision });
  const update = async (value: { name: string; path: string; targetBranch: string; checks: typeof check[]; branchPrefix?: string; revision: number; requestId: string }) => {
    const { revision, ...config } = value;
    return store.updateRepository(repo.id, await validateRepository(config, path => store.isOwnedRepositoryWorktree(path)), value);
  };
  const base = { name: 'Guarded', path: first, targetBranch: 'main', checks: [check], revision: 1, requestId: randomUUID() };
  const allowed = await update({ ...base, name: 'Renamed', branchPrefix: 'new/', checks: [check, { ...check, id: 'lint' }] });
  assert.equal(allowed.revision, 2);
  const current = { ...base, name: 'Renamed', branchPrefix: 'new/', checks: [check, { ...check, id: 'lint' }], revision: 2 };
  for (const change of [{ path: second }, { targetBranch: 'release' }, { checks: [{ ...check, id: 'lint' }] }]) {
    if ('targetBranch' in change) execFileSync('git', ['-C', first, 'branch', 'release']);
    await assert.rejects(update({ ...current, ...change, requestId: randomUUID() }), new RegExp(task.key));
  }
  assert.throws(() => store.retireRepository(repo.id, { revision: 2, requestId: randomUUID() }), new RegExp(task.key));
  const refusal = await update({ ...current, path: second, requestId: randomUUID() }).then(() => '', (error: Error) => error.message);
  assert.match(refusal, new RegExp(task.key)); assert.match(refusal, new RegExp(otherTask.key));
  assert.equal(store.snapshot().repositories.find(item => item.id === repo.id)?.revision, 2);
  store.cancelQueued(run.id, { requestId: randomUUID() });
  assert.throws(() => store.retireRepository(repo.id, { revision: 2, requestId: randomUUID() }), new RegExp(otherTask.key));
  store.cancelQueued(otherRun.id, { requestId: randomUUID() });
  const changed = await update({ ...current, path: second, checks: [{ ...check, id: 'lint' }], requestId: randomUUID() });
  assert.equal(changed.path, realpathSync(second));
  assert.equal(store.getRun(run.id).taskSnapshot.repositoryId, repo.id);
});

test('REP-02: prepared repository stays immutable, later queued preparation reads edits, and retirement retains historical run', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-history-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Original', path: checkout, targetBranch: 'main', checks: [check] });
  const makeRun = (title: string) => {
    const task = store.createTask({ title, description: '', repositoryId: repo.id });
    const refined = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['Read checks'] });
    return store.queue(task.id, { revision: refined.revision });
  };
  const first = makeRun('Historical ticket');
  const work = new RepositoryWork(store, join(root, 'owned'));
  const prepared = await work.prepare(first.id, { mode: 'new' });
  const input = { name: 'Edited', path: checkout, targetBranch: 'main', branchPrefix: 'next/', checks: [{ ...check, id: 'lint' }], revision: 1, requestId: randomUUID() };
  const { revision, ...config } = input;
  store.updateRepository(repo.id, await validateRepository(config, path => store.isOwnedRepositoryWorktree(path)), input);
  assert.deepEqual(store.repositoryWork(first.id)?.repository, prepared.repository);
  store.cancelQueued(first.id, { requestId: randomUUID() });
  const second = makeRun('Later queued ticket');
  const next = await work.prepare(second.id, { mode: 'new' });
  assert.equal(next.repository.name, 'Edited'); assert.equal(next.repository.branchPrefix, 'next/');
  assert.deepEqual(next.repository.checks?.map(item => item.id), ['lint']);
  store.cancelQueued(second.id, { requestId: randomUUID() });
  const staleTask = store.createTask({ title: 'Selected before retirement', description: '', repositoryId: repo.id });
  const staleBrief = store.refine(staleTask.id, { revision: 1, sourceRevision: 1, brief: 'No run on retired target', criteria: ['Re-select'] });
  const retireRequest = { revision: 2, requestId: randomUUID() };
  const retired = store.retireRepository(repo.id, retireRequest);
  assert.ok(retired.retiredAt);
  assert.deepEqual(store.repositoryWork(first.id)?.repository, prepared.repository);
  assert.equal(store.getRun(first.id).status, 'cancelled');
  assert.deepEqual(store.retireRepository(repo.id, retireRequest), retired);
  const replacement = saveRepositoryFixture(store, { name: 'Replacement for future tickets', path: checkout, targetBranch: 'main' });
  assert.notEqual(replacement.id, repo.id);
  assert.equal(store.getRun(first.id).taskSnapshot.repositoryId, repo.id);
  assert.equal(store.repositoryWork(first.id)?.repository.id, repo.id);
  const task = store.createTask({ title: 'Must reselect', description: '' });
  assert.throws(() => store.assignRepository(task.id, { repositoryId: repo.id, revision: 1 }), /retired/i);
  assert.throws(() => store.createTask({ title: 'Cannot select', description: '', repositoryId: repo.id }), /retired/i);
  assert.throws(() => store.queue(staleTask.id, { revision: staleBrief.revision }), /retired/i);
  assert.throws(() => store.queueResearch(staleTask.id, { revision: staleBrief.revision, requestId: randomUUID() },
    { repositoryId: repo.id, path: checkout, commit: 'a'.repeat(40) }), /retired/i);
});

test('REP-02: a moved checkout behind its original symlink prepares without a false configuration race', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-relocation-'));
  const checkout = join(root, 'checkout'), moved = join(root, 'moved'); seedRepository(checkout);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Relocated checkout', path: checkout, targetBranch: 'main' });
  const task = store.createTask({ title: 'Run after relocation', description: '', repositoryId: repo.id });
  const refined = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Use moved Git root', criteria: ['Prepare one branch'] });
  const run = store.queue(task.id, { revision: refined.revision });
  renameSync(checkout, moved); symlinkSync(moved, checkout);
  const prepared = await new RepositoryWork(store, join(root, 'owned')).prepare(run.id, { mode: 'new' });
  assert.equal(prepared.repository.path, realpathSync(moved));
  assert.equal(prepared.repository.revision, repo.revision);
  assert.equal(store.getRun(run.id).launchHold, undefined);
});
