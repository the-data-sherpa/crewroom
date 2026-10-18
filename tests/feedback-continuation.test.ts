import assert from 'node:assert/strict';
import { z } from 'zod';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { Coding } from '../server/coding';
import { CodingScheduler } from '../server/coding-scheduler';
import { Publication } from '../server/publication';
import { RuntimeSupervisor } from '../server/runtime/supervisor';
import { defaultRoles } from '../server/roles';
import { publicationFixture, reviewerSummary } from './fixtures/publication';
import type { BitbucketPullRequest } from '../server/bitbucket-client';

const repository = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
const policy = { statusKeys: ['pipeline'], pipelineSteps: ['Unit Tests'], optionalSteps: [] };

/** Simulated Bitbucket; the branch/push and candidate commits use disposable real Git repositories. */
test('DEL-04/05/06: explicit feedback creates a freshly validated successor on the same PR; a lost PUT reply is reconciled', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-revision-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { store, work, evidence, run, candidate, git, owned } = f;
  let pr: BitbucketPullRequest | undefined; let posts = 0, puts = 0, losePut = true, hosted = true;
  const comment = { id: 51, created_on: '2026-10-01T00:00:00Z', updated_on: '2026-10-01T00:00:00Z', deleted: false,
    content: { raw: 'Cover the additional edge case' }, user: { uuid: '{reviewer}', display_name: 'Reviewer' }, inline: { path: 'answer', to: 1 } };
  const remoteHead = () => git(root, '--git-dir', f.remote, 'rev-parse', `refs/heads/${owned.branch}`);
  const provider = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture'); res.setHeader('Content-Type', 'application/json');
    if (url.pathname === '/2.0/repositories/fixture/repository') { res.end(JSON.stringify({ uuid: repository.uuid, full_name: 'fixture/repository' })); return; }
    if (url.pathname.endsWith('/pullrequests/9/comments')) { res.end(JSON.stringify({ values: [comment] })); return; }
    if (req.method === 'POST' && url.pathname.endsWith('/pullrequests')) {
      let body = ''; for await (const chunk of req) body += chunk;
      const input = z.object({ title: z.string(), description: z.string(), source: z.object({ branch: z.object({ name: z.string() }) }),
        destination: z.object({ branch: z.object({ name: z.string() }) }) }).parse(JSON.parse(body)); posts++;
      pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-10-01T00:00:00Z',
        source: { branch: input.source.branch, commit: { hash: remoteHead() }, repository },
        destination: { branch: input.destination.branch, commit: { hash: candidate.revision.base }, repository },
        links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
      res.statusCode = 201; res.end(JSON.stringify(pr)); return;
    }
    if (req.method === 'PUT' && url.pathname.endsWith('/pullrequests/9')) {
      let body = ''; for await (const chunk of req) body += chunk;
      const input = z.object({ title: z.string(), description: z.string() }).parse(JSON.parse(body));
      if (!pr) { res.statusCode = 404; res.end('{}'); return; }
      puts++; pr = { ...pr, title: input.title, description: input.description };
      if (losePut) { losePut = false; req.socket.destroy(); } else res.end(JSON.stringify({ ...pr, source: { ...pr.source, commit: { hash: remoteHead() } } }));
      return;
    }
    if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr ? [{ ...pr, source: { ...pr.source, commit: { hash: remoteHead() } } }] : [] })); return; }
    if (!pr) { res.statusCode = 404; res.end('{}'); return; }
    if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify({ ...pr, source: { ...pr.source, commit: { hash: remoteHead() } } })); return; }
    if (url.pathname.endsWith('/statuses')) { res.end(JSON.stringify({ values: [{ key: 'pipeline', state: hosted ? 'SUCCESSFUL' : 'INPROGRESS', name: 'Pipeline', url: 'https://example.invalid/check', updated_on: '2026-10-01T00:00:00Z' }] })); return; }
    if (url.pathname.endsWith('/steps/')) { res.end(JSON.stringify({ values: [{ uuid: '{step}', name: 'Unit Tests', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } } }] })); return; }
    if (url.pathname.endsWith('/pipelines/')) { res.end(JSON.stringify({ values: hosted ? [{ uuid: '{pipeline}', build_number: 2, created_on: '2026-10-01T00:00:00Z', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } }, target: { type: 'pipeline_pullrequest_target', source: owned.branch, destination: 'main', commit: { hash: remoteHead() }, destination_commit: { hash: candidate.revision.base }, pullrequest: { id: 9 }, selector: { type: 'pull-requests', pattern: '**' } } }] : [] })); return; }
    res.statusCode = 404; res.end('{}');
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const address = provider.address(); if (!address || typeof address === 'string') throw new Error('Provider fixture did not bind.');
  const publication = new Publication(store, work, evidence, join(root, 'publication'), [{ repositoryPath: owned.repository.path,
    repository, policy, fixtureGitPath: f.remote, apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'synthetic' }]);
  t.after(async () => { await publication.close(); store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });

  await publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary });
  await publication.flush();
  assert.equal((await publication.read(run.id))?.ready, true); assert.equal(posts, 1);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: (await work.status(run.id)).revision });
  store.releaseCodingSlot(run.id, 'review', candidate.id);
  await publication.observeFeedback(run.id);
  assert.equal(store.queueFeedbackResume(run.id), true);
  const captured = store.getRun(run.id).feedbackContinuations![0];
  assert.equal(captured.prId, 9); assert.equal(captured.sourceCandidateId, candidate.id);
  store.savePublicationFeedback(run.id, 9, { state: 'complete', comments: [comment, { ...comment, id: 52,
    content: { raw: 'A later, uncaptured suggestion' }, updated_on: '2026-10-01T00:02:00Z' }] }, new Date(Date.now() + 1000).toISOString());
  assert.deepEqual(store.getRun(run.id).feedbackContinuations![0].feedback.comments.map(item => item.id), [51], 'Later feedback never rewrites the captured assignment.');
  assert.deepEqual(store.publication(run.id)?.feedback?.comments.map(item => item.id), [51, 52]);
  assert.equal(store.queueFeedbackResume(run.id), false); assert.equal(store.getRun(run.id).feedbackContinuations?.length, 1);
  store.reclaimCodingSlot(run.id);
  const requested = store.setDesiredState(run.id, 'running'); store.setResumeRequestState(run.id, 'starting');
  for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
  store.prepareContinuation(run.id, requested.controlRevision!);
  for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
    const attempt = store.getAttempt(assignment.currentAttemptId);
    store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 100 + index, ompPid: 200 + index, sessionId: `feedback-${index}`, hostUrl: `http://127.0.0.1:${9100 + index}`, pane: `%${index + 10}` });
  }
  store.startExecution(run.id); store.markFeedbackHandoff(run.id, captured.id); store.clearResumeRequest(run.id);
  const current = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
  const coding = new Coding(store, work, evidence);
  const claim = await coding.claim(current.Coordinator, 'feedback-claim', { assignee: 'Writer', instruction: 'Fix the captured edge case, without changing accepted scope.' });
  assert.equal(claim.feedbackContinuationId, captured.id); assert.equal(claim.automatic, false);
  const before = await work.readFile(run.id, 'answer');
  await coding.patch(current.Writer, 'feedback-edit', { path: 'answer', expectedHash: before.sha256, replacements: [{ oldText: 'correct', newText: 'correct and edge-safe' }] });
  const completion = await coding.requestCompletion(current.Writer, 'feedback-finish', { summary: 'Cover the feedback edge case' });
  store.recordOwnership(current.Writer, true);
  await work.releaseWriter(current.Writer);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.beginPause(run.id, 'review'); store.finishPause(run.id);
  const next = await coding.complete(run.id, completion.id);
  assert.notEqual(next.id, candidate.id); assert.notEqual(next.revision.head, candidate.revision.head);
  assert.deepEqual(next.feedbackFrom, { continuationId: captured.id, sourceCandidateId: candidate.id });
  assert.equal(store.codingRecord(run.id)?.candidates.length, 2);
  assert.equal((await evidence.assess(run.id)).ready, false, 'Old check/review evidence does not validate the feedback revision.');
  await coding.complete(run.id, completion.id); assert.equal(store.codingRecord(run.id)?.candidates.length, 2);
  store.setDesiredState(run.id, 'paused');
  assert.equal(store.queueFeedbackResume(run.id), true);
  assert.equal(store.getRun(run.id).feedbackContinuations?.length, 1, 'The unpublished feedback candidate stays in its original continuation.');
  store.setDesiredState(run.id, 'running'); store.setResumeRequestState(run.id, 'starting');
  for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
  store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
  for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
    const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 300 + index, ompPid: 400 + index, sessionId: `validation-${index}`, hostUrl: `http://127.0.0.1:${9200 + index}`, pane: `%${index + 20}` });
  }
  store.startExecution(run.id);
  store.clearResumeRequest(run.id);
  const validation = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
  await assert.rejects(publication.prepare(validation.Coordinator, { candidateId: next.id, summary: reviewerSummary }), /locally validated/);
  const check = store.codingRecord(run.id)!.requiredChecks[0];
  evidence.recordCheck(run.id, { candidateId: next.id, attemptId: validation.Reviewer, checkId: check.id, definition: check,
    environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 1, timedOut: false, started: true, completed: true,
    sourceIntact: true, output: evidence.artifact('Simulated failing controller receipt'), source: evidence.artifact('Simulated source snapshot') });
  await assert.rejects(publication.prepare(validation.Coordinator, { candidateId: next.id, summary: reviewerSummary }), /locally validated/);
  evidence.recordCheck(run.id, { candidateId: next.id, attemptId: validation.Reviewer, checkId: check.id, definition: check,
    environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 0, timedOut: false, started: true, completed: true,
    sourceIntact: true, output: evidence.artifact('Simulated passing controller receipt for feedback revision'), source: evidence.artifact('Simulated source snapshot') });
  evidence.assignReview(validation.Coordinator, 'feedback-review', { assignee: 'Reviewer', candidateId: next.id, instruction: 'Review the edge case change.' });
  evidence.review(validation.Reviewer, { candidateId: next.id, verdict: 'pass', summary: 'Feedback change verified', findings: [], coverage: [{ criterion: 0, assessment: 'Edge case verified', checkIds: [check.id] }] });
  assert.equal((await evidence.assess(run.id)).ready, true);
  const openPR = store.publication(run.id)!;
  store.savePublication({ ...openPR, pr: { ...openPR.pr, providerState: 'DECLINED' } });
  await assert.rejects(publication.prepare(validation.Coordinator, { candidateId: next.id, summary: reviewerSummary }), /not open/);
  store.savePublication(openPR);
  await publication.prepare(validation.Coordinator, { candidateId: next.id, summary: reviewerSummary + '\n\nAddresses review feedback on the edge case.' });
  hosted = false; await publication.flush();
  assert.equal(remoteHead(), next.revision.head); assert.equal(puts, 1); assert.equal(posts, 1);
  assert.equal((await publication.read(run.id))?.ready, false, 'Lost description response does not invent readiness.');
  hosted = true; await publication.flush();
  const published = await publication.read(run.id);
  assert.equal(published?.candidateId, next.id); assert.equal(published?.pr.id, 9); assert.equal(published?.ready, true);
  assert.equal(puts, 1); assert.equal(posts, 1); assert.equal(published?.history?.[0].candidateId, candidate.id);
});

