import { test, expect } from '@playwright/test';
import { z } from 'zod';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { publicationFixture, reviewerSummary } from '../fixtures/publication';
import { startServer } from '../../server/app';
import { Workbench } from '../../server/workbench';
import { RepositoryWork } from '../../server/repository-work';
import { Evidence } from '../../server/evidence';
import { Publication } from '../../server/publication';
import { Coding } from '../../server/coding';
import { defaultRoles } from '../../server/roles';
import type { BitbucketPullRequest } from '../../server/bitbucket-client';

const repository = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
const policy = { statusKeys: ['pipeline'], pipelineSteps: ['Unit Tests'], optionalSteps: [] };

/** Provider, model-host and check boundaries are simulated; Git, browser, workbench and publication services are real disposable fixtures. */
test('observe PR feedback, select Resume and publish a validated revision to the same PR', async ({ page }, testInfo) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-browser-'));
  const f = await publicationFixture(root, { claimSlot: true });
  const { run, candidate, owned, git } = f;
  let pr: BitbucketPullRequest | undefined; let posts = 0, puts = 0; let app: { origin: string; close(): Promise<void> } | undefined;
  let comment = { id: 71, created_on: '2026-10-01T00:00:00Z', updated_on: '2026-10-01T00:00:00Z', deleted: false,
    content: { raw: 'Handle the reviewed edge case' }, user: { uuid: '{reviewer}', display_name: 'Reviewer' }, inline: { path: 'answer', to: 1 } };
  const remoteHead = () => git(root, '--git-dir', f.remote, 'rev-parse', `refs/heads/${owned.branch}`);
  const provider = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture'); res.setHeader('Content-Type', 'application/json');
    if (url.pathname === '/2.0/repositories/fixture/repository') { res.end(JSON.stringify({ uuid: repository.uuid, full_name: 'fixture/repository' })); return; }
    if (url.pathname.endsWith('/pullrequests/9/comments')) { res.end(JSON.stringify({ values: [comment] })); return; }
    if (req.method === 'POST' && url.pathname.endsWith('/pullrequests')) {
      let body = ''; for await (const part of req) body += part;
      const input = z.object({ title: z.string(), description: z.string(), source: z.object({ branch: z.object({ name: z.string() }) }),
        destination: z.object({ branch: z.object({ name: z.string() }) }) }).parse(JSON.parse(body)); posts++;
      pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-10-01T00:00:00Z',
        source: { branch: input.source.branch, commit: { hash: remoteHead() }, repository },
        destination: { branch: input.destination.branch, commit: { hash: candidate.revision.base }, repository },
        links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
      res.end(JSON.stringify(pr)); return;
    }
    if (req.method === 'PUT' && url.pathname.endsWith('/pullrequests/9')) {
      let body = ''; for await (const part of req) body += part;
      const input = z.object({ title: z.string(), description: z.string() }).parse(JSON.parse(body));
      if (!pr) { res.statusCode = 404; res.end('{}'); return; }
      puts++; pr = { ...pr, title: input.title, description: input.description };
      res.end(JSON.stringify({ ...pr, source: { ...pr.source, commit: { hash: remoteHead() } } })); return;
    }
    if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr ? [{ ...pr, source: { ...pr.source, commit: { hash: remoteHead() } } }] : [] })); return; }
    if (!pr) { res.statusCode = 404; res.end('{}'); return; }
    if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify({ ...pr, source: { ...pr.source, commit: { hash: remoteHead() } } })); return; }
    if (url.pathname.endsWith('/statuses')) { res.end(JSON.stringify({ values: [{ key: 'pipeline', name: 'Pipeline', state: 'SUCCESSFUL', updated_on: '2026-10-01T00:00:00Z', url: 'https://example.invalid/check' }] })); return; }
    if (url.pathname.endsWith('/steps/')) { res.end(JSON.stringify({ values: [{ uuid: '{step}', name: 'Unit Tests', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } } }] })); return; }
    if (url.pathname.endsWith('/pipelines/')) { res.end(JSON.stringify({ values: [{ uuid: '{pipeline}', build_number: 1, created_on: '2026-10-01T00:00:00Z', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } }, target: {
      type: 'pipeline_pullrequest_target', source: owned.branch, destination: 'main', commit: { hash: remoteHead() }, destination_commit: { hash: candidate.revision.base },
      pullrequest: { id: 9 }, selector: { type: 'pull-requests', pattern: '**' } } }] })); return; }
    res.statusCode = 404; res.end('{}');
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const address = provider.address(); if (!address || typeof address === 'string') throw new Error('Provider fixture did not bind.');
  const connections = [{ repositoryPath: owned.repository.path, repository, policy, fixtureGitPath: f.remote,
    apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'synthetic' }];
  let publication = new Publication(f.store, f.work, f.evidence, join(root, 'publication'), connections);
  try {
    await publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary });
    await publication.flush(); expect((await publication.read(run.id))?.ready).toBe(true);
    for (const attempt of f.store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') f.store.recordOwnership(attempt.id, true);
    f.store.setDesiredState(run.id, 'paused'); f.store.beginPause(run.id, 'review'); f.store.finishPause(run.id);
    f.store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: (await f.work.status(run.id)).revision });
    f.store.releaseCodingSlot(run.id, 'review', candidate.id);
    const task = f.store.createTask({ title: 'Occupy coding slot', description: '', repositoryId: run.taskSnapshot.repositoryId });
    const brief = f.store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Wait here', criteria: ['No concurrent writer'] });
    const holder = f.store.queue(task.id, { revision: brief.revision }, defaultRoles);
    f.store.claimCodingSlot(holder.id, true); f.store.setDesiredState(holder.id, 'paused'); f.store.beginPause(holder.id, 'operator'); f.store.finishPause(holder.id);
    await publication.close(); f.store.close();
    app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json'), publication: connections,
      runtimeConfig: { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
        mail: { url: 'http://127.0.0.1:9/mcp/', token: 'synthetic', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
        catalog: [], policy: {}, models: { providers: {} } } });
    await page.goto(`${app.origin}/#${run.taskId}`);
    await page.getByRole('tab', { name: 'Change & evidence' }).click();
    const feedback = page.getByRole('region', { name: 'PR feedback' });
    await expect(feedback).toContainText('Comment #71'); await expect(feedback).toContainText('Reviewer');
    await expect(feedback).toContainText('answer:1'); await expect(feedback).toContainText('Handle the reviewed edge case');
    await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
    expect(posts).toBe(1); expect(puts).toBe(0);
    await page.getByRole('button', { name: 'Resume run' }).click();
    await expect(page.getByRole('button', { name: 'Resume queued' })).toBeVisible();
    await expect(feedback).toContainText('Captured by continuation');
    await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('feedback-resume-queued.png'), fullPage: true });
    await feedback.screenshot({ path: testInfo.outputPath('feedback-panel-queued.png') });
    comment = { ...comment, updated_on: '2026-10-01T00:03:00Z', content: { raw: 'Later edit outside the captured assignment' } };
    const refresh = await page.request.get(`${app.origin}/api/runs/${run.id}/feedback`);
    expect(refresh.ok()).toBe(true);
    await expect(feedback).toContainText('Later edit outside the captured assignment');
    await expect(feedback).toContainText('Captured by continuation', { timeout: 5000 });
    expect(posts).toBe(1); expect(puts).toBe(0);
    await app.close(); app = undefined;

    // Simulate the stopped coordinator/writer and check receipts, but exercise the real durable control,
    // Git worktree, claim, completion, evidence, and publication transitions in this same run.
    const store = new Workbench(join(root, 'workflow.sqlite'));
    const work = new RepositoryWork(store, join(root, 'owned'));
    const evidence = new Evidence(store, work, join(root, 'artifacts'));
    publication = new Publication(store, work, evidence, join(root, 'publication'), connections);
    try {
      expect(store.getRun(run.id).feedbackContinuations?.[0].feedback.comments[0].content?.raw).toBe('Handle the reviewed edge case');
      store.setDesiredState(holder.id, 'cancelled'); store.reclaimCodingSlot(run.id);
      const request = store.setDesiredState(run.id, 'running'); store.setResumeRequestState(run.id, 'starting');
      for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
      store.prepareContinuation(run.id, request.controlRevision!);
      for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
        const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
        store.recordAttemptIdentity(attempt.id, { hostPid: 800 + index, ompPid: 900 + index, sessionId: `browser-feedback-${index}`, hostUrl: `http://127.0.0.1:${9400 + index}`, pane: `%${index + 40}` });
      }
      store.startExecution(run.id);
      const captured = store.getRun(run.id).feedbackContinuations![0]; store.markFeedbackHandoff(run.id, captured.id); store.clearResumeRequest(run.id);
      const team = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
      const coding = new Coding(store, work, evidence);
      const claim = await coding.claim(team.Coordinator, 'browser-feedback-claim', { assignee: 'Writer', instruction: 'Handle captured review feedback within accepted brief.' });
      expect(claim.feedbackContinuationId).toBe(captured.id);
      const prior = await work.readFile(run.id, 'answer');
      await coding.patch(team.Writer, 'browser-feedback-edit', { path: 'answer', expectedHash: prior.sha256,
        replacements: [{ oldText: 'correct', newText: 'correct and edge-safe' }] });
      const completion = await coding.requestCompletion(team.Writer, 'browser-feedback-finish', { summary: 'Address feedback edge case' });
      store.recordOwnership(team.Writer, true); await work.releaseWriter(team.Writer);
      for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
      store.beginPause(run.id, 'review'); store.finishPause(run.id);
      const next = await coding.complete(run.id, completion.id);
      expect(next.revision.head).not.toBe(candidate.revision.head);
      for (const assignment of store.runtime(run.id).assignments) store.replaceAttempt(assignment.id, { automatic: false });
      store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
      for (const [index, assignment] of store.runtime(run.id).assignments.entries()) {
        const attempt = store.getAttempt(assignment.currentAttemptId); store.claimAttempt(attempt.id);
        store.recordAttemptIdentity(attempt.id, { hostPid: 1000 + index, ompPid: 1100 + index, sessionId: `browser-check-${index}`, hostUrl: `http://127.0.0.1:${9500 + index}`, pane: `%${index + 50}` });
      }
      store.startExecution(run.id);
      const checkTeam = Object.fromEntries(store.runtime(run.id).assignments.map(item => [item.name, item.currentAttemptId]));
      const check = store.codingRecord(run.id)!.requiredChecks[0];
      evidence.recordCheck(run.id, { candidateId: next.id, attemptId: checkTeam.Reviewer, checkId: check.id, definition: check,
        environment: { image: check.image, platform: 'simulated', network: 'none' }, exitCode: 0, timedOut: false, started: true, completed: true,
        sourceIntact: true, output: evidence.artifact('Simulated controller check passed'), source: evidence.artifact('Simulated source snapshot') });
      evidence.assignReview(checkTeam.Coordinator, 'browser-feedback-review', { assignee: 'Reviewer', candidateId: next.id, instruction: 'Review the new edge case.' });
      evidence.review(checkTeam.Reviewer, { candidateId: next.id, verdict: 'pass', summary: 'Reviewed edge case', findings: [], coverage: [{ criterion: 0, assessment: 'Edge case covered', checkIds: [check.id] }] });
      await publication.prepare(checkTeam.Coordinator, { candidateId: next.id, summary: reviewerSummary + '\n\nAddressed comment #71.' });
      await publication.flush();
      expect((await publication.read(run.id))?.ready).toBe(true);
      expect(remoteHead()).toBe(next.revision.head); expect(posts).toBe(1); expect(puts).toBe(1);
      for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
      store.setDesiredState(run.id, 'paused'); store.beginPause(run.id, 'review'); store.finishPause(run.id);
      await publication.close(); store.close();
    } finally { await publication.close(); store.close(); }
    app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json'), publication: connections });
    await page.goto(`${app.origin}/#${run.taskId}`); await page.getByRole('tab', { name: 'Change & evidence' }).click();
    await expect(page.getByRole('region', { name: 'PR delivery' })).toContainText('Ready for human review');
    await expect(page.getByRole('region', { name: 'PR delivery' }).getByRole('link', { name: 'Open pull request' }))
      .toHaveAttribute('href', 'https://bitbucket.org/fixture/repository/pull-requests/9');
    await expect(page.getByRole('region', { name: 'PR feedback' })).toContainText('Captured by continuation');
    await page.screenshot({ path: testInfo.outputPath('feedback-same-pr-ready.png'), fullPage: true });
    await page.getByRole('region', { name: 'PR feedback' }).screenshot({ path: testInfo.outputPath('feedback-panel-ready.png') });
    expect(posts).toBe(1); expect(puts).toBe(1);
  } finally {
    await app?.close(); await publication.close(); f.store.close();
    await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true });
  }
});
