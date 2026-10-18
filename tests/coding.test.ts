import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { saveRepositoryFixture } from './fixtures/repository';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { Coding } from '../server/coding';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

test('LIM-01/EVD-03: one logical implementation responsibility gets at most two automatic repairs', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-evidence-')); const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'sum.py'), 'def sum_values(xs): return sum(xs)\n'); git('add', '.'); git('commit', '-m', 'fixture');
  const store = new Workbench(join(root, 'db')); t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = store.createTask({ title: 'Sum empty inputs', description: '', repositoryId: repo.id }); store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Support zero', criteria: ['Empty input returns zero'] }); const run = store.queue(task.id, { revision: 2 });
  const work = new RepositoryWork(store, join(root, 'owned')); const owned = (await work.prepare(run.id, { mode: 'new' }));
  const role = { ...defaultRoles[1], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const writer = store.admitAssignment(run.id, { name: 'Writer', role, model, coordinator: false, input: 'Implement' });
  const reviewer = store.admitAssignment(run.id, { name: 'Reviewer', role: { ...role, id: 'reviewer' }, model, coordinator: false, input: 'Review independently' });
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role: { ...role, id: 'coordinator' }, model, coordinator: true, input: 'Coordinate work' });
  for (const [i, item] of [writer, reviewer, coordinator].entries()) { store.claimAttempt(item.attempt.id); store.recordAttemptIdentity(item.attempt.id, { hostPid: i + 1, ompPid: i + 3, sessionId: `fixture-${i}`, hostUrl: `http://127.0.0.1:${5000 + i}`, pane: `%${i}` }); }
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const evidence = new Evidence(store, work, join(root, 'artifacts'));
  evidence.configure(run.id, []); const coding = new Coding(store, work, evidence);
  const current = (assignmentId: string) => store.runtime(run.id).assignments.find(item => item.id === assignmentId)!.currentAttemptId;
  for (let round = 0; round < 3; round++) {
    const activeWriter = current(writer.assignment.id); const activeCoordinator = current(coordinator.assignment.id);
    const claim = (await coding.claim(activeCoordinator, `claim-${round}`, { assignee: 'Writer', instruction: 'Implement or repair the accepted brief.' }));
    assert.equal(claim.writerAttemptId, activeWriter);
    assert.equal((await coding.claim(activeCoordinator, `claim-${round}`, { assignee: 'Writer', instruction: 'Implement or repair the accepted brief.' })).id, claim.id);
    assert.equal(store.runtime(run.id).assignments.find(item => item.id === writer.assignment.id)!.automaticCycles ?? 0, round);
    const { sha256: expectedHash } = (await work.readFile(run.id, 'sum.py'));
    (await coding.edit(activeWriter, `edit-${round}`, { path: 'sum.py', expectedHash, content: `def sum_values(xs): return ${round}\n` }));
    const completion = (await coding.requestCompletion(activeWriter, `finish-${round}`, { summary: `Implementation round ${round}` }));
    store.beginPause(run.id); for (const attempt of store.runtime(run.id).attempts) store.recordOwnership(attempt.id, true); store.finishPause(run.id); await work.reconcileStopped(run.id);
    const candidate = await coding.complete(run.id, completion.id);
    const next = store.runtime(run.id).assignments.map(assignment => store.replaceAttempt(assignment.id, { automatic: false }));
    store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
    for (const [i, attempt] of next.entries()) { store.claimAttempt(attempt.id); store.recordAttemptIdentity(attempt.id, { hostPid: i + 10, ompPid: i + 20, sessionId: `round-${round}-${i}`, hostUrl: `http://127.0.0.1:${6000 + i}`, pane: `%${i}` }); }
    store.startExecution(run.id);
    evidence.assignReview(current(coordinator.assignment.id), `review-${round}`, { assignee: 'Reviewer', candidateId: candidate.id, instruction: 'Review the repaired candidate.' });
    evidence.review(current(reviewer.assignment.id), { candidateId: candidate.id, verdict: 'changes_requested', summary: 'The implementation still needs work.', findings: [{ severity: 'major', body: 'Cover all input lengths.' }], coverage: [] });
  }
  await assert.rejects(async () => (await coding.claim(current(coordinator.assignment.id), 'fourth', { assignee: 'Writer', instruction: 'Try again' })), /two|limit|exhausted/i);
  await assert.rejects(async () => (await coding.claim(current(coordinator.assignment.id), 'rename-bypass', { assignee: 'Reviewer', instruction: 'Take over the failed implementation' })), /two|limit|exhausted/i);
});