test('DEL-05: a feedback continuation with unchanged files retains the published candidate and all historical evidence', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-no-change-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { store, work, evidence, run, candidate } = f;
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const published = { id: randomUUID(), runId: run.id, attemptId: f.coordinator.attempt.id, candidateId: candidate.id,
    repositoryPath: f.owned.repository.path, repository, branch: f.owned.branch, target: 'main', revision: candidate.revision,
    title: 'SIM-1', description: 'Existing PR', policy, createdAt: new Date().toISOString(), push: { state: 'confirmed' as const, expectedHead: null },
    pr: { state: 'confirmed' as const, id: 9, providerState: 'OPEN' as const }, ready: true, blockers: [],
    feedback: { state: 'complete' as const, observedAt: '2026-10-01T00:01:00Z', comments: [{ id: 1,
      created_on: '2026-10-01T00:00:00Z', updated_on: '2026-10-01T00:00:00Z', deleted: false,
      content: { raw: 'Please recheck the result' }, currentness: 'known' as const }] } };
  store.savePublication(published);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: (await work.status(run.id)).revision });
  store.releaseCodingSlot(run.id, 'review', candidate.id);
  store.savePublication({ ...published, pr: { ...published.pr, providerState: 'DECLINED' } });
  assert.throws(() => store.queueFeedbackResume(run.id), /closed, merged/);
  assert.equal(store.getRun(run.id).feedbackContinuations, undefined);
  store.savePublication(published);
  store.queueFeedbackResume(run.id); const captured = store.getRun(run.id).feedbackContinuations![0];
  store.reclaimCodingSlot(run.id); const request = store.setDesiredState(run.id, 'running'); store.setResumeRequestState(run.id, 'starting');
  for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
  store.prepareContinuation(run.id, request.controlRevision!);
  for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
    const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 500 + index, ompPid: 600 + index, sessionId: `unchanged-${index}`, hostUrl: `http://127.0.0.1:${9300 + index}`, pane: `%${index + 30}` });
  }
  store.startExecution(run.id); store.markFeedbackHandoff(run.id, captured.id); store.clearResumeRequest(run.id);
  const team = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
  const coding = new Coding(store, work, evidence);
  const claimed = await coding.claim(team.Coordinator, 'check-feedback', { assignee: 'Writer', instruction: 'Verify the captured request within scope.' });
  assert.equal(claimed.feedbackContinuationId, captured.id);
  const completion = await coding.requestCompletion(team.Writer, 'unchanged', { summary: 'Verified existing answer' });
  store.recordOwnership(team.Writer, true); await work.releaseWriter(team.Writer);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.beginPause(run.id, 'review'); store.finishPause(run.id);
  const before = store.codingRecord(run.id)!;
  const outcome = await coding.complete(run.id, completion.id);
  assert.equal(outcome.id, candidate.id); assert.equal(outcome.revision.head, candidate.revision.head);
  assert.equal(store.getRun(run.id).feedbackContinuations?.[0].state, 'no_change');
  assert.equal(store.codingRecord(run.id)?.candidates.length, 1);
  assert.deepEqual(store.codingRecord(run.id)?.checks, before.checks);
  assert.deepEqual(store.codingRecord(run.id)?.reviews, before.reviews);
  assert.equal(store.publication(run.id)?.candidateId, candidate.id);
  await coding.complete(run.id, completion.id);
  assert.equal(store.codingRecord(run.id)?.candidates.length, 1);
  const { currentness: _currentness, ...sameComment } = published.feedback.comments[0];
  store.savePublicationFeedback(run.id, 9, { state: 'complete', comments: [sameComment] }, new Date().toISOString());
  store.setDesiredState(run.id, 'paused');
  assert.equal(store.queueFeedbackResume(run.id), true);
  assert.equal(store.getRun(run.id).feedbackContinuations?.length, 1, 'A repeat read alone cannot create a second logical continuation.');
});

