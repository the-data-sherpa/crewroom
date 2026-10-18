import assert from 'node:assert/strict';
import { test } from 'node:test';
import { randomUUID } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { cancelledFixture } from './fixtures/cancel-continuation';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { RunContinuation } from '../server/run-continuation';
import { resolveAssignment } from '../server/runtime/model-policy';
import { reviewerSummary } from './fixtures/publication';
import { TargetRefresh } from '../server/target-refresh';
import { runLabel } from '../shared/types';

test('CAN-02: schema-25 owned work migrates without losing history or the single current branch owner', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-migration-')); const f = await cancelledFixture(root);
  t.after(async () => { await f.close(); rmSync(root, { recursive: true, force: true }); }); await f.cancel();
  const original = f.store.repositoryWork(f.run.id); f.store.close();
  const db = new DatabaseSync(join(root, 'workflow.sqlite'));
  db.exec(`DROP INDEX current_branch_owner; ALTER TABLE repository_work RENAME TO newer_work;
    CREATE TABLE repository_work (run_id TEXT PRIMARY KEY REFERENCES runs(id), common_dir TEXT NOT NULL, branch TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(common_dir,branch));
    INSERT INTO repository_work SELECT * FROM newer_work; DROP TABLE newer_work; PRAGMA user_version=25;`); db.close();
  const reopened = new Workbench(join(root, 'workflow.sqlite'));
  try {
    assert.deepEqual(reopened.repositoryWork(f.run.id), original);
    const schema = new DatabaseSync(join(root, 'workflow.sqlite'));
    assert.equal(schema.prepare('PRAGMA user_version').get()!.user_version, 26); schema.close();
    const work = new RepositoryWork(reopened, join(root, 'owned'));
    const next = await new RunContinuation(reopened, work).queue(f.run.id, { requestId: randomUUID(), revision: 3, roleId: 'coordinator' }, f.roles);
    await work.prepare(next.id, next.launch!.repository);
    const check = new DatabaseSync(join(root, 'workflow.sqlite'));
    assert.equal(check.prepare("SELECT COUNT(*) AS n FROM repository_work WHERE json_extract(data,'$.transferredTo') IS NULL").get()!.n, 1);
    assert.equal(check.prepare('SELECT COUNT(*) AS n FROM repository_work').get()!.n, 2); check.close();
  } finally { reopened.close(); }
});

test('CAN-01/02/04: terminal cancel preserves dirty work; a keyed successor and atomic ownership survive reopen', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-')); const f = await cancelledFixture(root);
  t.after(async () => { await f.close(); rmSync(root, { recursive: true, force: true }); });
  f.store.setDesiredState(f.run.id, 'cancelled'); f.store.beginPause(f.run.id);
  assert.match(runLabel(f.store.getRun(f.run.id)), /Cancelling/);
  await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /confirmed/);
  assert.equal(f.store.finishPause(f.run.id).status, 'attention_required');
  assert.throws(() => f.store.setDesiredState(f.run.id, 'running'), /cancelled/);
  writeFileSync(join(f.owned.path, 'answer'), 'unfinished edits\n'); await f.cancel();
  const oldEvidence = f.store.codingRecord(f.run.id); const attempts = f.store.runtime(f.run.id).attempts;
  const request = f.request(); const [next, concurrent] = await Promise.all([f.continuation.queue(f.run.id, request, f.roles), f.continuation.queue(f.run.id, request, f.roles)]);
  assert.deepEqual(concurrent, next);
  assert.equal(next.position, 0); assert.equal(f.store.repositoryWork(next.id), undefined);
  assert.equal(f.store.getRun(f.run.id).continuedBy, next.id); assert.equal(next.continuedFrom?.runId, f.run.id);
  assert.deepEqual(await f.continuation.queue(f.run.id, request, f.roles), next);
  await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /already has/);
  await assert.rejects(f.continuation.queue(f.run.id, { ...request, roleId: 'reviewer' }, f.roles), /different content/);
  const reopened = new Workbench(join(root, 'workflow.sqlite')); const work = new RepositoryWork(reopened, join(root, 'owned'));
  try {
    assert.deepEqual(await new RunContinuation(reopened, work).queue(f.run.id, request, f.roles), next);
    const prepared = await work.prepare(next.id, next.launch!.repository);
    assert.equal(prepared.path, f.owned.path); assert.equal(prepared.branch, f.owned.branch);
    assert.equal(readFileSync(join(prepared.path, 'answer'), 'utf8'), 'unfinished edits\n');
    assert.equal(reopened.repositoryWork(f.run.id)?.transferredTo, next.id);
    assert.deepEqual(await work.prepare(next.id, next.launch!.repository), prepared);
    assert.equal(reopened.codingRecord(next.id), undefined);
    assert.deepEqual(reopened.codingRecord(f.run.id), oldEvidence); assert.deepEqual(reopened.runtime(f.run.id).attempts, attempts);
    assert.equal(reopened.getRun(f.run.id).status, 'cancelled');
    assert.throws(() => reopened.setQueueLaunch(next.id, { requestId: randomUUID(), launch: { ...next.launch, repository: { mode: 'new' } } }, f.roles), /retain/);
  } finally { reopened.close(); }
});

