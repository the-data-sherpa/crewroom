import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { Coding } from '../server/coding';
import { Publication } from '../server/publication';
import { publicationFixture, reviewerSummary } from './fixtures/publication';
import { gitLatch } from './fixtures/git-latch';
import { JiraClient } from '../server/jira-client';
import { JiraDelivery } from '../server/jira-delivery';

const remoteIdentity = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
const policy = { statusKeys: ['prs:**:main', 'security'], pipelineSteps: ['Unit Tests'], optionalSteps: ['Renovate'] };

test('EVD-04/DEL-01: adopted requirements make the previously ready candidate ineligible for publication', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-adoption-evidence-'));
  const { store, work, evidence, run, candidate, coordinator } = await publicationFixture(root);
  const publication = new Publication(store, work, evidence, join(root, 'publication'), []);
  t.after(async () => { await publication.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  assert.equal((await evidence.assess(run.id)).ready, true);
  const task = store.getTask(run.taskId);
  const accepted = store.refine(task.id, { revision: task.revision, sourceRevision: task.sourceRevision, brief: task.brief, criteria: ['Correct output', 'Reject empty input'] });
  store.adoptRequirements(run.id, { requestId: crypto.randomUUID(), revision: accepted.revision, reason: 'Empty input was omitted.' });
  const stale = await evidence.assess(run.id);
  assert.equal(stale.ready, false); assert.ok(stale.blockers.some(item => /requirements/.test(item)));
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id);
  // The fixture commits directly; real completed candidates retain a matching head checkpoint.
  store.checkpointRepository(run.id, { id: crypto.randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: (await work.status(run.id)).revision });
  const resumed = store.replaceAttempt(coordinator.assignment.id, { automatic: false });
  store.claimAttempt(resumed.id);
  store.recordAttemptIdentity(resumed.id, { hostPid: 50, ompPid: 51, sessionId: 'resumed-coordinator', hostUrl: 'http://127.0.0.1:9000', pane: '%5' });
  const writer = store.runtime(run.id).assignments.find(item => item.name === 'Writer')!;
  const resumedWriter = store.replaceAttempt(writer.id, { automatic: false });
  store.claimAttempt(resumedWriter.id);
  store.recordAttemptIdentity(resumedWriter.id, { hostPid: 52, ompPid: 53, sessionId: 'resumed-writer', hostUrl: 'http://127.0.0.1:9001', pane: '%6' });
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  await assert.rejects(publication.prepare(resumed.id, { candidateId: candidate.id, summary: reviewerSummary }), /locally validated current candidate/);
  assert.equal(store.publication(run.id), undefined);
  const rework = await new Coding(store, work, evidence).claim(resumed.id, 'adopted-scope', { assignee: 'Writer', instruction: 'Implement the adopted empty-input criterion.' });
  assert.equal(rework.automatic, false, 'An operator scope decision is not an automatic repair cycle.');
});