test('DEL-04/06: a closed PR terminalizes queued feedback and leaves the global coding queue available', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-closed-queue-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { store, run, candidate } = f;
  const published = { id: randomUUID(), runId: run.id, attemptId: f.coordinator.attempt.id, candidateId: candidate.id,
    repositoryPath: f.owned.repository.path, repository, branch: f.owned.branch, target: 'main', revision: candidate.revision,
    title: 'SIM-1', description: 'Existing PR', policy, createdAt: new Date().toISOString(), push: { state: 'confirmed' as const, expectedHead: null },
    pr: { state: 'confirmed' as const, id: 9, providerState: 'OPEN' as const }, ready: true, blockers: [],
    feedback: { state: 'complete' as const, observedAt: '2026-10-01T00:00:00Z', comments: [{ id: 71,
      created_on: '2026-09-30T00:00:00Z', updated_on: '2026-09-30T00:00:00Z', deleted: false,
      content: { raw: 'Check this edge' }, currentness: 'known' as const }] } };
  store.savePublication(published);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  store.releaseCodingSlot(run.id, 'review', candidate.id);
  const holderTask = store.createTask({ title: 'Slot holder', description: '', repositoryId: run.taskSnapshot.repositoryId });
  const holderBrief = store.refine(holderTask.id, { revision: 1, sourceRevision: 1, brief: 'Hold slot', criteria: ['Hold slot'] });
  const holder = store.queue(holderTask.id, { revision: holderBrief.revision }, defaultRoles);
  store.claimCodingSlot(holder.id, true);
  const nextTask = store.createTask({ title: 'Next independent work', description: '', repositoryId: run.taskSnapshot.repositoryId });
  const nextBrief = store.refine(nextTask.id, { revision: 1, sourceRevision: 1, brief: 'Continue queue', criteria: ['Start next work'] });
  const next = store.queue(nextTask.id, { revision: nextBrief.revision, launch: {
    team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Continue independently.' }], repository: { mode: 'new' } } }, defaultRoles);
  const runtime = new RuntimeSupervisor(store, { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:4000');
  let completeLaunch!: () => void; const launched = new Promise<void>(resolve => { completeLaunch = resolve; });
  const scheduled = new CodingScheduler(store, f.work, f.evidence,
    { busy: () => false, resume: (id: string) => runtime.resume(id), preflightStart: () => {}, startScheduled: async () => { completeLaunch(); return {}; } } as unknown as RuntimeSupervisor,
    () => defaultRoles);
  t.after(async () => { await scheduled.close(); await runtime.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  runtime.onSchedulingChange(() => scheduled.kick());
  assert.equal(store.queueFeedbackResume(run.id), true);
  const captured = store.getRun(run.id).feedbackContinuations![0];
  store.savePublication({ ...published, pr: { ...published.pr, providerState: 'DECLINED' }, ready: false, reviewInvalidated: true, blockers: ['PR closed'] });
  store.setDesiredState(holder.id, 'cancelled');
  scheduled.kick(); await scheduled.idle();
  const blocked = store.getRun(run.id);
  assert.equal(blocked.resumeRequest, undefined);
  assert.equal(blocked.feedbackContinuations?.[0].state, 'blocked');
  assert.match(blocked.feedbackContinuations?.[0].holdReason ?? '', /closed|DECLINED/);
  assert.equal(store.codingSlot()?.id, next.id, 'An invalid old feedback intent cannot starve the next independent plan.');
  await launched; await scheduled.idle();
  assert.equal(store.getRun(run.id).feedbackContinuations?.[0].id, captured.id);
  assert.throws(() => runtime.resume(run.id), /closed|not open/);
});

test('DEL-05/LIF-03: interrupted feedback edits restore the same writer grant and finish a new candidate', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-restore-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { store, work, evidence, run, candidate } = f;
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const published = { id: randomUUID(), runId: run.id, attemptId: f.coordinator.attempt.id, candidateId: candidate.id,
    repositoryPath: f.owned.repository.path, repository, branch: f.owned.branch, target: 'main', revision: candidate.revision,
    title: 'SIM-1', description: 'Existing PR', policy, createdAt: new Date().toISOString(), push: { state: 'confirmed' as const, expectedHead: null },
    pr: { state: 'confirmed' as const, id: 9, providerState: 'OPEN' as const }, ready: true, blockers: [],
    feedback: { state: 'complete' as const, observedAt: '2026-10-01T00:00:00Z', comments: [{ id: 73,
      created_on: '2026-09-30T00:00:00Z', updated_on: '2026-09-30T00:00:00Z', deleted: false,
      content: { raw: 'Handle the edge case' }, currentness: 'known' as const }] } };
  store.savePublication(published);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: (await work.status(run.id)).revision });
  store.releaseCodingSlot(run.id, 'review', candidate.id);
  store.queueFeedbackResume(run.id);
  const captured = store.getRun(run.id).feedbackContinuations![0];
  store.reclaimCodingSlot(run.id);
  const request = store.setDesiredState(run.id, 'running'); store.setResumeRequestState(run.id, 'starting');
  for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
  store.prepareContinuation(run.id, request.controlRevision!);
  for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
    const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 1200 + index, ompPid: 1300 + index, sessionId: `feedback-edit-${index}`, hostUrl: `http://127.0.0.1:${9600 + index}`, pane: `%${index + 60}` });
  }
  store.startExecution(run.id); store.markFeedbackHandoff(run.id, captured.id); store.clearResumeRequest(run.id);
  const first = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
  const coding = new Coding(store, work, evidence);
  const grant = await coding.claim(first.Coordinator, 'first-feedback-writer', { assignee: 'Writer', instruction: 'Apply the captured feedback within scope.' });
  const file = await work.readFile(run.id, 'answer');
  await coding.patch(first.Writer, 'first-feedback-edit', { path: 'answer', expectedHash: file.sha256,
    replacements: [{ oldText: 'correct', newText: 'correct with retained edge' }] });
  store.beginPause(run.id, 'recovery');
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  assert.equal(store.repositoryWork(run.id)?.writer, undefined);
  assert.equal((await work.readFile(run.id, 'answer')).content, 'correct with retained edge');
  for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
  store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
  for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
    const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 1400 + index, ompPid: 1500 + index, sessionId: `feedback-recovery-${index}`, hostUrl: `http://127.0.0.1:${9700 + index}`, pane: `%${index + 70}` });
  }
  store.startExecution(run.id);
  const resumed = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
  await coding.restoreWriter(run.id);
  assert.equal(store.repositoryWork(run.id)?.writer?.attemptId, resumed.Writer);
  assert.equal(store.codingRecord(run.id)?.claims?.at(-1)?.feedbackContinuationId, captured.id);
  assert.equal(store.codingRecord(run.id)?.claims?.at(-1)?.id === grant.id, false);
  const completion = await coding.requestCompletion(resumed.Writer, 'complete-restored-feedback', { summary: 'Finish the retained feedback edit' });
  store.recordOwnership(resumed.Writer, true); await work.releaseWriter(resumed.Writer);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.beginPause(run.id, 'review'); store.finishPause(run.id);
  const next = await coding.complete(run.id, completion.id);
  assert.notEqual(next.revision.head, candidate.revision.head);
  assert.equal(store.getRun(run.id).feedbackContinuations?.[0].state, 'candidate');
  assert.equal(store.codingRecord(run.id)?.candidates.length, 2);
});