test('R2: specialists search the owned worktree or a pinned revision, and patch exact spans under writer and hash rules', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-tools-')); const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'sum.py'), 'def sum_values(xs):\n    return sum(xs)\n\ndef twice(xs):\n    return sum(xs) * 2\n');
  writeFileSync(join(checkout, '.gitignore'), 'secret.txt\n'); git('add', '.'); git('commit', '-m', 'fixture');
  const store = new Workbench(join(root, 'db')); t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = store.createTask({ title: 'Sum empty inputs', description: '', repositoryId: repo.id }); store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Support zero', criteria: ['Empty input returns zero'] }); const run = store.queue(task.id, { revision: 2 });
  const work = new RepositoryWork(store, join(root, 'owned')); const owned = await work.prepare(run.id, { mode: 'new' });
  const role = { ...defaultRoles[1], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const writer = store.admitAssignment(run.id, { name: 'Writer', role, model, coordinator: false, input: 'Implement' });
  const reviewer = store.admitAssignment(run.id, { name: 'Reviewer', role: { ...role, id: 'reviewer' }, model, coordinator: false, input: 'Review' });
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role: { ...role, id: 'coordinator' }, model, coordinator: true, input: 'Coordinate' });
  for (const [i, item] of [writer, reviewer, coordinator].entries()) { store.claimAttempt(item.attempt.id); store.recordAttemptIdentity(item.attempt.id, { hostPid: i + 1, ompPid: i + 3, sessionId: `fixture-${i}`, hostUrl: `http://127.0.0.1:${5000 + i}`, pane: `%${i}` }); }
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const evidence = new Evidence(store, work, join(root, 'artifacts')); evidence.configure(run.id, []); const coding = new Coding(store, work, evidence);
  const pinned = (await work.status(run.id)).revision;
  await coding.claim(coordinator.attempt.id, 'claim', { assignee: 'Writer', instruction: 'Implement the brief.' });
  writeFileSync(join(owned.path, 'notes.md'), 'sum_values is documented here\n'); writeFileSync(join(owned.path, 'secret.txt'), 'sum_values secret\n');

  const found = await work.search(run.id, { query: 'sum_values' });
  assert.deepEqual(found.matches.map(item => [item.path, item.line]), [['notes.md', 1], ['sum.py', 1]], 'Untracked, non-ignored files are searched; ignored files are not.');
  assert.deepEqual((await work.search(run.id, { query: 'return sum\\(xs\\)( \\*)?', regex: true, path: 'sum.py' })).matches.map(item => item.line), [2, 5]);
  assert.equal((await work.search(run.id, { query: 'absent text' })).matches.length, 0);
  await assert.rejects(work.search(run.id, { query: 'x', path: '../operator' }), /outside repository scope/);
  await assert.rejects(work.search(run.id, { query: 'x', path: ':(top)' }), /plain repository paths/);
  // Paths are literal (no glob), per-file overflow is reported, and repository grep settings cannot change the output format.
  writeFileSync(join(owned.path, 'a[1].txt'), 'needle\n'); writeFileSync(join(owned.path, 'a1.txt'), 'needle\n'.repeat(120));
  execFileSync('git', ['-C', owned.path, 'config', 'grep.column', 'true']);
  assert.deepEqual((await work.search(run.id, { query: 'needle', path: 'a[1].txt' })).matches.map(item => [item.path, item.text]), [['a[1].txt', 'needle']]);
  const many = await work.search(run.id, { query: 'needle', path: 'a1.txt' });
  assert.equal(many.matches.length, 100); assert.equal(many.truncated, true);

  const { sha256 } = await work.readFile(run.id, 'sum.py');
  await assert.rejects(coding.patch(writer.attempt.id, 'ambiguous', { path: 'sum.py', expectedHash: sha256, replacements: [{ oldText: 'return sum(xs)', newText: 'return 0' }] }), /more than one/);
  await assert.rejects(coding.patch(writer.attempt.id, 'stale', { path: 'sum.py', expectedHash: 'a'.repeat(64), replacements: [{ oldText: 'twice', newText: 'double' }] }), /changed since it was read/);
  await assert.rejects(coding.patch(reviewer.attempt.id, 'not-writer', { path: 'sum.py', expectedHash: sha256, replacements: [{ oldText: 'twice', newText: 'double' }] }), /current writer/);
  const request = { path: 'sum.py', expectedHash: sha256, replacements: [{ oldText: '    return sum(xs)\n\n', newText: '    return sum(xs) if xs else 0\n\n' }, { oldText: 'def twice', newText: 'def double' }] };
  const applied = await coding.patch(writer.attempt.id, 'patch-1', request);
  const after = await work.readFile(run.id, 'sum.py');
  assert.equal(after.content, 'def sum_values(xs):\n    return sum(xs) if xs else 0\n\ndef double(xs):\n    return sum(xs) * 2\n');
  assert.equal(applied.sha256, after.sha256);
  assert.equal((await coding.patch(writer.attempt.id, 'patch-1', request)).operationId, applied.operationId, 'A replayed tool call returns the recorded edit.');
  await assert.rejects(coding.patch(writer.attempt.id, 'patch-1', { ...request, replacements: [{ oldText: 'double', newText: 'triple' }] }), /different content/);
  assert.deepEqual(store.codingRecord(run.id)?.edits?.map(item => item.state), ['applied'], 'Rejected patches never reach the edit ledger.');

  // A reviewer's search of a pinned revision sees that revision, not the writer's live edits.
  assert.deepEqual((await work.search(run.id, { query: 'def twice' }, pinned)).matches.map(item => item.path), ['sum.py']);
  assert.equal((await work.search(run.id, { query: 'def twice' })).matches.length, 0);
});