test('CAN-03: source head, dirty content, ownership, requirements and queued admission changes fail closed', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-guards-')); const f = await cancelledFixture(root);
  t.after(async () => { await f.close(); rmSync(root, { recursive: true, force: true }); }); await f.cancel();
  const original = readFileSync(join(f.owned.path, 'answer'), 'utf8');
  writeFileSync(join(f.owned.path, 'answer'), 'outside edits');
  await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /saved edits changed/);
  writeFileSync(join(f.owned.path, 'answer'), original);
  f.git(f.owned.path, 'commit', '--allow-empty', '-m', 'outside commit');
  await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /head or saved edits changed/);
  f.git(f.owned.path, 'reset', '--keep', f.candidate.revision.head);
  f.store.recordOwnership(f.coordinator.attempt.id, false, 'unknown child');
  await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /ownership is unresolved/);
  f.store.recordOwnership(f.coordinator.attempt.id, true);
  await assert.rejects(f.continuation.queue(f.run.id, { ...f.request(), revision: 1 }, f.roles), /current brief/);
  const originalRequest = f.request();
  const next = await f.continuation.queue(f.run.id, originalRequest, f.roles);
  writeFileSync(join(f.owned.path, 'answer'), 'moved while queued');
  await assert.rejects(f.work.prepare(next.id, next.launch!.repository), /saved edits changed/);
  assert.equal(f.store.repositoryWork(next.id), undefined); assert.equal(f.store.repositoryWork(f.run.id)?.transferredTo, undefined);
  writeFileSync(join(f.owned.path, 'answer'), original);
  f.store.cancelQueued(next.id, { requestId: randomUUID() });
  await assert.rejects(f.work.prepare(next.id, next.launch!.repository), /newer control/);
  assert.equal(f.store.getRun(f.run.id).continuedBy, undefined);
  assert.equal((await f.continuation.queue(f.run.id, originalRequest, f.roles)).id, next.id);
  const replacement = await f.continuation.queue(f.run.id, f.request(), f.roles);
  assert.notEqual(replacement.id, next.id);
  assert.equal((await f.work.prepare(replacement.id, replacement.launch!.repository)).path, f.owned.path);
});

for (const changed of [false, true]) test(`CAN-03/04: live closed PRs block queue/admission; ${changed ? 'changed' : 'unchanged'} successor needs fresh evidence and updates the same PR after a lost reply`, async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-pr-')); const f = await cancelledFixture(root, true);
  t.after(async () => { await f.close(); rmSync(root, { recursive: true, force: true }); }); await f.cancel();
  const history = f.store.codingRecord(f.run.id)!;
  for (const state of ['DECLINED', 'MERGED'] as const) { f.state(state); await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /closed, merged/); }
  f.state('OPEN'); f.outage(true); await assert.rejects(f.continuation.queue(f.run.id, f.request(), f.roles), /unavailable/); f.outage(false);
  let sourceId = f.run.id;
  if (changed) {
    const middle = await f.continuation.queue(sourceId, f.request(), f.roles);
    await f.work.prepare(middle.id, middle.launch!.repository); await f.cancel(middle.id); sourceId = middle.id;
    assert.equal(f.store.publication(sourceId), undefined);
    f.state('DECLINED'); await assert.rejects(f.continuation.queue(sourceId, f.request(), f.roles), /closed, merged/); f.state('OPEN');
  }
  const next = await f.continuation.queue(sourceId, f.request(), f.roles);
  assert.equal(next.continuedFrom?.publicationId, f.store.publication(f.run.id)!.id);
  f.state('DECLINED'); await assert.rejects(f.work.prepare(next.id, next.launch!.repository), /closed, merged/); f.state('OPEN');
  await f.work.prepare(next.id, next.launch!.repository);
  f.evidence.configure(next.id, history.requiredChecks);
  assert.equal((await f.evidence.assess(next.id)).ready, false);
  const role = f.roles.find(item => item.id === 'implementer')!;
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const team = ['Coordinator', 'Writer', 'Reviewer'].map(name => f.store.admitAssignment(next.id, {
    name, coordinator: name === 'Coordinator', role: { ...role, id: name === 'Reviewer' ? 'reviewer' : role.id }, model, input: 'Continue retained work.' }));
  for (const [i, member] of team.entries()) { f.store.claimAttempt(member.attempt.id); f.store.recordAttemptIdentity(member.attempt.id, { hostPid: i + 20, ompPid: i + 30, sessionId: `new-${i}`, hostUrl: `http://127.0.0.1:${9500 + i}`, pane: `%${i + 20}` }); }
  f.store.setDesiredState(next.id, 'running'); f.store.startExecution(next.id); f.store.recordOwnership(team[1].attempt.id, true);
  if (changed) { writeFileSync(join(f.owned.path, 'answer'), 'correct and extended'); f.git(f.owned.path, 'commit', '-am', 'Extend answer'); }
  const candidate = await f.evidence.registerCandidate(next.id, [team[1].attempt.id]);
  assert.equal(candidate.revision.head === f.candidate.revision.head, !changed); assert.notEqual(candidate.id, f.candidate.id);
  await assert.rejects(f.publication.prepare(team[0].attempt.id, { candidateId: candidate.id, summary: reviewerSummary }), /locally validated/);
  const check = history.requiredChecks[0];
  f.evidence.recordCheck(next.id, { candidateId: candidate.id, attemptId: team[2].attempt.id, checkId: check.id, definition: check,
    environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 0, timedOut: false, started: true, completed: true,
    output: f.evidence.artifact('New controlled check receipt'), source: f.evidence.artifact(JSON.stringify(await f.work.listFiles(next.id))), sourceIntact: true });
  f.evidence.assignReview(team[0].attempt.id, 'review-successor', { assignee: 'Reviewer', candidateId: candidate.id, instruction: 'Review the successor independently.' });
  f.evidence.review(team[2].attempt.id, { candidateId: candidate.id, verdict: 'pass', summary: 'Retained code reviewed afresh', findings: [], coverage: [{ criterion: 0, assessment: 'Correct output', checkIds: [check.id] }] });
  const published = await f.publication.prepare(team[0].attempt.id, { candidateId: candidate.id, summary: reviewerSummary + '\n\nRevalidated after cancellation.' });
  assert.equal(published.runId, next.id); assert.equal(published.pr.id, 9); assert.equal(published.id, f.store.publication(f.run.id)?.id);
  f.loseUpdate(); await f.publication.flush(); await f.publication.flush();
  assert.deepEqual(f.counts(), { posts: 1, puts: 1 });
  assert.deepEqual(f.store.codingRecord(f.run.id), history);
  assert.equal(f.store.getRun(f.run.id).status, 'cancelled');
  assert.equal(f.store.publication(next.id)?.candidateId, candidate.id);
});