test('DEL-05: a failed feedback candidate is repaired within its original continuation before same-PR publication', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-rework-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { store, work, evidence, run, candidate } = f;
  const published = { id: randomUUID(), runId: run.id, attemptId: f.coordinator.attempt.id, candidateId: candidate.id,
    repositoryPath: f.owned.repository.path, repository, branch: f.owned.branch, target: 'main', revision: candidate.revision,
    title: 'SIM-1', description: 'Existing PR', policy, createdAt: new Date().toISOString(), push: { state: 'confirmed' as const, expectedHead: null },
    pr: { state: 'confirmed' as const, id: 9, providerState: 'OPEN' as const }, ready: true, blockers: [],
    feedback: { state: 'complete' as const, observedAt: '2026-10-01T00:00:00Z', comments: [{ id: 74,
      created_on: '2026-09-30T00:00:00Z', updated_on: '2026-09-30T00:00:00Z', deleted: false,
      content: { raw: 'Cover the reviewed boundary' }, currentness: 'known' as const }] } };
  store.savePublication(published);
  const publication = new Publication(store, work, evidence, join(root, 'publication'), [{ repositoryPath: f.owned.repository.path,
    repository, policy, fixtureGitPath: f.remote, apiUrl: 'http://127.0.0.1:9', credentials: async () => 'synthetic' }]);
  t.after(async () => { await publication.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  const stopped = () => {
    for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  };
  stopped(); store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: (await work.status(run.id)).revision });
  store.releaseCodingSlot(run.id, 'review', candidate.id);
  store.queueFeedbackResume(run.id); const pinned = store.getRun(run.id).feedbackContinuations![0];
  const activate = (stage: number, explicit = false) => {
    if (explicit) { store.reclaimCodingSlot(run.id); const request = store.setDesiredState(run.id, 'running'); store.setResumeRequestState(run.id, 'starting');
      for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
      store.prepareContinuation(run.id, request.controlRevision!);
    } else {
      for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
      store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
    }
    for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
      const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
      store.recordAttemptIdentity(attempt.id, { hostPid: 1600 + stage * 10 + index, ompPid: 1700 + stage * 10 + index,
        sessionId: `feedback-rework-${stage}-${index}`, hostUrl: `http://127.0.0.1:${9800 + stage * 10 + index}`, pane: `%${stage * 10 + index}` });
    }
    store.startExecution(run.id);
    if (store.getRun(run.id).feedbackContinuations?.[0].state === 'starting') store.markFeedbackHandoff(run.id, pinned.id);
    if (store.getRun(run.id).resumeRequest) store.clearResumeRequest(run.id);
    return Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
  };
  const coding = new Coding(store, work, evidence);
  const first = activate(1, true);
  await coding.claim(first.Coordinator, 'feedback-initial', { assignee: 'Writer', instruction: 'Address reviewer boundary.' });
  const firstFile = await work.readFile(run.id, 'answer');
  await coding.patch(first.Writer, 'feedback-first-edit', { path: 'answer', expectedHash: firstFile.sha256,
    replacements: [{ oldText: 'correct', newText: 'correct for boundary one' }] });
  const completion = await coding.requestCompletion(first.Writer, 'feedback-first-finish', { summary: 'First reviewer boundary' });
  store.recordOwnership(first.Writer, true); await work.releaseWriter(first.Writer); stopped(); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  const c2 = await coding.complete(run.id, completion.id);
  const reviewer = activate(2);
  const check = store.codingRecord(run.id)!.requiredChecks[0];
  evidence.recordCheck(run.id, { candidateId: c2.id, attemptId: reviewer.Reviewer, checkId: check.id, definition: check,
    environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 1, timedOut: false, started: true, completed: true,
    sourceIntact: true, output: evidence.artifact('Simulated failing feedback check'), source: evidence.artifact('Simulated source snapshot') });
  stopped(); store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'validation'); store.finishPause(run.id);
  store.queueFeedbackResume(run.id);
  assert.equal(store.getRun(run.id).feedbackContinuations?.length, 1, 'Failed local evidence does not create another feedback assignment.');
  const repair = activate(3, true);
  const grant = await coding.claim(repair.Coordinator, 'feedback-repair', { assignee: 'Writer', instruction: 'Repair the failed check on the same feedback change.' });
  assert.equal(grant.feedbackContinuationId, pinned.id); assert.equal(grant.automatic, true);
  const secondFile = await work.readFile(run.id, 'answer');
  await coding.patch(repair.Writer, 'feedback-second-edit', { path: 'answer', expectedHash: secondFile.sha256,
    replacements: [{ oldText: 'boundary one', newText: 'all boundaries' }] });
  const secondCompletion = await coding.requestCompletion(repair.Writer, 'feedback-second-finish', { summary: 'Repair reviewer boundary' });
  store.recordOwnership(repair.Writer, true); await work.releaseWriter(repair.Writer); stopped(); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  const c3 = await coding.complete(run.id, secondCompletion.id);
  assert.deepEqual(c3.feedbackFrom, { continuationId: pinned.id, sourceCandidateId: c2.id });
  assert.equal(store.getRun(run.id).feedbackContinuations?.[0].candidateId, c3.id);
  assert.equal((await evidence.assess(run.id)).ready, false, 'C2 failed check cannot validate C3.');
  const final = activate(4);
  evidence.recordCheck(run.id, { candidateId: c3.id, attemptId: final.Reviewer, checkId: check.id, definition: check,
    environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 0, timedOut: false, started: true, completed: true,
    sourceIntact: true, output: evidence.artifact('Simulated corrected feedback check'), source: evidence.artifact('Simulated source snapshot') });
  evidence.assignReview(final.Coordinator, 'feedback-final-review', { assignee: 'Reviewer', candidateId: c3.id, instruction: 'Review the repaired boundary.' });
  evidence.review(final.Reviewer, { candidateId: c3.id, verdict: 'pass', summary: 'All boundaries reviewed', findings: [],
    coverage: [{ criterion: 0, assessment: 'Both boundaries verified', checkIds: [check.id] }] });
  const intent = await publication.prepare(final.Coordinator, { candidateId: c3.id, summary: reviewerSummary + '\n\nAddressed all review boundaries.' });
  assert.equal(intent.pr.id, 9); assert.equal(intent.push.expectedHead, candidate.revision.head);
  assert.equal(intent.history?.[0].candidateId, candidate.id);
});