test('DEL-02/04: explicit PR write rejection remains blocked without repeated writes', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-publication-denied-')); const fixture = (await publicationFixture(root));
  let posts = 0;
  const provider = createServer((req, res) => {
    if (req.method === 'POST') { posts++; res.statusCode = 403; res.end('{}'); return; }
    if (req.url === '/2.0/repositories/fixture/repository') res.end(JSON.stringify({ ...remoteIdentity, full_name: 'fixture/repository' }));
    else res.end(JSON.stringify({ values: [] }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const publication = new Publication(fixture.store, fixture.work, fixture.evidence, join(root, 'publication'), [{ repositoryPath: fixture.checkout, repository: remoteIdentity, policy, fixtureGitPath: fixture.remote,
    apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }]);
  t.after(async () => { await publication.close(); fixture.store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  (await publication.prepare(fixture.coordinator.attempt.id, { candidateId: fixture.candidate.id, summary: reviewerSummary }));
  await publication.flush(); assert.equal((await publication.read(fixture.run.id))!.pr.state, 'blocked'); assert.equal(posts, 1);
  await publication.flush(); assert.equal((await publication.read(fixture.run.id))!.pr.state, 'blocked'); assert.equal(posts, 1);
  assert.match((await publication.read(fixture.run.id))!.error!, /403/, 'Reconciliation retains the reason the PR write was rejected.');
  assert.equal(fixture.store.deliveryNotices(fixture.run.id).at(-1)!.payload.prState, 'blocked');
});

test('DEL-03/04/06: durable publication recovers a lost create response and gates readiness on current required observations', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-publication-'));
  const fixture = (await publicationFixture(root)); const { checkout, remote, git, run, candidate, coordinator, owned } = fixture;
  let { store, work, evidence } = fixture;
  let posts = 0, jiraPosts = 0, hidePr = false, wrongRepository = true, mode = 'pending'; let pr: any;
  const provider = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture'); res.setHeader('Content-Type', 'application/json');
    if (url.pathname === '/2.0/repositories/fixture/repository') { res.end(JSON.stringify({ uuid: wrongRepository ? '{00000000-0000-0000-0000-000000000000}' : remoteIdentity.uuid, full_name: 'fixture/repository' })); return; }
    if (url.pathname === '/rest/api/3/issue/100/comment') {
      if (req.method === 'POST') { jiraPosts++; res.statusCode = 201; res.end(JSON.stringify({ id: 'jira-comment-1' })); }
      else res.end(JSON.stringify({ comments: [], startAt: 0, total: 0 }));
      return;
    }
    if (req.method === 'POST') {
      let text = ''; for await (const chunk of req) text += chunk; const input = JSON.parse(text); posts++;
      pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-09-28T00:00:00Z', source: { branch: input.source.branch, commit: { hash: candidate.revision.head }, repository: { uuid: remoteIdentity.uuid } },
        destination: { branch: input.destination.branch, commit: { hash: candidate.revision.base }, repository: { uuid: remoteIdentity.uuid } }, links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
      req.socket.destroy(); return;
    }
    if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr && !hidePr ? [pr] : [] })); return; }
    if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify(pr)); return; }
    if (url.pathname.endsWith('/statuses')) { if (mode === 'outage') { res.statusCode = 503; res.end('{}'); return; } res.end(JSON.stringify({ values: policy.statusKeys.map(key => ({ key, state: mode === 'failed' && key === 'security' ? 'FAILED' : 'SUCCESSFUL', name: key, refname: mode === 'wrong-ref' ? 'another-branch' : owned.branch, url: 'https://example.invalid/check', updated_on: mode === 'stale' ? '2026-09-27T00:00:00Z' : '2026-09-28T00:00:00Z' })) })); return; }
    if (url.pathname.endsWith('/steps/')) { res.end(JSON.stringify({ values: [{ uuid: 'unit', name: 'Unit Tests', state: { name: 'COMPLETED', result: { name: mode === 'skipped' ? 'SKIPPED' : 'SUCCESSFUL' } } }, { uuid: 'renovate', name: 'Renovate', state: { name: 'COMPLETED', result: { name: 'SKIPPED' } } }] })); return; }
    // Mirrors Bitbucket: /commit/<abbreviation> resolves a full hash, a selector-type filter matches no pipelines, and PR pipelines report a 12-character destination commit.
    const commit = url.pathname.match(/\/commit\/([a-f0-9]+)$/);
    if (commit) { res.end(JSON.stringify({ hash: [candidate.revision.head, candidate.revision.base].find(item => item.startsWith(commit[1])) })); return; }
    if (url.searchParams.has('target.selector.type')) { res.end(JSON.stringify({ values: [] })); return; }
    res.end(JSON.stringify({ values: mode === 'pending' ? [] : [{ uuid: '{pipeline}', build_number: 1, run_number: mode === 'stale-retry' ? 1 : 2, created_on: '2026-09-28T00:00:00Z', state: { name: 'COMPLETED', result: { name: mode === 'failed-retry' ? 'FAILED' : 'SUCCESSFUL' } }, target: { type: 'pipeline_pullrequest_target', source: owned.branch, destination: 'main', commit: { hash: candidate.revision.head }, destination_commit: { hash: candidate.revision.base.slice(0, 12) }, pullrequest: { id: 9 }, selector: { type: 'pull-requests', pattern: '**' } } }] }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  let credentials = 0;
  const connections = [{ repositoryPath: checkout, repository: remoteIdentity, policy, fixtureGitPath: remote, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => { if (++credentials === 3) store.setDesiredState(run.id, 'paused'); return 'synthetic'; } }];
  let publication = new Publication(store, work, evidence, join(root, 'publication'), connections);
  t.after(async () => { await publication.close(); store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  await assert.rejects(async () => (await publication.prepare(fixture.reporter.attempt.id, { candidateId: candidate.id, summary: reviewerSummary })), /coordinator/);
  const prepared = (await publication.prepare(coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary }));
  assert.equal(prepared.push.state, 'pending'); assert.equal(posts, 0);
  assert.equal((await publication.prepare(coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary })).id, prepared.id);
  await publication.flush();
  assert.match((await publication.read(run.id))!.error!, /repository identity changed/i);
  assert.throws(() => git(root, '--git-dir', remote, 'show-ref', '--verify', `refs/heads/${owned.branch}`));
  assert.equal(posts, 0, 'A changed repository identity prevents both push and PR creation.');
  wrongRepository = false; credentials = 0;
  await publication.flush(); assert.equal((await publication.read(run.id))!.pr.state, 'pending'); assert.equal(posts, 0, 'Pause during credential resolution prevents PR creation.');
  store.setDesiredState(run.id, 'running');
  git(root, '--git-dir', remote, 'update-ref', `refs/heads/${owned.branch}`, candidate.revision.base);
  await publication.flush(); assert.equal(posts, 0, 'Source drift after a confirmed push prevents creation for the wrong revision.');
  git(root, '--git-dir', remote, 'update-ref', `refs/heads/${owned.branch}`, candidate.revision.head);
  await publication.flush(); let saved = (await publication.read(run.id))!;
  assert.equal(saved.push.state, 'confirmed'); assert.equal(saved.pr.state, 'uncertain'); assert.equal(posts, 1);
  assert.equal(git(root, '--git-dir', remote, 'rev-parse', `refs/heads/${owned.branch}`), candidate.revision.head);
  hidePr = true; await publication.flush(); assert.equal((await publication.read(run.id))!.pr.state, 'uncertain'); assert.equal(posts, 1, 'An inconclusive empty PR lookup never triggers a duplicate create.'); hidePr = false;
  await publication.close(); store.close(); store = new Workbench(join(root, 'workflow.sqlite')); work = new RepositoryWork(store, join(root, 'owned')); evidence = new Evidence(store, work, join(root, 'artifacts'));
  publication = new Publication(store, work, evidence, join(root, 'publication'), connections);
  await publication.flush(); saved = (await publication.read(run.id))!;
  assert.equal(saved.pr.state, 'confirmed'); assert.equal(saved.pr.id, 9); assert.equal(saved.ready, false); assert.equal(posts, 1);
  const delivery = new JiraDelivery(store, new JiraClient({ source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator', name: 'Fixture' }, apiUrl: connections[0].apiUrl, credentials: async () => 'synthetic' }), evidence);
  const link = (await delivery.prepare(fixture.reporter.attempt.id, { kind: 'pr_link', eventKey: 'pr-created', candidateId: candidate.id }));
  assert.ok(link.body!.includes(saved.pr.url!)); assert.ok(link.body!.includes(candidate.revision.head));
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'confirmed'); assert.equal(jiraPosts, 1);
  assert.match(saved.description, new RegExp(candidate.revision.head)); assert.match(saved.description, /unit/);
  mode = 'ready'; await publication.flush(); assert.equal((await publication.read(run.id))!.ready, true);
  mode = 'outage'; await publication.flush();
  assert.equal((await publication.read(run.id))!.ready, false, 'Provider unavailability removes the ready claim.');
  assert.equal(store.publication(run.id)!.reviewInvalidated, false, 'An outage alone does not reserve the coding slot.');
  mode = 'ready'; await publication.flush(); assert.equal((await publication.read(run.id))!.ready, true);
  const check = store.codingRecord(run.id)!.checks.find(item => item.candidateId === candidate.id)!;
  const output = evidence.readArtifact(check.output);
  writeFileSync(join(root, 'artifacts', check.output.id), 'temporarily unreadable evidence');
  await publication.flush();
  assert.equal((await publication.read(run.id))!.ready, false);
  assert.equal(store.publication(run.id)!.reviewInvalidated, false, 'An unreadable local artifact is unavailable evidence, not a confirmed regression.');
  writeFileSync(join(root, 'artifacts', check.output.id), output);
  await publication.flush(); assert.equal((await publication.read(run.id))!.ready, true);
  mode = 'failed-retry'; await publication.flush(); assert.equal((await publication.read(run.id))!.ready, false);
  assert.equal(store.publication(run.id)!.reviewInvalidated, true, 'A confirmed current-head pipeline failure revokes review readiness.');
  mode = 'stale-retry'; await publication.flush(); assert.equal((await publication.read(run.id))!.ready, false, 'An older successful pipeline retry cannot erase the latest failure.');
  for (mode of ['wrong-ref', 'skipped', 'failed']) { await publication.flush(); assert.equal((await publication.read(run.id))!.ready, false); }
  mode = 'stale'; await publication.flush(); assert.equal((await publication.read(run.id))!.ready, false, 'Older successful statuses cannot erase a known failure.');
  mode = 'outage'; await publication.flush();
  assert.equal((await publication.read(run.id))!.ready, false);
  assert.equal(store.publication(run.id)!.reviewInvalidated, true, 'A later outage does not erase a confirmed current-check regression.');
  mode = 'pending'; await publication.flush();
  assert.equal((await publication.read(run.id))!.ready, false);
  assert.equal(store.publication(run.id)!.reviewInvalidated, true, 'A pending observation cannot erase a confirmed failure before readiness returns.');
  mode = 'ready'; await publication.flush();
  assert.equal((await publication.read(run.id))!.ready, true);
  assert.equal(store.publication(run.id)!.reviewInvalidated, false, 'Only fully restored readiness clears the re-hold reason.');
  mode = 'ready'; pr.source.commit.hash = 'c'.repeat(40); await publication.flush(); assert.equal((await publication.read(run.id))!.ready, false);
  pr.source.commit.hash = candidate.revision.head; git(root, '--git-dir', remote, 'update-ref', `refs/heads/${owned.branch}`, candidate.revision.base);
  await publication.flush(); assert.equal((await publication.read(run.id))!.ready, false, 'A stale PR response cannot hide a changed remote branch.');
  assert.equal(store.publication(run.id)!.reviewInvalidated, true, 'A confirmed published revision change revokes review readiness.');
  assert.equal(posts, 1);
});

test('R3: a refinement while the final Git probe is pending blocks stale evidence, publication and Jira reports', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-refinement-race-')); const latch = gitLatch(root);
  const { store, work, evidence, run, candidate, coordinator, reporter, checkout, remote } = await publicationFixture(root);
  let providerRequests = 0;
  const publication = new Publication(store, work, evidence, join(root, 'publication'), [{ repositoryPath: checkout, repository: remoteIdentity, policy, fixtureGitPath: remote,
    apiUrl: 'http://127.0.0.1:9', credentials: async () => { providerRequests++; return 'synthetic'; } }]);
  const delivery = new JiraDelivery(store, new JiraClient({ source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator', name: 'Fixture' },
    credentials: async () => { providerRequests++; return 'synthetic'; } }), evidence);
  t.after(async () => { latch.restore(); await publication.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  const accepted = store.getTask(run.taskId).criteria;
  const refine = (criteria: string[]) => { const task = store.getTask(run.taskId); store.refine(task.id, { revision: task.revision, sourceRevision: task.sourceRevision, brief: task.brief, criteria }); };
  /** Starts a gate, changes the criteria through the refinement interface while its merge-base probe is pending, then releases Git. */
  const raced = async <T>(gate: () => Promise<T>) => {
    assert.equal((await evidence.assess(run.id)).ready, true, 'The candidate is ready before the refinement.');
    latch.arm(); const pending = gate().then(value => ({ value }), (error: Error) => ({ error })); await latch.blocked();
    refine([...accepted, 'Also reject negative input']); latch.release();
    try { return await pending; } finally { refine(accepted); }
  };
  const assessed = await raced(() => evidence.assess(run.id));
  assert.ok('value' in assessed && !assessed.value.ready, 'Readiness returned after the refinement cannot cover the old criteria.');
  assert.ok(assessed.value.blockers.some(reason => /requirements/.test(reason)));
  const status = await raced(() => work.status(run.id));
  assert.ok('value' in status && status.value.drift.requirements, 'Authoritative status reports the refinement that landed during Git.');
  const observed = await raced(() => work.observedStatus(run.id));
  assert.ok('value' in observed && observed.value.drift.requirements, 'Observed status reports the refinement that landed during Git.');
  const published = await raced(() => publication.prepare(coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary }));
  assert.ok('error' in published && /locally validated current candidate/.test(published.error.message), 'Publication cannot be requested for superseded criteria.');
  assert.equal(store.publication(run.id), undefined);
  const reported = await raced(() => delivery.prepare(reporter.attempt.id, { kind: 'evidence', eventKey: 'candidate-ready', candidateId: candidate.id }));
  assert.ok('error' in reported && /current verified/.test(reported.error.message), 'A Jira evidence report cannot cite superseded criteria.');
  assert.deepEqual(delivery.read(run.id), []);
  assert.equal(providerRequests, 0);
  // Control: with the accepted criteria restored, the same gates admit the candidate.
  assert.equal((await publication.prepare(coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary })).candidateId, candidate.id);
  assert.equal((await delivery.prepare(reporter.attempt.id, { kind: 'evidence', eventKey: 'candidate-ready', candidateId: candidate.id })).state, 'pending');
});

test('DEL-04/06: feedback observation retains newer rows and labels partial reads without changing publication readiness', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-feedback-observation-'));
  const fixture = await publicationFixture(root);
  let mode: 'current' | 'older' | 'partial' | 'equal-conflict' | 'missing' | 'deleted' | 'metadata' = 'current'; let writes = 0;
  const original = { id: 51, created_on: '2026-09-28T00:00:00Z', updated_on: '2026-09-28T00:01:00Z', deleted: false,
    content: { raw: 'Use a boundary check' }, inline: { path: 'sum.py', to: 2 }, user: { uuid: '{reviewer}', display_name: 'Reviewer' } };
  const provider = createServer((req, res) => {
    if (req.method !== 'GET') { writes++; res.statusCode = 500; res.end('{}'); return; }
    if (req.url?.includes('/comments')) {
      if (mode === 'partial') { res.statusCode = 503; res.end('{}'); return; }
      if (mode === 'missing') { res.end(JSON.stringify({ values: [] })); return; }
      const comment = mode === 'older' ? { ...original, updated_on: '2026-09-27T00:00:00Z', content: { raw: 'Older text' } }
        : mode === 'equal-conflict' ? { ...original, content: { raw: 'Unexpected same-version text' } }
          : mode === 'deleted' ? { ...original, updated_on: '2026-09-28T00:02:00Z', deleted: true, content: undefined }
            : mode === 'metadata' ? { ...original, inline: { ...original.inline, outdated: true }, user: { ...original.user, display_name: 'Updated reviewer' } } : original;
      res.end(JSON.stringify({ values: [comment] })); return;
    }
    res.statusCode = 404; res.end('{}');
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const address = provider.address(); if (!address || typeof address === 'string') throw new Error('Feedback fixture did not bind.');
  const publication = new Publication(fixture.store, fixture.work, fixture.evidence, join(root, 'publication'), [{ repositoryPath: fixture.checkout,
    repository: remoteIdentity, policy, fixtureGitPath: fixture.remote, apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'synthetic' }]);
  t.after(async () => { await publication.close(); fixture.store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const prepared = await publication.prepare(fixture.coordinator.attempt.id, { candidateId: fixture.candidate.id, summary: reviewerSummary });
  fixture.store.savePublication({ ...prepared, pr: { state: 'confirmed', id: 9, providerState: 'OPEN' } });
  const before = fixture.store.publication(fixture.run.id)!;
  await publication.observeFeedback(fixture.run.id);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].content?.raw, 'Use a boundary check');
  mode = 'metadata'; await publication.observeFeedback(fixture.run.id);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.state, 'complete');
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].inline?.outdated, true);
  mode = 'missing'; await publication.observeFeedback(fixture.run.id);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].currentness, 'unknown');
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].deleted, false, 'Absence is not deletion.');
  mode = 'older'; await publication.observeFeedback(fixture.run.id);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].content?.raw, 'Use a boundary check');
  mode = 'partial'; await publication.observeFeedback(fixture.run.id);
  const partial = fixture.store.publication(fixture.run.id)!;
  assert.equal(partial.feedback?.state, 'unavailable'); assert.equal(partial.feedback?.comments[0].content?.raw, 'Use a boundary check');
  mode = 'equal-conflict'; await publication.observeFeedback(fixture.run.id);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].currentness, 'conflict');
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].content?.raw, 'Use a boundary check');
  mode = 'deleted'; await publication.observeFeedback(fixture.run.id);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].deleted, true);
  assert.equal(fixture.store.publication(fixture.run.id)?.feedback?.comments[0].currentness, 'known');
  assert.deepEqual([partial.push, partial.pr, partial.observation, partial.reviewInvalidated], [before.push, before.pr, before.observation, before.reviewInvalidated]);
  assert.equal(writes, 0, 'Feedback observation performs no provider mutation.');
});