for (const movement of ['local', 'remote', 'returned']) test(`CAN-03: a moved ${movement} target retains the old base until fresh implementation and an explicit refresh`, async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-target-')); const f = await cancelledFixture(root, movement === 'remote');
  t.after(async () => { await f.close(); rmSync(root, { recursive: true, force: true }); }); await f.cancel();
  writeFileSync(join(f.checkout, 'target-addition'), 'new target work');
  f.git(f.checkout, 'add', '.'); f.git(f.checkout, 'commit', '-m', 'Advance target');
  const target = f.git(f.checkout, 'rev-parse', 'HEAD');
  if (movement === 'remote') f.git(f.checkout, 'push', f.remote, 'main');
  const next = await f.continuation.queue(f.run.id, f.request(), f.roles);
  const prepared = await f.work.prepare(next.id, next.launch!.repository);
  assert.equal(prepared.base, f.owned.base); assert.equal(prepared.targetRef, f.owned.base);
  assert.equal(prepared.continuationTargetRef, f.owned.targetRef);
  f.evidence.configure(next.id, f.store.codingRecord(f.run.id)!.requiredChecks);
  const role = f.roles.find(item => item.id === 'implementer')!;
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const writer = f.store.admitAssignment(next.id, { name: 'Writer', coordinator: false, role, model, input: 'Inspect the retained implementation.' });
  f.store.claimAttempt(writer.attempt.id);
  f.store.recordAttemptIdentity(writer.attempt.id, { hostPid: 123, ompPid: 124, sessionId: 'successor-writer', hostUrl: 'http://127.0.0.1:9555', pane: '%31' });
  f.store.recordOwnership(writer.attempt.id, true);
  const fresh = await f.evidence.registerCandidate(next.id, [writer.attempt.id]);
  assert.notEqual(fresh.id, f.candidate.id);
  assert.match((await f.evidence.assess(next.id)).blockers.join(' '), /Refresh from the target/);
  f.store.setDesiredState(next.id, 'paused'); f.store.beginPause(next.id); f.store.finishPause(next.id);
  if (movement === 'returned') f.git(f.checkout, 'reset', '--keep', f.owned.base);
  const refreshed = await new TargetRefresh(f.store, f.work, f.evidence, movement === 'remote' ? f.publication : undefined).refresh(next.id, { requestId: randomUUID() });
  assert.equal(refreshed.changed, movement !== 'returned'); assert.equal(f.store.repositoryWork(next.id)!.base, movement === 'returned' ? f.owned.base : target);
  assert.equal(f.store.repositoryWork(next.id)!.continuationTargetRef, undefined);
  assert.equal(f.store.codingRecord(next.id)!.candidates.at(-1)!.refreshedFrom, movement === 'returned' ? undefined : fresh.id);
  assert.equal((await f.evidence.assess(next.id)).ready, false);
  if (movement !== 'returned') assert.equal(readFileSync(join(f.owned.path, 'target-addition'), 'utf8'), 'new target work');
});