test('DEL-04/06: comments arriving after an ordinary queued Resume cannot strand it when the PR closes', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-late-closed-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { store, run, candidate } = f;
  const published = { id: randomUUID(), runId: run.id, attemptId: f.coordinator.attempt.id, candidateId: candidate.id,
    repositoryPath: f.owned.repository.path, repository, branch: f.owned.branch, target: 'main', revision: candidate.revision,
    title: 'SIM-1', description: 'Existing PR', policy, createdAt: new Date().toISOString(), push: { state: 'confirmed' as const, expectedHead: null },
    pr: { state: 'confirmed' as const, id: 9, providerState: 'OPEN' as const }, ready: true, blockers: [] };
  store.savePublication(published);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
  store.releaseCodingSlot(run.id, 'review', candidate.id);
  const holderTask = store.createTask({ title: 'Earlier slot holder', description: '', repositoryId: run.taskSnapshot.repositoryId });
  const holderBrief = store.refine(holderTask.id, { revision: 1, sourceRevision: 1, brief: 'Hold slot', criteria: ['Hold slot'] });
  const holder = store.queue(holderTask.id, { revision: holderBrief.revision }, defaultRoles); store.claimCodingSlot(holder.id, true);
  assert.equal(store.queueReviewResume(run.id), true);
  assert.equal(store.getRun(run.id).feedbackContinuations, undefined, 'No feedback was known at the explicit control.');
  store.savePublicationFeedback(run.id, 9, { state: 'complete', comments: [{ id: 75,
    created_on: '2026-10-01T00:00:00Z', updated_on: '2026-10-01T00:00:00Z', deleted: false,
    content: { raw: 'A later reviewer comment' } }] }, new Date().toISOString());
  store.savePublication({ ...published, pr: { ...published.pr, providerState: 'DECLINED' }, ready: false,
    reviewInvalidated: true, blockers: ['PR closed'] });
  const nextTask = store.createTask({ title: 'After closed PR', description: '', repositoryId: run.taskSnapshot.repositoryId });
  const nextBrief = store.refine(nextTask.id, { revision: 1, sourceRevision: 1, brief: 'Advance queue', criteria: ['Next task'] });
  const next = store.queue(nextTask.id, { revision: nextBrief.revision, launch: {
    team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Continue.' }], repository: { mode: 'new' } } }, defaultRoles);
  const runtime = new RuntimeSupervisor(store, { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:4000');
  let completeNextLaunch!: () => void; const nextLaunched = new Promise<void>(resolve => { completeNextLaunch = resolve; });
  const scheduler = new CodingScheduler(store, f.work, f.evidence,
    { busy: () => false, resume: (id: string) => runtime.resume(id), preflightStart: () => {}, startScheduled: async () => { completeNextLaunch(); return {}; } } as unknown as RuntimeSupervisor,
    () => defaultRoles);
  t.after(async () => { await scheduler.close(); await runtime.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  runtime.onSchedulingChange(() => scheduler.kick());
  store.setDesiredState(holder.id, 'cancelled');
  scheduler.kick(); await scheduler.idle();
  assert.equal(store.getRun(run.id).resumeRequest, undefined, 'The old ordinary request must leave FIFO after its PR closes.');
  assert.match(store.getRun(run.id).attentionReason ?? '', /closed|not open/);
  assert.equal(store.getRun(run.id).feedbackContinuations, undefined, 'Late comments were never implicitly captured.');
  assert.equal(store.codingSlot()?.id, next.id);
  await nextLaunched; await scheduler.idle();
});
