import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { execFileSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { test, type TestContext } from 'node:test';
import { Workbench } from '../server/workbench';
import { saveRepositoryFixture } from './fixtures/repository';
import { Coding } from '../server/coding';
import { Evidence } from '../server/evidence';
import { RepositoryWork } from '../server/repository-work';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { gitLatch } from './fixtures/git-latch';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-git-'));
  const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'answer.txt'), 'original\n'); writeFileSync(join(checkout, 'AGENTS.md'), 'Keep the patch scoped.\n');
  git('add', '.'); git('commit', '-m', 'fixture');
  let store = new Workbench(join(root, 'db'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const queue = (title: string) => { const task = store.createTask({ title, description: '', repositoryId: repository.id }); store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['Keep existing behavior'] }); return store.queue(task.id, { revision: 2 }); };
  const run = queue('Update answer');
  const work = () => new RepositoryWork(store, join(root, 'owned'));
  const actor = (runId = run.id, name = 'Implementer') => {
    const role = { ...defaultRoles[1], model: 'fixture/model', effort: 'low' as const };
    const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
    const admitted = store.admitAssignment(runId, { name, role, model, coordinator: false, input: 'Implement the brief.' });
    store.claimAttempt(admitted.attempt.id); store.recordAttemptIdentity(admitted.attempt.id, { hostPid: 1, ompPid: 2, sessionId: name, hostUrl: 'http://127.0.0.1:4000', pane: '%0' });
    store.setDesiredState(runId, 'running'); store.startExecution(runId); return admitted.attempt;
  };
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  return { root, checkout, git, queue, run, work, actor, get store() { return store; }, reopen() { store.close(); store = new Workbench(join(root, 'db')); } };
}

/** Delays every backend Git invocation (each starts with its content-filter probe) without changing its result. */
function slowGit(t: TestContext, delayMs: number) {
  const real = execFileSync('sh', ['-c', 'command -v git'], { encoding: 'utf8' }).trim();
  const dir = mkdtempSync(join(tmpdir(), 'crewroom-slow-git-'));
  writeFileSync(join(dir, 'git'), `#!/bin/sh\nif [ "$1" = config ] && [ "$2" = --includes ]; then sleep ${delayMs / 1000}; fi\nexec '${real}' "$@"\n`, { mode: 0o755 });
  const path = process.env.PATH; process.env.PATH = `${dir}:${path}`;
  t.after(() => { process.env.PATH = path; rmSync(dir, { recursive: true, force: true }); });
}

/**
 * Runs an operation while a 10 ms observer timer ticks; a synchronous Git call stalls it for the whole delay.
 * Real timers are the point: event-loop responsiveness is only observable against the platform clock.
 */
async function observed<T>(label: string, operation: () => T | Promise<T>) {
  let last = performance.now(), worst = 0, ticks = 0;
  const timer = setInterval(() => { const now = performance.now(); worst = Math.max(worst, now - last); last = now; ticks++; }, 10);
  try { return await operation(); }
  finally {
    clearInterval(timer); worst = Math.max(worst, performance.now() - last);
    assert.ok(ticks > 0 && worst < 150, `${label} blocked observers for ${Math.round(worst)} ms (${ticks} ticks)`);
  }
}

test('REP-01: isolated work preserves a dirty operator checkout and survives reopening without duplicate branches', async t => {
  const f = fixture(t); const base = f.git('rev-parse', 'HEAD');
  writeFileSync(join(f.checkout, 'answer.txt'), 'operator edits\n');
  const admitted = (await f.work().prepare(f.run.id, { mode: 'new' }));
  assert.equal(admitted.base, base); assert.notEqual(admitted.path, f.checkout);
  assert.equal(readFileSync(join(admitted.path, 'answer.txt'), 'utf8'), 'original\n');
  assert.equal(readFileSync(join(f.checkout, 'answer.txt'), 'utf8'), 'operator edits\n');
  assert.equal(f.git('branch', '--show-current'), 'main');
  assert.equal(admitted.instructions[0].body, 'Keep the patch scoped.\n');
  f.reopen(); assert.equal((await f.work().prepare(f.run.id, { mode: 'new' })).path, admitted.path);
  assert.equal(f.git('worktree', 'list', '--porcelain').split('worktree ').length - 1, 2);
});

test('REP-01: project branch naming survives restart and constrains new work', async t => {
  const f = fixture(t);
  f.git('branch', 'alternate');
  const repository = saveRepositoryFixture(f.store, { name: 'Project policy', path: f.checkout, targetBranch: 'alternate', branchPrefix: 'fix/crewroom/' });
  const task = f.store.createTask({ title: 'Repair the modal', description: '', repositoryId: repository.id });
  f.store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Keep modal actions reachable.', criteria: ['Actions are reachable with large content'] });
  const run = f.store.queue(task.id, { revision: 2 });
  f.reopen();
  const work = (await f.work().prepare(run.id, { mode: 'new' }));
  assert.match(work.branch, /^fix\/crewroom\//);
  assert.equal(f.git('branch', '--show-current'), 'main');
  f.reopen();
  assert.equal((await f.work().prepare(run.id, { mode: 'new' })).branch, work.branch);
  assert.throws(() => saveRepositoryFixture(f.store, { name: 'Bad policy', path: f.checkout, targetBranch: 'main', branchPrefix: '../escape/' }), /branchPrefix/);
});

test('REP-02/LIF-05: one writer retains interrupted edits and commits, and replacement waits for proven termination', async t => {
  const f = fixture(t); const work = (await f.work().prepare(f.run.id, { mode: 'new' }));
  const attempt = f.actor(); (await f.work().claimWriter(attempt.id));
  writeFileSync(join(work.path, 'answer.txt'), 'interrupted edits\n');
  await assert.rejects(async () => (await f.work().releaseWriter(attempt.id)), /stopped|ownership/i);
  f.reopen(); assert.equal((await f.work().status(f.run.id)).writer?.attemptId, attempt.id);
  f.store.beginPause(f.run.id); f.store.recordOwnership(attempt.id, false, 'child ownership unknown');
  await assert.rejects(f.work().reconcileStopped(f.run.id), /ownership/i);
  f.store.recordOwnership(attempt.id, true); f.store.finishPause(f.run.id);
  await f.work().reconcileStopped(f.run.id);
  const saved = (await f.work().status(f.run.id)); assert.equal(saved.writer, undefined);
  assert.ok(saved.checkpoints.some(point => point.reason === 'interrupted' && point.revision.dirty));
  assert.equal(readFileSync(join(work.path, 'answer.txt'), 'utf8'), 'interrupted edits\n');
  execFileSync('git', ['-C', work.path, 'commit', '-am', 'retained commit']);
  assert.equal((await f.work().status(f.run.id)).drift.head, true);
  assert.equal(f.git('rev-parse', 'main'), work.base);
});

test('REP-03: explicit branch continuation rejects checked-out branches and changed heads', async t => {
  const f = fixture(t); const head = f.git('rev-parse', 'HEAD');
  await assert.rejects(async () => (await f.work().prepare(f.run.id, { mode: 'continue', branch: 'main', expectedHead: head })), /checked out/i);
  f.git('branch', 'existing-work');
  await assert.rejects(async () => (await f.work().prepare(f.run.id, { mode: 'continue', branch: 'existing-work', expectedHead: 'a'.repeat(40) })), /head/i);
  await assert.rejects(async () => (await f.work().prepare(f.run.id, { mode: 'continue', branch: 'existing-work' })), /expectedHead/i);
  const work = (await f.work().prepare(f.run.id, { mode: 'continue', branch: 'existing-work', expectedHead: head }));
  assert.equal(work.branch, 'existing-work'); assert.equal(work.startHead, head);
});

test('REP-04/EVD-04: base, working content and requirement drift are separate recorded facts', async t => {
  const f = fixture(t); const work = (await f.work().prepare(f.run.id, { mode: 'new' }));
  let timerFired = false;
  setImmediate(() => { timerFired = true; });
  const clean = await f.work().status(f.run.id);
  assert.deepEqual(clean.drift, { base: false, head: false, worktree: false, requirements: false, instructions: false });
  assert.equal(timerFired, true, 'The uncached launch gate must not block other supervisor work.');
  writeFileSync(join(work.path, 'answer.txt'), 'candidate\n');
  assert.equal((await f.work().status(f.run.id)).drift.worktree, true);
  writeFileSync(join(f.checkout, 'new.txt'), 'upstream\n'); f.git('add', '.'); f.git('commit', '-m', 'upstream');
  assert.equal((await f.work().status(f.run.id)).drift.base, true);
  const task = f.store.getTask(f.run.taskId); f.store.refine(task.id, { revision: task.revision, sourceRevision: task.sourceRevision, brief: 'Changed requirement', criteria: task.criteria });
  assert.equal((await f.work().status(f.run.id)).drift.requirements, true);
  writeFileSync(join(work.path, 'AGENTS.md'), 'Different rules\n');
  assert.equal((await f.work().status(f.run.id)).drift.instructions, true);
});

test('REP-02: competing writer attempts are rejected across reopened controllers', async t => {
  const f = fixture(t); (await f.work().prepare(f.run.id, { mode: 'new' }));
  const first = f.actor(); (await f.work().claimWriter(first.id));
  // Admit another specialist at the normal stopped coordination boundary.
  f.store.beginPause(f.run.id); f.store.recordOwnership(first.id, true); f.store.finishPause(f.run.id);
  const replacement = f.store.replaceAttempt(first.assignmentId, { automatic: false });
  f.store.prepareContinuation(f.run.id, f.store.getRun(f.run.id).controlRevision!);
  f.store.claimAttempt(replacement.id); f.store.recordAttemptIdentity(replacement.id, { hostPid: 3, ompPid: 4, sessionId: 'replacement', hostUrl: 'http://127.0.0.1:4001', pane: '%1' }); f.store.startExecution(f.run.id);
  f.reopen(); await assert.rejects(async () => (await f.work().claimWriter(replacement.id)), /writer/i);
  (await f.work().releaseWriter(first.id)); (await f.work().claimWriter(replacement.id));
  assert.equal((await f.work().status(f.run.id)).writer?.attemptId, replacement.id);
});

test('REP-01/LIF-05: a completed Git mutation with no saved receipt is adopted from durable admission', async t => {
  const f = fixture(t); const original = (await f.work().prepare(f.run.id, { mode: 'new' }));
  // Replay the exact pre-receipt SQLite boundary in a second disposable run.
  const next = f.queue('Second isolated effort');
  const record = { ...original, runId: next.id, path: join(dirname(original.path), next.id), branch: `codex/crewroom/${next.id}`, state: 'preparing' as const, gitFile: undefined, instructions: [], checkpoints: [] };
  f.store.reserveRepositoryWork(record);
  f.git('update-ref', `refs/crewroom/${next.id}/start`, record.startHead);
  f.git('worktree', 'add', '-b', record.branch, record.path, record.startHead);
  f.reopen(); const adopted = (await f.work().prepare(next.id, { mode: 'new' }));
  assert.equal(adopted.path, record.path); assert.equal(adopted.state, 'ready');
  assert.equal(f.git('worktree', 'list', '--porcelain').split('worktree ').length - 1, 3);
});

test('SEC-02: Git pointers and unsafe continuation refs fail closed', async t => {
  const f = fixture(t);
  await assert.rejects(async () => (await f.work().prepare(f.run.id, { mode: 'continue', branch: '--detach', expectedHead: f.git('rev-parse', 'HEAD') })), /Git|branch/i);
  const work = (await f.work().prepare(f.run.id, { mode: 'new' }));
  const pointer = readFileSync(join(work.path, '.git'), 'utf8');
  writeFileSync(join(work.path, '.git'), `gitdir: ${join(f.checkout, '.git')}\n`);
  await assert.rejects(async () => (await f.work().status(f.run.id)), /metadata/i);
  writeFileSync(join(work.path, '.git'), pointer);
  assert.equal((await f.work().status(f.run.id)).revision.dirty, false);
});

test('REP-02: another coding ticket cannot claim an agent while prior process ownership is held or uncertain', async t => {
  const f = fixture(t); (await f.work().prepare(f.run.id, { mode: 'new' })); const first = f.actor();
  const second = f.queue('Competing ticket'); (await f.work().prepare(second.id, { mode: 'new' }));
  assert.throws(() => f.actor(second.id), /one coding ticket/);
  f.store.recordOwnership(first.id, false, 'Unconfirmed descendants');
  const pending = f.store.runtime(second.id).attempts[0];
  assert.throws(() => f.store.claimAttempt(pending.id), /one coding ticket/);
  f.store.recordOwnership(first.id, true); f.store.claimAttempt(pending.id);
});

test('SEC-02: repository hooks and content filters do not run during isolated admission', async t => {
  const f = fixture(t); const marker = join(f.root, 'unexpected-hook');
  writeFileSync(join(f.checkout, '.gitattributes'), '*.txt filter=trip\n'); f.git('add', '.gitattributes'); f.git('commit', '-m', 'attributes');
  const hook = join(f.checkout, '.git', 'hooks', 'post-checkout');
  writeFileSync(hook, `#!/bin/sh\ntouch '${marker}'\n`, { mode: 0o755 });
  writeFileSync(join(f.root, 'included-config'), `[filter \"trip\"]\n  smudge = touch ${marker}\n`);
  f.git('config', 'include.path', join(f.root, 'included-config'));
  f.git('cat-file', '--filters', 'HEAD:answer.txt');
  assert.equal(readFileSync(marker, 'utf8'), ''); rmSync(marker);
  (await f.work().prepare(f.run.id, { mode: 'new' }));
  assert.throws(() => readFileSync(marker), /ENOENT/);
});

test('EVD-01/SEC-02: repository edits require the current writer and compare the previous file hash', async t => {
  const f = fixture(t); const owned = (await f.work().prepare(f.run.id, { mode: 'new' })); const writer = f.actor();
  await assert.rejects(async () => (await f.work().writeFile(writer.id, { path: 'answer.txt', expectedHash: null, content: 'wrong' })), /writer/i);
  (await f.work().claimWriter(writer.id));
  const original = (await f.work().readFile(f.run.id, 'answer.txt'));
  (await f.work().writeFile(writer.id, { path: 'answer.txt', expectedHash: original.sha256, content: 'candidate\n' }));
  assert.equal(readFileSync(join(owned.path, 'answer.txt'), 'utf8'), 'candidate\n');
  await assert.rejects(async () => (await f.work().writeFile(writer.id, { path: 'answer.txt', expectedHash: original.sha256, content: 'stale edit' })), /changed/i);
  await assert.rejects(async () => (await f.work().writeFile(writer.id, { path: '../operator/answer.txt', expectedHash: null, content: 'escaped' })), /path|scope/i);
  await assert.rejects(async () => (await f.work().writeFile(writer.id, { path: '.git', expectedHash: null, content: 'gitdir: /elsewhere' })), /path|scope/i);
  f.store.recordOwnership(writer.id, true);
  await assert.rejects(async () => (await f.work().writeFile(writer.id, { path: 'new.txt', expectedHash: null, content: 'after stop' })), /writer/i);
});


test('EVD-01/LIF-06: a repeated file operation adopts its written content after a lost receipt', async t => {
  const f = fixture(t); (await f.work().prepare(f.run.id, { mode: 'new' })); const writer = f.actor(); (await f.work().claimWriter(writer.id));
  new Evidence(f.store, f.work(), join(f.root, 'artifacts')).configure(f.run.id, []);
  let coding = new Coding(f.store, f.work());
  const input = { path: 'answer.txt', expectedHash: (await f.work().readFile(f.run.id, 'answer.txt')).sha256, content: 'One recorded edit\n' };
  const first = (await coding.edit(writer.id, 'tool-1', input));
  assert.deepEqual((await coding.edit(writer.id, 'tool-1', input)), first);
  await assert.rejects(async () => (await coding.edit(writer.id, 'tool-1', { ...input, content: 'Different request' })), /different/i);
  f.store.updateCodingRecord(f.run.id, record => ({ ...record!, edits: record!.edits!.map(edit => ({ ...edit, state: 'pending' as const })) }));
  f.reopen(); coding = new Coding(f.store, f.work());
  assert.deepEqual((await coding.edit(writer.id, 'tool-1', input)), first);
  assert.equal(f.store.codingRecord(f.run.id)!.edits!.length, 1);
  assert.equal((await f.work().readFile(f.run.id, 'answer.txt')).content, input.content);
});

test('EVD-01/LIF-06: completion waits for stopped ownership and adopts one exact commit after a lost receipt', async t => {
  const f = fixture(t); (await f.work().prepare(f.run.id, { mode: 'new' })); const writer = f.actor(); (await f.work().claimWriter(writer.id));
  const evidence = new Evidence(f.store, f.work(), join(f.root, 'artifacts')); evidence.configure(f.run.id, []);
  const coding = new Coding(f.store, f.work(), evidence);
  const { sha256: expectedHash } = (await f.work().readFile(f.run.id, 'answer.txt'));
  (await coding.edit(writer.id, 'edit', { path: 'answer.txt', expectedHash, content: 'Finished implementation\n' }));
  const seal = (await coding.requestCompletion(writer.id, 'finish', { summary: 'Finish the accepted change' }));
  await assert.rejects(coding.complete(f.run.id, seal.id), /stopped|pause/i);
  f.store.beginPause(f.run.id); f.store.recordOwnership(writer.id, true); f.store.finishPause(f.run.id); await f.work().reconcileStopped(f.run.id);
  const candidate = await coding.complete(f.run.id, seal.id);
  assert.equal(candidate.revision.dirty, false);
  assert.notEqual(candidate.revision.head, seal.head);
  const firstCount = f.git('rev-list', '--count', (await f.work().status(f.run.id)).branch);
  f.store.updateCodingRecord(f.run.id, record => ({ ...record!, completions: record!.completions!.map(item => ({ ...item, state: 'requested' as const, candidateId: undefined })) }));
  assert.equal((await coding.complete(f.run.id, seal.id)).revision.head, candidate.revision.head);
  assert.equal(f.git('rev-list', '--count', (await f.work().status(f.run.id)).branch), firstCount);
  assert.equal((await f.work().status(f.run.id)).drift.head, false);
});

test('EVD-02: checks receive a separate source snapshot for the exact clean candidate', async t => {
  const f = fixture(t); const owned = (await f.work().prepare(f.run.id, { mode: 'new' }));
  const revision = (await f.work().status(f.run.id)).revision;
  const copy = (await f.work().exportRevision(f.run.id, revision, join(f.root, 'check-source')));
  assert.equal(readFileSync(join(copy, 'answer.txt'), 'utf8'), 'original\n');
  assert.throws(() => readFileSync(join(copy, '.git')), /ENOENT/);
  writeFileSync(join(copy, 'answer.txt'), 'check-local mutation');
  assert.equal(readFileSync(join(owned.path, 'answer.txt'), 'utf8'), 'original\n');
  writeFileSync(join(owned.path, 'answer.txt'), 'uncommitted drift');
  await assert.rejects(async () => (await f.work().exportRevision(f.run.id, revision, join(f.root, 'stale-check'))), /revision|changed/i);
});

test('EVD-04: assigned review reads a pinned revision while later working edits remain separate', async t => {
  const f = fixture(t); const owned = (await f.work().prepare(f.run.id, { mode: 'new' })); const revision = (await f.work().status(f.run.id)).revision;
  writeFileSync(join(owned.path, 'answer.txt'), 'later uncommitted edit\n');
  const observed = (await f.work().readRevision(f.run.id, revision, 'answer.txt'));
  assert.equal(observed.content, 'original\n'); assert.equal(observed.head, revision.head);
  await assert.rejects(async () => (await f.work().readRevision(f.run.id, revision, '../operator/answer.txt')), /path|scope/i);
});

test('EVD-04: a continuation behind its selected base is stale before any new writer starts', async t => {
  const f = fixture(t); const original = f.git('rev-parse', 'HEAD'); f.git('branch', 'old-work');
  writeFileSync(join(f.checkout, 'upstream.txt'), 'new base'); f.git('add', '.'); f.git('commit', '-m', 'base update');
  (await f.work().prepare(f.run.id, { mode: 'continue', branch: 'old-work', expectedHead: original }));
  assert.equal((await f.work().status(f.run.id)).drift.base, true);
  const attempt = f.actor(); await assert.rejects(async () => (await f.work().claimWriter(attempt.id)), /changed|drift/i);
});

test('LIF-06: stopped writer reconciliation adopts an applied edit whose receipt was lost', async t => {
  const f = fixture(t); (await f.work().prepare(f.run.id, { mode: 'new' })); const attempt = f.actor();
  new Evidence(f.store, f.work(), join(f.root, 'artifacts')).configure(f.run.id, []);
  (await f.work().claimWriter(attempt.id)); const coding = new Coding(f.store, f.work());
  const { sha256: expectedHash } = (await f.work().readFile(f.run.id, 'answer.txt'));
  (await coding.edit(attempt.id, 'lost-edit', { path: 'answer.txt', expectedHash, content: 'retained applied change\n' }));
  f.store.updateCodingRecord(f.run.id, record => ({ ...record!, edits: record!.edits!.map(item => ({ ...item, state: 'pending' as const })) }));
  await assert.rejects(coding.reconcileStoppedEdits(f.run.id), /stopped|ownership/i);
  f.store.beginPause(f.run.id); f.store.recordOwnership(attempt.id, true); f.store.finishPause(f.run.id); await f.work().reconcileStopped(f.run.id);
  await coding.reconcileStoppedEdits(f.run.id);
  assert.equal(f.store.codingRecord(f.run.id)!.edits![0].state, 'applied');
  assert.equal((await f.work().readFile(f.run.id, 'answer.txt')).content, 'retained applied change\n');
});

test('R3: agent repository reads, writer claim, edit, completion and check snapshots keep observers running', async t => {
  const f = fixture(t); await f.work().prepare(f.run.id, { mode: 'new' }); const writer = f.actor();
  new Evidence(f.store, f.work(), join(f.root, 'artifacts')).configure(f.run.id, []);
  const work = f.work(); const coding = new Coding(f.store, work);
  slowGit(t, 250);
  const { revision } = await observed('status', () => work.status(f.run.id));
  await observed('file list', () => work.listFiles(f.run.id));
  const original = await observed('file read', () => work.readFile(f.run.id, 'answer.txt'));
  await observed('pinned review read', () => work.readRevision(f.run.id, revision, 'answer.txt'));
  await observed('candidate diff', () => work.diffRevision(f.run.id, revision));
  await observed('check source snapshot', () => work.exportRevision(f.run.id, revision, join(f.root, 'check-source')));
  await observed('writer claim', () => work.claimWriter(writer.id));
  await observed('edit', () => coding.edit(writer.id, 'edit', { path: 'answer.txt', expectedHash: original.sha256, content: 'candidate\n' }));
  const completion = await observed('completion request', () => coding.requestCompletion(writer.id, 'finish', { summary: 'Finish the change' }));
  assert.equal(completion.worktreeHash, (await work.status(f.run.id)).revision.worktreeHash);
});

test('R3/EVD-01: a writer stopped while its edit waits on Git cannot write the file', async t => {
  const f = fixture(t); const owned = await f.work().prepare(f.run.id, { mode: 'new' }); const writer = f.actor();
  await f.work().claimWriter(writer.id); const original = await f.work().readFile(f.run.id, 'answer.txt');
  const edit = f.work().writeFile(writer.id, { path: 'answer.txt', expectedHash: original.sha256, content: 'late edit\n' });
  // Pause confirms the stop while the edit's revision gate is still running Git.
  f.store.recordOwnership(writer.id, true);
  await assert.rejects(edit, /writer/i);
  assert.equal(readFileSync(join(owned.path, 'answer.txt'), 'utf8'), 'original\n');
});

test('R3/EVD-01: a pause recorded while completion waits on Git records no completion request', async t => {
  const f = fixture(t); await f.work().prepare(f.run.id, { mode: 'new' }); const writer = f.actor();
  new Evidence(f.store, f.work(), join(f.root, 'artifacts')).configure(f.run.id, []);
  await f.work().claimWriter(writer.id); const coding = new Coding(f.store, f.work());
  const request = coding.requestCompletion(writer.id, 'finish', { summary: 'Finish the change' });
  // Pause records intent at once; its stop is queued behind this request.
  f.store.setDesiredState(f.run.id, 'paused');
  await assert.rejects(request, /writer/i);
  assert.deepEqual(f.store.codingRecord(f.run.id)!.completions ?? [], []);
});

test('R3: a refinement while an edit waits on its ignore probe rejects the edit', async t => {
  const f = fixture(t); const owned = await f.work().prepare(f.run.id, { mode: 'new' }); const writer = f.actor();
  const latch = gitLatch(f.root, 'check-ignore'); t.after(() => latch.restore());
  await f.work().claimWriter(writer.id); const original = await f.work().readFile(f.run.id, 'answer.txt');
  latch.arm(); const edit = f.work().writeFile(writer.id, { path: 'answer.txt', expectedHash: original.sha256, content: 'obsolete edit\n' });
  await latch.blocked();
  const task = f.store.getTask(f.run.taskId);
  f.store.refine(task.id, { revision: task.revision, sourceRevision: task.sourceRevision, brief: task.brief, criteria: ['Keep existing behavior', 'Reject negative input'] });
  latch.release();
  await assert.rejects(edit, /requirements/i);
  assert.equal(readFileSync(join(owned.path, 'answer.txt'), 'utf8'), 'original\n');
});
