import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { DatabaseSync, backup } from 'node:sqlite';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import { Publication } from '../server/publication';
import { PublicationGit } from '../server/publication-git';
import { TargetRefresh } from '../server/target-refresh';
import { RepositoryWork } from '../server/repository-work';
import { Workbench } from '../server/workbench';
import { Coding } from '../server/coding';
import { Evidence } from '../server/evidence';
import { Results } from '../server/results';
import { RuntimeSupervisor } from '../server/runtime/supervisor';
import { Messaging } from '../server/runtime/messaging';
import { publicationFixture, reviewerSummary } from './fixtures/publication';
import { startServer } from '../server/app';
import { gitLatch } from './fixtures/git-latch';
import { acquireController, inspectCheckedOwned, launchCheckedOwned, prepareCheckedOwned } from '../server/runtime/processes';

const remoteIdentity = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
const policy = { statusKeys: ['build'], pipelineSteps: ['Unit Tests'], optionalSteps: [] };

async function setup(t: TestContext, prefix: string) {
  const root = mkdtempSync(join(tmpdir(), prefix)); const fixture = await publicationFixture(root);
  const authority = await acquireController(join(root, 'promotion-controller'));
  fixture.work.setPromotionAuthority(authority);
  const { git, remote, store, run } = fixture;
  // Another developer's clone advances the remote target; the operator checkout's refs stay where they were.
  const other = join(root, 'other'); git(root, 'clone', '-q', remote, other); git(other, 'config', 'user.name', 'Other'); git(other, 'config', 'user.email', 'other@example.invalid');
  const advance = (file: string, content: string) => { writeFileSync(join(other, file), content); git(other, 'add', '.'); git(other, 'commit', '-qm', `Change ${file}`); git(other, 'push', '-q', 'origin', 'main'); return git(other, 'rev-parse', 'HEAD'); };
  const rewind = (head: string) => { git(other, 'reset', '-q', '--hard', head); git(other, 'push', '-qf', 'origin', 'main'); };
  let posts = 0, puts = 0; let pr: any; let gate: { arrived: () => void; released: Promise<void> } | undefined; let hostedDown = false;
  const remoteHead = (ref: string) => git(root, '--git-dir', remote, 'rev-parse', ref);
  const provider = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture'); res.setHeader('Content-Type', 'application/json');
    let text = ''; for await (const chunk of req) text += chunk;
    if (url.pathname === '/2.0/repositories/fixture/repository') { if (gate) { gate.arrived(); await gate.released; } res.end(JSON.stringify({ uuid: remoteIdentity.uuid, full_name: 'fixture/repository' })); return; }
    // Mirrors Bitbucket: pull-request searches above 50 per page are rejected.
    if (url.pathname.endsWith('/pullrequests') && req.method === 'GET' && Number(url.searchParams.get('pagelen')) > 50) { res.statusCode = 400; res.end(JSON.stringify({ type: 'error', error: { message: 'Invalid pagelen' } })); return; }
    // Mirrors Bitbucket: PR objects carry 12-character hashes; /commit/<abbreviation> resolves the full one.
    const commit = url.pathname.match(/\/commit\/([a-f0-9]+)$/);
    if (commit) { res.end(JSON.stringify({ hash: git(root, '--git-dir', remote, 'rev-parse', commit[1]) })); return; }
    if (pr) { pr.source.commit.hash = remoteHead(`refs/heads/${pr.source.branch.name}`).slice(0, 12); pr.destination.commit.hash = remoteHead('refs/heads/main').slice(0, 12); }
    if (req.method === 'POST') {
      const input = JSON.parse(text); posts++;
      pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-09-29T00:00:00Z', source: { branch: input.source.branch, commit: { hash: input.source.commit.hash.slice(0, 12) }, repository: { uuid: remoteIdentity.uuid } },
        destination: { branch: input.destination.branch, commit: { hash: remoteHead('refs/heads/main').slice(0, 12) }, repository: { uuid: remoteIdentity.uuid } }, links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
      res.end(JSON.stringify(pr)); return;
    }
    if (req.method === 'PUT' && url.pathname.endsWith('/pullrequests/9')) { const input = JSON.parse(text); puts++; pr = { ...pr, title: input.title, description: input.description, updated_on: '2026-09-29T01:00:00Z' }; res.end(JSON.stringify(pr)); return; }
    if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr ? [pr] : [] })); return; }
    if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify(pr)); return; }
    if (hostedDown && url.pathname.endsWith('/statuses')) { res.statusCode = 503; res.end('{}'); return; }
    res.end(JSON.stringify({ values: [] }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const connection = { repositoryPath: fixture.checkout, repository: remoteIdentity, policy, fixtureGitPath: remote,
    apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' };
  const publication = new Publication(store, fixture.work, fixture.evidence, join(root, 'publication'), [connection]);
  const refresh = new TargetRefresh(store, fixture.work, fixture.evidence, publication);
  t.after(async () => { await authority.close(); await publication.close(); store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const assignments = () => store.runtime(run.id).assignments;
  /** Pause with every agent confirmed stopped, as the supervisor leaves a run before an operator transition. */
  const stop = () => {
    store.setDesiredState(run.id, 'paused'); store.beginPause(run.id);
    for (const assignment of assignments()) { const attempt = store.getAttempt(assignment.currentAttemptId); if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true); }
    store.finishPause(run.id);
  };
  /** Resume with replacement attempts, as the supervisor does after the operator selects Resume. */
  const resume = () => {
    const { controlRevision } = store.setDesiredState(run.id, 'running');
    const attempts = assignments().map(assignment => store.replaceAttempt(assignment.id, { automatic: false }));
    store.prepareContinuation(run.id, controlRevision!);
    for (const [i, attempt] of attempts.entries()) { store.claimAttempt(attempt.id); store.recordAttemptIdentity(attempt.id, { hostPid: 20 + i, ompPid: 30 + i, sessionId: `resumed-${i}`, hostUrl: `http://127.0.0.1:${9100 + i}`, pane: `%${10 + i}` }); }
    store.startExecution(run.id);
    return Object.fromEntries(assignments().map(item => [item.name, item.currentAttemptId]));
  };
  /** Execute the actual continuation and launch admission against controlled host transport. */
  const resumeProduction = async () => {
    const hostRoot = join(root, 'hosts');
    const runtime = new RuntimeSupervisor(store, { root: hostRoot, socket: join(root, 'tmux.sock'), ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], policy: {},
      models: { providers: { fixture: { api: 'openai-responses', baseUrl: 'http://127.0.0.1:9/v1', apiKey: 'fixture' } } } }, 'http://127.0.0.1:9', () => [], fixture.work);
    const seam = runtime as unknown as { openPane: (name: string, command: string[]) => Promise<string>;
      tmux: (args: string[]) => Promise<string>; request: (host: { attemptId: string }, body: { op: string }) => Promise<unknown>;
      hosts: Map<string, unknown>; stopObserver: (host: unknown) => Promise<void> };
    const originalRegister = Messaging.prototype.register;
    Messaging.prototype.register = async () => {};
    let launched = 0;
    seam.openPane = async () => {
      launched++;
      const directory = readdirSync(hostRoot).map(name => join(hostRoot, name)).find(path => existsSync(join(path, 'host.json')) && !existsSync(join(path, 'endpoint.json')))!;
      writeFileSync(join(directory, 'endpoint.json'), JSON.stringify({ port: 9500 + launched }));
      return `%${launched}`;
    };
    seam.tmux = async () => '';
    seam.request = async (_host, body) => body.op === 'identity' ? { hostPid: 0, ompPid: 0, sessionId: 'scripted' }
      : body.op === 'events' ? [] : {};
    try {
      await runtime.resume(run.id);
      assert.equal(store.getRun(run.id).status, 'running');
      assert.equal(launched, assignments().length, 'The real supervisor launch gate admitted the stopped team.');
      return Object.fromEntries(assignments().map(item => [item.name, item.currentAttemptId]));
    } finally {
      Messaging.prototype.register = originalRegister;
      for (const host of seam.hosts.values()) await seam.stopObserver(host);
      seam.hosts.clear();
      await runtime.close();
    }
  };
  /** Holds the next repository-identity request until released; `arrived` resolves once a flush is waiting on it. */
  const hold = () => {
    let arrived!: () => void, release!: () => void;
    const signal = new Promise<void>(resolve => { arrived = resolve; });
    gate = { arrived, released: new Promise<void>(resolve => { release = () => { gate = undefined; resolve(); }; }) };
    return { arrived: signal, release };
  };
  return { ...fixture, authority, root, advance, rewind, remoteHead, publication, connection, refresh, stop, resume, resumeProduction, hold, counts: () => ({ posts, puts }), pr: () => pr, hosted: (up: boolean) => { hostedDown = !up; } };
}

/** Historical promoting records cannot be manufactured with today's admission method. */
async function legacyPromotion(store: Workbench, work: RepositoryWork, root: string, runId: string, requestId: string, operationId: string, toHead: string) {
  const process = await work.describePromotionProcess(store.repositoryWork(runId)!, operationId, toHead);
  store.beginTargetPromotion(runId, requestId, operationId, toHead, process);
  work.preparePromotionProcess(runId, requestId, operationId);
  const record = store.repositoryWork(runId)!;
  delete record.refreshes!.find(item => item.requestId === requestId)!.resolutionWorkspace!.promotionProcess;
  const db = new DatabaseSync(join(root, 'workflow.sqlite'));
  try { db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(record), runId); }
  finally { db.close(); }
}

test('M6.3: Git transport rejects shared, malformed and operator-owned target destinations', async t => {
  const f = await setup(t, 'crewroom-refresh-namespace-');
  const transport = new PublicationGit(join(f.root, 'namespace-transport'), { fixturePath: f.remote });
  t.after(() => transport.close());
  for (const destination of [`refs/crewroom/${f.run.id}/target`, `refs/crewroom/${f.run.id}/refresh/not-a-uuid/target`, 'refs/heads/main'])
    await assert.rejects(transport.fetchTarget({ directory: f.checkout, branch: 'main', destination }));
  assert.equal(f.git(f.checkout, 'for-each-ref', '--format=%(refname)', 'refs/crewroom/'), `refs/crewroom/${f.run.id}/start`);
  assert.equal(f.git(f.checkout, 'rev-parse', 'main'), f.candidate.revision.base);
});

test('M6.3: a remote advancing before ordinary rebase receipt retains the original branch and PR', async t => {
  const f = await setup(t, 'crewroom-refresh-live-rebase-');
  const { store, run, work, owned, git, candidate } = f;
  await f.publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary });
  await f.publication.flush(); const published = f.remoteHead(`refs/heads/${owned.branch}`);
  const original = git(owned.path, 'rev-parse', 'HEAD');
  f.stop(); f.advance('unrelated', 'first remote change\n');
  let observed = 0;
  work.setTargetObserver(async () => { observed++; f.advance('another', 'second remote change\n'); return f.remoteHead('refs/heads/main'); });
  const requestId = randomUUID();
  await assert.rejects(f.refresh.refresh(run.id, { requestId }), /live target changed/);
  assert.equal(observed, 1);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'conflicted');
  assert.equal(store.repositoryWork(run.id)!.base, candidate.revision.base);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), original);
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), published);
});

test('M6.3: ownership changing while observing the live target cannot commit a rebase receipt or reset new work', async t => {
  const f = await setup(t, 'crewroom-refresh-observer-ownership-');
  const { store, run, work, owned, git, candidate } = f;
  f.stop(); f.advance('unrelated', 'target change\n');
  work.setTargetObserver(async () => { f.resume(); return f.remoteHead('refs/heads/main'); });
  const id = randomUUID();
  await assert.rejects(f.refresh.refresh(run.id, { requestId: id }), /source or ownership changed/);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'rebasing', 'Uncertain ownership is retained for reconciliation, not silently completed.');
  assert.equal(store.repositoryWork(run.id)!.base, candidate.revision.base);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD^'), f.remoteHead('refs/heads/main'), 'No unsafe reset was attempted after the run resumed.');
});

test('M6.3: first ordinary no-op has a candidate-bound receipt and concurrent same-key replay cannot refetch it', async t => {
  const f = await setup(t, 'crewroom-refresh-noop-race-');
  const { store, run, candidate, owned, git } = f;
  f.stop(); const before = store.repositoryWork(run.id)!;
  const notices = store.deliveryNotices(run.id).length;
  const id = randomUUID(); const original = f.publication.fetchTarget.bind(f.publication);
  let fetched!: () => void; let release!: () => void; let fetches = 0;
  const arrived = new Promise<void>(resolve => { fetched = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  f.publication.fetchTarget = async (...args) => {
    const snapshot = await original(...args); fetches++; fetched(); await gate; return snapshot;
  };
  const first = f.refresh.refresh(run.id, { requestId: id });
  await arrived;
  const pinned = `refs/crewroom/${run.id}/refresh/${id}/target`;
  assert.equal(git(owned.path, 'rev-parse', pinned), before.base);
  const replay = new TargetRefresh(store, f.work, f.evidence, f.publication).refresh(run.id, { requestId: id });
  const moved = f.advance('unrelated', 'next remote target\n');
  release();
  const [completed, duplicate] = await Promise.all([first, replay]);
  assert.equal(completed.changed, false); assert.equal(duplicate.changed, false);
  assert.deepEqual(duplicate, completed);
  assert.equal(completed.refresh?.candidateId, candidate.id);
  assert.equal(store.repositoryWork(run.id)!.targetRef, before.targetRef);
  assert.equal(store.repositoryWork(run.id)!.base, before.base);
  assert.equal(store.repositoryWork(run.id)!.checkpoints.length, before.checkpoints.length);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 1);
  assert.equal(store.deliveryNotices(run.id).length, notices);
  assert.equal(fetches, 1); assert.equal(git(owned.path, 'rev-parse', pinned), before.base);
  assert.equal(f.remoteHead('refs/heads/main'), moved);
  await assert.rejects(original(run.id, id), /recorded target request/);
  assert.equal(git(owned.path, 'rev-parse', pinned), before.base);
  const fresh = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(fresh.changed, true); assert.equal(fetches, 2);
});

test('M6.3: rewritten-target rejection leaves adopted and retained-conflict snapshots untouched', async t => {
  const f = await setup(t, 'crewroom-refresh-rewrite-isolation-');
  const { store, run, owned, git, candidate } = f;
  await f.publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary });
  await f.publication.flush(); const published = f.remoteHead(`refs/heads/${owned.branch}`);
  f.stop(); const acceptedBase = f.advance('unrelated', 'first target\n');
  await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const accepted = store.repositoryWork(run.id)!;
  const retainedBase = f.advance('answer', 'correct');
  const held = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  assert.equal(held.state, 'conflicted'); assert.equal(held.resolutionWorkspace, undefined);
  const originalHead = git(owned.path, 'rev-parse', 'HEAD');
  f.rewind(candidate.revision.base);
  const rewritten = f.advance('unrelated', 'different remote history\n');
  const retry = randomUUID();
  await assert.rejects(f.refresh.refresh(run.id, { requestId: retry, operatorDecision: { conflictRequestId: held.requestId, reason: 'Inspect rewritten target.' } }), /rewritten/);
  assert.equal(git(owned.path, 'rev-parse', `refs/crewroom/${run.id}/refresh/${retry}/target`), rewritten, 'Rejected staging snapshot may remain unreferenced.');
  assert.equal(git(owned.path, 'rev-parse', accepted.targetRef), acceptedBase);
  assert.equal(git(owned.path, 'rev-parse', held.targetRef), retainedBase);
  assert.equal(store.repositoryWork(run.id)!.targetRef, accepted.targetRef);
  assert.equal(store.repositoryWork(run.id)!.base, acceptedBase);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.requestId, held.requestId);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.supersededBy, undefined);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), originalHead);
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), published);
});

test('M6.3: admission changing after a real fetch cannot change the accepted snapshot', async t => {
  const f = await setup(t, 'crewroom-refresh-late-admission-');
  const { store, run, owned, git } = f;
  f.stop(); f.advance('unrelated', 'first target\n');
  await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const accepted = store.repositoryWork(run.id)!;
  const original = f.publication.fetchTarget.bind(f.publication);
  let arrived!: () => void; let release!: () => void;
  const fetched = new Promise<void>(resolve => { arrived = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  f.publication.fetchTarget = async (...args) => { const snapshot = await original(...args); arrived(); await gate; return snapshot; };
  f.advance('unrelated', 'second target\n');
  const id = randomUUID(); const pending = f.refresh.refresh(run.id, { requestId: id });
  await fetched;
  f.resume(); release();
  await assert.rejects(pending, /Pause the run/);
  assert.equal(git(owned.path, 'rev-parse', accepted.targetRef), accepted.base);
  assert.equal(store.repositoryWork(run.id)!.targetRef, accepted.targetRef);
  assert.equal(store.repositoryWork(run.id)!.base, accepted.base);
  assert.equal(store.repositoryWork(run.id)!.refreshes?.some(item => item.requestId === id), false);
  f.stop();
});

test('M6.3: losing a configured connection cannot reinterpret an adopted snapshot as live', async t => {
  const f = await setup(t, 'crewroom-refresh-disconnected-');
  const { root, store, run, work, evidence } = f;
  f.stop(); f.advance('unrelated', 'remote target\n');
  await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const accepted = store.repositoryWork(run.id)!;
  const offline = new Publication(store, work, evidence, join(root, 'publication-offline'), []);
  t.after(() => offline.close());
  await assert.rejects(new TargetRefresh(store, work, evidence, offline).refresh(run.id, { requestId: randomUUID() }), /remote target is unavailable/);
  assert.equal(store.repositoryWork(run.id)!.base, accepted.base);
  assert.equal(store.repositoryWork(run.id)!.targetRef, accepted.targetRef);
});

test('M6.3: a rejected refresh after an adopted target cannot strand a requested completion', async t => {
  const f = await setup(t, 'crewroom-refresh-pending-completion-');
  const { store, run, work, evidence, owned, git } = f;
  f.stop(); f.advance('unrelated', 'first target change\n');
  await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const accepted = store.repositoryWork(run.id)!;
  const acceptedRef = accepted.targetRef; const acceptedBase = accepted.base;
  const resumed = f.resume(); const coding = new Coding(store, work, evidence);
  evidence.assignReview(resumed.Coordinator, 'pending-review', { assignee: 'Reviewer', candidateId: store.codingRecord(run.id)!.candidates.at(-1)!.id, instruction: 'Check target adjustment.' });
  evidence.review(resumed.Reviewer, { candidateId: store.codingRecord(run.id)!.candidates.at(-1)!.id, verdict: 'changes_requested', summary: 'Recheck the answer before completion.', findings: [{ severity: 'major', body: 'Clarify answer.' }], coverage: [] });
  await coding.claim(resumed.Coordinator, 'pending-claim', { assignee: 'Writer', instruction: 'Finish the current answer.' });
  const completion = await coding.requestCompletion(resumed.Writer, 'pending-completion', { summary: 'The accepted answer is complete.' });
  store.beginPause(run.id);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  f.advance('unrelated', 'second target change\n');
  assert.equal((await new Results(store, work, evidence, f.publication).read(run.id)).refreshable, false);
  const rejectedId = randomUUID();
  await assert.rejects(f.refresh.refresh(run.id, { requestId: rejectedId }), /pending implementation completion/);
  assert.equal(git(owned.path, 'for-each-ref', '--format=%(refname)', `refs/crewroom/${run.id}/refresh/${rejectedId}/`), '', 'Preflight rejects before the network fetch.');
  assert.equal(store.repositoryWork(run.id)!.base, acceptedBase);
  assert.equal(store.repositoryWork(run.id)!.targetRef, acceptedRef);
  assert.equal(git(owned.path, 'rev-parse', acceptedRef), acceptedBase);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), completion.head);
  assert.equal((await coding.complete(run.id, completion.id)).revision.base, acceptedBase);
});

test('M6.3: a moved target rebases the owned branch into a new candidate; the PR follows it only after its own checks and review', async t => {
  const f = await setup(t, 'crewroom-refresh-');
  const { store, run, evidence, work, candidate, owned, git, checkout } = f;
  await f.publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary }); await f.publication.flush();
  let published = (await f.publication.read(run.id))!;
  assert.equal(published.push.state, 'confirmed'); assert.equal(published.pr.state, 'confirmed'); assert.deepEqual(f.counts(), { posts: 1, puts: 0 });

  const moved = f.advance('unrelated', 'target change\n');
  await f.publication.flush();
  assert.ok((await f.publication.read(run.id))!.blockers.includes('The remote target does not match the validated base.'));
  assert.equal(store.publication(run.id)?.reviewInvalidated, true, 'An observed target advance revokes readiness, not just the ready label.');

  await assert.rejects(f.refresh.refresh(run.id, { requestId: randomUUID() }), /Pause the run/, 'An executing run cannot be rebased under its agents.');
  f.stop();
  const requestId = randomUUID();
  const refreshed = await f.refresh.refresh(run.id, { requestId });
  assert.equal(refreshed.changed, true);
  const current = await work.status(run.id);
  assert.equal(current.base, moved); assert.equal(current.revision.base, moved); assert.equal(current.targetRef, `refs/crewroom/${run.id}/refresh/${requestId}/target`);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD^'), moved, 'The candidate commit now sits on the moved target.');
  assert.equal(git(owned.path, 'show', 'HEAD:answer'), 'correct'); assert.equal(git(owned.path, 'show', 'HEAD:unrelated'), 'target change');
  assert.equal(git(checkout, 'rev-parse', 'main'), candidate.revision.base, "The operator's own target branch is untouched.");
  const next = store.codingRecord(run.id)!.candidates.at(-1)!;
  assert.equal(next.id, refreshed.candidateId); assert.equal(next.refreshedFrom, candidate.id); assert.deepEqual(next.implementationAttempts, candidate.implementationAttempts);
  const stale = await evidence.assess(run.id);
  assert.equal(stale.ready, false); assert.equal(stale.candidate!.id, next.id);
  assert.ok(stale.blockers.includes('Required check unit is missing or failed.')); assert.ok(stale.blockers.includes('Independent review is missing for this revision.'));
  assert.equal(store.deliveryNotices(run.id).at(-1)!.payload.kind, 'target_refresh');
  assert.equal(store.deliveryNotices(run.id).at(-1)!.payload.candidateId, next.id);

  const replay = await f.refresh.refresh(run.id, { requestId });
  assert.equal(replay.changed, true); assert.equal(replay.candidateId, next.id); assert.equal(store.codingRecord(run.id)!.candidates.length, 2, 'A replayed request records no second candidate.');
  assert.equal((await f.refresh.refresh(run.id, { requestId: randomUUID() })).changed, false, 'An unmoved target changes nothing.');

  const attempts = f.resume();
  await assert.rejects(f.publication.prepare(attempts.Coordinator, { candidateId: next.id, summary: reviewerSummary }), /locally validated/, 'Evidence for the earlier candidate never covers the refreshed one.');
  const check = store.codingRecord(run.id)!.requiredChecks[0];
  evidence.recordCheck(run.id, { candidateId: next.id, attemptId: attempts.Reviewer, checkId: 'unit', definition: check, environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 0, timedOut: false,
    output: evidence.artifact('Controlled receipt for the refreshed candidate.'), source: evidence.artifact(JSON.stringify(await work.listFiles(run.id))), sourceIntact: true, started: true, completed: true });
  evidence.assignReview(attempts.Coordinator, 'review-refreshed', { assignee: 'Reviewer', candidateId: next.id, instruction: 'Review the refreshed candidate' });
  evidence.review(attempts.Reviewer, { candidateId: next.id, verdict: 'pass', summary: 'Still correct on the new base', findings: [], coverage: [{ criterion: 0, assessment: 'Reviewed output', checkIds: ['unit'] }] });

  // A periodic flush still holding the earlier record must not overwrite the successor saved while it waits.
  const held = f.hold(); const flushing = f.publication.flush(); await held.arrived;
  const assess = evidence.assess.bind(evidence); let assessed!: () => void; const reached = new Promise<void>(resolve => { assessed = resolve; });
  evidence.assess = async (...args) => { const result = await assess(...args); assessed(); return result; };
  const preparing = f.publication.prepare(attempts.Coordinator, { candidateId: next.id, summary: reviewerSummary });
  await reached; evidence.assess = assess; held.release(); await Promise.all([flushing, preparing]);
  assert.equal(store.publication(run.id)!.candidateId, next.id, 'The successor survives a flush that loaded the earlier record.');
  published = (await f.publication.read(run.id))!;
  assert.equal(published.candidateId, next.id); assert.equal(published.history!.length, 1); assert.equal(published.history![0].pushedHead, candidate.revision.head);
  assert.equal(published.push.expectedHead, candidate.revision.head, 'The lease expects exactly what was pushed before.');
  await f.publication.flush(); published = (await f.publication.read(run.id))!;
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), next.revision.head); assert.equal(published.push.state, 'confirmed');
  assert.deepEqual(f.counts(), { posts: 1, puts: 1 }, 'The existing PR is updated, never duplicated.');
  assert.ok(f.pr().description.startsWith(reviewerSummary), 'The reviewer summary leads the PR description.');
  assert.match(f.pr().description, new RegExp(`Candidate commit \`${next.revision.head}\``)); assert.match(f.pr().description, new RegExp(`Rebased onto the target: \`${candidate.revision.base.slice(0, 12)}\` → \`${moved.slice(0, 12)}\``));
  assert.ok(!published.blockers.includes('The remote target does not match the validated base.'));
  await f.publication.flush(); assert.equal(f.counts().puts, 1, 'A described PR is not rewritten again.');
  // A revised summary for the same candidate rewrites the description once, with no new push or PR.
  const revised = `${reviewerSummary}\n\n## Risks\n\nNone beyond the answer file.`;
  const pending = await f.publication.prepare(attempts.Coordinator, { candidateId: next.id, summary: revised });
  assert.ok(pending.blockers.includes('The PR description update has not been confirmed.'), 'An unwritten description is not ready for review.');
  await f.publication.flush(); await f.publication.flush();
  assert.deepEqual(f.counts(), { posts: 1, puts: 2 }); assert.ok(f.pr().description.startsWith(revised));
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), next.revision.head);
  // Only a merge into the published target at the published head completes delivery.
  const pr = f.pr(); pr.state = 'MERGED'; pr.merge_commit = { hash: next.revision.head.slice(0, 12) }; pr.destination.branch.name = 'release';
  await f.publication.flush(); assert.equal(store.publication(run.id)!.merged, undefined, 'A PR retargeted and merged elsewhere is not a delivery.');
  // Hosted checks are unavailable, but the PR itself reports the merge.
  f.hosted(false); pr.destination.branch.name = 'main'; await f.publication.flush();
  assert.equal(store.publication(run.id)!.merged!.commit, next.revision.head);
  await assert.rejects(f.publication.prepare(attempts.Coordinator, { candidateId: next.id, summary: `${revised}\n\nLate edit.` }), /merged/);
});

test('M6.3: a conflicting target is retained in an isolated resolution worktree while the owned branch stays unchanged', async t => {
  const f = await setup(t, 'crewroom-refresh-conflict-');
  const { store, run, work, owned, git, candidate } = f;
  f.advance('answer', 'different\n'); f.stop(); const before = (await work.status(run.id)).drift;
  const result = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(result.changed, false);
  assert.equal(result.refresh?.state, 'conflicted');
  const saved = store.repositoryWork(run.id)!; const refresh = saved.refreshes!.at(-1)!;
  assert.deepEqual(refresh.conflicts, ['answer']); assert.ok(refresh.resolutionWorkspace);
  const workspace = refresh.resolutionWorkspace.path;
  assert.notEqual(workspace, owned.path); assert.ok(existsSync(workspace));
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.deepEqual(git(workspace, 'diff', '--name-only', '--diff-filter=U', '-z').split('\0').filter(Boolean), ['answer']);
  assert.equal((await work.validateConflictWorkspace(run.id)).state, 'conflicted');
  assert.equal(store.codingRecord(run.id)!.candidates.length, 1);
  assert.deepEqual((await work.status(run.id)).drift, before);
  const blocked = await f.evidence.assess(run.id);
  assert.equal(blocked.ready, false, 'The old candidate cannot be ready while its target conflict is unresolved.');
  assert.match(blocked.blockers.join(' '), /target conflict/i);
});

test('M6.3: a target that already contains part of a candidate records an operator hold without moving the owned branch', async t => {
  const f = await setup(t, 'crewroom-refresh-contained-');
  const { store, run, work, owned, git, candidate } = f;
  f.advance('answer', 'correct'); f.stop(); const before = (await work.status(run.id)).drift;
  const result = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(result.changed, false);
  assert.equal(result.refresh?.state, 'conflicted');
  assert.equal(result.refresh?.resolutionWorkspace, undefined);
  assert.deepEqual(result.refresh?.conflicts, []);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.deepEqual((await work.status(run.id)).drift, before);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 1);
  await assert.rejects(f.refresh.refresh(run.id, { requestId: randomUUID() }), /operator reason/i);
  f.advance('answer', 'target moved again\n');
  const retryId = randomUUID(); const reason = 'The target changed again after I reviewed the contained revision.';
  const retry = await f.refresh.refresh(run.id, { requestId: retryId, operatorDecision: { conflictRequestId: result.refresh!.requestId, reason } });
  assert.equal(retry.refresh?.state, 'conflicted'); assert.equal(retry.refresh?.requestId, retryId);
  const history = store.repositoryWork(run.id)!.refreshes!;
  assert.equal(history.find(item => item.requestId === result.refresh!.requestId)?.supersededBy?.requestId, retryId);
  assert.equal(history.find(item => item.requestId === result.refresh!.requestId)?.supersededBy?.reason, reason);
  assert.equal(history.length, 2);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
});

test('M6.3: a reasoned retry at the unchanged target supersedes its hold without changing the candidate', async t => {
  const f = await setup(t, 'crewroom-refresh-unchanged-retry-');
  const { store, run, work, owned, git } = f;
  await f.publication.prepare(f.coordinator.attempt.id, { candidateId: f.candidate.id, summary: reviewerSummary });
  await f.publication.flush();
  const pushedHead = f.remoteHead(`refs/heads/${owned.branch}`); const originalPr = f.pr().id;
  f.advance('other', 'target-only\n'); f.stop();
  const moved = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(moved.refresh?.state, 'completed');
  const current = store.codingRecord(run.id)!.candidates.at(-1)!;
  const before = store.repositoryWork(run.id)!; const base = before.base; const targetRef = before.targetRef;
  assert.equal(current.revision.head, git(owned.path, 'rev-parse', 'HEAD'));
  f.advance('answer', 'correct');
  const held = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(held.refresh?.state, 'conflicted'); assert.equal(held.refresh?.resolutionWorkspace, undefined);
  f.rewind(base);
  await assert.rejects(f.refresh.refresh(run.id, { requestId: randomUUID() }), /operator reason/i);
  await assert.rejects(f.refresh.refresh(run.id, { requestId: randomUUID(), operatorDecision: { conflictRequestId: randomUUID(), reason: 'Wrong hold' } }), /operator reason/i);
  assert.equal(work.unresolvedConflict(run.id)?.requestId, held.refresh!.requestId);
  const retryId = randomUUID(); const reason = 'Reviewed the target rollback and confirmed the current base.';
  const decision = { conflictRequestId: held.refresh!.requestId, reason };
  const head = git(owned.path, 'rev-parse', 'HEAD'); const candidateCount = store.codingRecord(run.id)!.candidates.length;
  const beforeNotices = store.deliveryNotices(run.id).length; const beforeCheckpoints = store.repositoryWork(run.id)!.checkpoints.length;
  const retry = await f.refresh.refresh(run.id, { requestId: retryId, operatorDecision: decision });
  assert.equal(retry.changed, false);
  assert.equal(retry.refresh?.requestId, retryId);
  assert.equal(retry.refresh?.state, 'completed');
  assert.equal(retry.refresh?.fromBase, base); assert.equal(retry.refresh?.toBase, base);
  assert.equal(retry.refresh?.fromHead, head); assert.equal(retry.refresh?.toHead, head);
  assert.deepEqual(retry.refresh?.supersedes, { requestId: held.refresh!.requestId, reason });
  const history = store.repositoryWork(run.id)!.refreshes!;
  assert.deepEqual(history.find(item => item.requestId === held.refresh!.requestId)?.supersededBy, { requestId: retryId, reason, at: retry.refresh!.at });
  assert.equal(work.unresolvedConflict(run.id), undefined);
  assert.equal(store.repositoryWork(run.id)!.base, base); assert.equal(store.repositoryWork(run.id)!.targetRef, targetRef);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), head);
  assert.equal(store.codingRecord(run.id)!.candidates.length, candidateCount);
  assert.equal(store.repositoryWork(run.id)!.checkpoints.length, beforeCheckpoints);
  assert.equal(store.deliveryNotices(run.id).length, beforeNotices, 'An unchanged target sends no new-candidate notice.');
  const assessment = await f.evidence.assess(run.id);
  assert.equal(assessment.ready, false, 'Clearing the hold cannot claim missing new-candidate evidence.');
  assert.ok(assessment.blockers.some(reason => /check|review/i.test(reason)));
  assert.equal(store.getRun(run.id).status, 'paused'); assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), pushedHead); assert.equal(f.pr().id, originalPr);
  const app = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath: join(f.root, 'roles.json') });
  t.after(app.close);
  const view = await (await fetch(`${app.origin}/api/runs/${run.id}/result`)).json();
  assert.equal(view.refreshes.at(-1).requestId, retryId);
  assert.equal(view.refreshes.at(-1).candidateId, current.id);
  assert.equal(view.assessment.ready, false);
  assert.equal(view.assessment.blockers.some((reason: string) => /target conflict/i.test(reason)), false);
  const replay = await f.refresh.refresh(run.id, { requestId: retryId, operatorDecision: decision });
  assert.equal(replay.changed, false); assert.equal(replay.refresh?.requestId, retryId);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.length, history.length);
  assert.equal(store.codingRecord(run.id)!.candidates.length, candidateCount);
  await assert.rejects(f.refresh.refresh(run.id, { requestId: retryId, operatorDecision: { ...decision, reason: 'Different decision' } }), /different operator decision/);
  f.advance('answer', 'different again\n');
  const newer = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(newer.refresh?.state, 'conflicted');
  // Simulate a crash after R's atomic receipt/supersession save but before its candidate association.
  const db = new DatabaseSync(join(f.root, 'workflow.sqlite'));
  try {
    const persisted = store.repositoryWork(run.id)!;
    persisted.refreshes!.find(item => item.requestId === retryId)!.candidateId = undefined;
    db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(persisted), run.id);
  } finally { db.close(); }
  const reopened = new Workbench(join(f.root, 'workflow.sqlite'));
  t.after(() => reopened.close());
  const recoveredWork = new RepositoryWork(reopened, join(f.root, 'owned'));
  const historical = await new TargetRefresh(reopened, recoveredWork, new Evidence(reopened, recoveredWork, join(f.root, 'artifacts')))
    .refresh(run.id, { requestId: retryId, operatorDecision: decision });
  assert.equal(historical.changed, false); assert.equal(historical.refresh?.requestId, retryId);
  assert.equal(historical.refresh?.candidateId, current.id);
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === retryId)?.candidateId, current.id);
  assert.equal(reopened.codingRecord(run.id)!.candidates.length, candidateCount);
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.length, history.length + 1);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.requestId, newer.refresh!.requestId);
  assert.equal(work.unresolvedConflict(run.id)?.requestId, newer.refresh!.requestId, 'Historical replay cannot clear the newer hold.');
  assert.equal((await f.refresh.refresh(run.id, { requestId: held.refresh!.requestId })).refresh?.state, 'conflicted');
});

test('M6.3: a no-edit completion creates a distinct same-head candidate that an unchanged retry retains', async t => {
  const f = await setup(t, 'crewroom-refresh-same-head-');
  const { store, run, work, evidence, owned, git } = f;
  f.stop(); f.advance('other', 'target-only\n');
  const moved = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(moved.refresh?.state, 'completed');
  const source = store.codingRecord(run.id)!.candidates.at(-1)!;
  const base = store.repositoryWork(run.id)!.base; const targetRef = store.repositoryWork(run.id)!.targetRef;
  const head = git(owned.path, 'rev-parse', 'HEAD'); const tree = git(owned.path, 'rev-parse', 'HEAD^{tree}');
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  evidence.assignReview(resumed.Coordinator, 'review-no-edit', { assignee: 'Reviewer', candidateId: source.id, instruction: 'Review the current answer.' });
  evidence.review(resumed.Reviewer, { candidateId: source.id, verdict: 'changes_requested', summary: 'Reconsider the accepted answer.',
    findings: [{ severity: 'major', body: 'Explain why the current answer is correct.' }], coverage: [] });
  await coding.claim(resumed.Coordinator, 'claim-no-edit', { assignee: 'Writer', instruction: 'Recheck the answer against the accepted brief.' });
  const completion = await coding.requestCompletion(resumed.Writer, 'complete-no-edit', { summary: 'Reviewed the answer; no source change is needed.' });
  store.beginPause(run.id);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const latest = await coding.complete(run.id, completion.id);
  assert.notEqual(latest.id, source.id); assert.deepEqual(latest.revision, source.revision);
  assert.notDeepEqual(latest.implementationAttempts, source.implementationAttempts);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), head); assert.equal(git(owned.path, 'rev-parse', 'HEAD^{tree}'), tree);
  f.advance('answer', 'correct');
  const held = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(held.refresh?.state, 'conflicted'); assert.equal(held.refresh?.resolutionWorkspace, undefined);
  f.rewind(base);
  const candidateCount = store.codingRecord(run.id)!.candidates.length;
  const checkpointCount = store.repositoryWork(run.id)!.checkpoints.length; const noticeCount = store.deliveryNotices(run.id).length;
  const requestId = randomUUID(); const reason = 'Reviewed the target rollback against the latest completed candidate.';
  const decision = { conflictRequestId: held.refresh!.requestId, reason };
  const firstReceipt: { candidateId?: string; sourceCandidateId?: string; supersededBy?: string }[] = [];
  const unsubscribe = store.onChange(() => {
    const saved = store.repositoryWork(run.id)!.refreshes!;
    const receipt = saved.find(item => item.requestId === requestId);
    if (receipt && !firstReceipt.length) firstReceipt.push({ candidateId: receipt.candidateId, sourceCandidateId: receipt.sourceCandidateId,
      supersededBy: saved.find(item => item.requestId === held.refresh!.requestId)?.supersededBy?.requestId });
  });
  const retry = await f.refresh.refresh(run.id, { requestId, operatorDecision: decision }).finally(unsubscribe);
  assert.equal(retry.changed, false); assert.equal(retry.refresh?.requestId, requestId);
  assert.equal(retry.refresh?.candidateId, latest.id); assert.equal(retry.refresh?.sourceCandidateId, latest.id);
  assert.deepEqual(firstReceipt, [{ candidateId: latest.id, sourceCandidateId: latest.id, supersededBy: requestId }],
    'The first committed receipt already binds the source/output candidate and clears exactly its hold.');
  const persisted = store.repositoryWork(run.id)!;
  assert.equal(persisted.refreshes!.find(item => item.requestId === requestId)?.candidateId, latest.id);
  assert.equal(persisted.refreshes!.find(item => item.requestId === held.refresh!.requestId)?.supersededBy?.requestId, requestId);
  assert.equal(persisted.base, base); assert.equal(persisted.targetRef, targetRef);
  assert.equal(persisted.checkpoints.length, checkpointCount); assert.equal(store.deliveryNotices(run.id).length, noticeCount);
  assert.equal(store.codingRecord(run.id)!.candidates.length, candidateCount);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), head); assert.equal(git(owned.path, 'rev-parse', 'HEAD^{tree}'), tree);
  assert.equal(store.getRun(run.id).status, 'paused'); assert.deepEqual(f.counts(), { posts: 0, puts: 0 });
  const later = f.resume();
  evidence.assignReview(later.Coordinator, 'review-later-no-edit', { assignee: 'Reviewer', candidateId: latest.id, instruction: 'Review the retained answer again.' });
  evidence.review(later.Reviewer, { candidateId: latest.id, verdict: 'changes_requested', summary: 'One more assessment is required.',
    findings: [{ severity: 'major', body: 'Check the answer against the latest target.' }], coverage: [] });
  await coding.claim(later.Coordinator, 'claim-later-no-edit', { assignee: 'Writer', instruction: 'Assess the same answer again.' });
  const laterCompletion = await coding.requestCompletion(later.Writer, 'complete-later-no-edit', { summary: 'The accepted answer still requires no source edit.' });
  store.beginPause(run.id);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const successor = await coding.complete(run.id, laterCompletion.id);
  assert.notEqual(successor.id, latest.id); assert.deepEqual(successor.revision, latest.revision);
  const replay = await f.refresh.refresh(run.id, { requestId, operatorDecision: decision });
  assert.equal(replay.changed, false); assert.equal(replay.refresh?.candidateId, latest.id);
  assert.equal(store.codingRecord(run.id)!.candidates.at(-1)!.id, successor.id);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.requestId, requestId);
  const reopened = new Workbench(join(f.root, 'workflow.sqlite'));
  t.after(() => reopened.close());
  const recoveredWork = new RepositoryWork(reopened, join(f.root, 'owned'));
  const historical = await new TargetRefresh(reopened, recoveredWork, new Evidence(reopened, recoveredWork, join(f.root, 'artifacts')))
    .refresh(run.id, { requestId, operatorDecision: decision });
  assert.equal(historical.refresh?.candidateId, latest.id);
  assert.equal(reopened.codingRecord(run.id)!.candidates.at(-1)!.id, successor.id);
});

test('M6.3: a moved target follows the admitted same-head candidate, not an older author', async t => {
  const f = await setup(t, 'crewroom-refresh-same-head-conflict-');
  const { store, run, work, evidence, candidate, git, owned } = f;
  const coding = new Coding(store, work, evidence);
  f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const authoring = f.resume();
  evidence.assignReview(authoring.Coordinator, 'review-same-head-source', { assignee: 'Reviewer', candidateId: candidate.id, instruction: 'Review the retained answer.' });
  evidence.review(authoring.Reviewer, { candidateId: candidate.id, verdict: 'changes_requested', summary: 'Reconsider this answer.',
    findings: [{ severity: 'major', body: 'Confirm the accepted output.' }], coverage: [] });
  await coding.claim(authoring.Coordinator, 'claim-same-head-source', { assignee: 'Writer', instruction: 'Reconsider the answer.' });
  const noEdit = await coding.requestCompletion(authoring.Writer, 'complete-same-head-source', { summary: 'Rechecked the answer without edits.' });
  store.beginPause(run.id);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const source = await coding.complete(run.id, noEdit.id);
  assert.notEqual(source.id, candidate.id); assert.deepEqual(source.revision, candidate.revision);
  const target = f.advance('other', 'target-only\n');
  const refreshed = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(refreshed.changed, true); assert.equal(refreshed.refresh?.sourceCandidateId, source.id);
  const next = store.codingRecord(run.id)!.candidates.at(-1)!;
  assert.equal(refreshed.refresh?.candidateId, next.id);
  assert.equal(next.revision.base, target);
  assert.equal(next.refreshedFrom, source.id, 'The new candidate must not follow the older candidate with the same head.');
  assert.deepEqual(next.implementationAttempts, source.implementationAttempts);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), next.revision.head);
});

test('M6.3: conflict completion follows the recorded same-head source candidate', async t => {
  const f = await setup(t, 'crewroom-refresh-same-head-promotion-');
  const { store, run, work, evidence, candidate, git, owned } = f;
  const coding = new Coding(store, work, evidence);
  f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const authoring = f.resume();
  evidence.assignReview(authoring.Coordinator, 'review-promotion-source', { assignee: 'Reviewer', candidateId: candidate.id, instruction: 'Review the accepted answer.' });
  evidence.review(authoring.Reviewer, { candidateId: candidate.id, verdict: 'changes_requested', summary: 'Recheck this answer.',
    findings: [{ severity: 'major', body: 'Confirm the answer.' }], coverage: [] });
  await coding.claim(authoring.Coordinator, 'claim-promotion-source', { assignee: 'Writer', instruction: 'Recheck the answer.' });
  const noEdit = await coding.requestCompletion(authoring.Writer, 'complete-promotion-source', { summary: 'The answer is unchanged.' });
  store.beginPause(run.id);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const source = await coding.complete(run.id, noEdit.id);
  assert.notEqual(source.id, candidate.id); assert.deepEqual(source.revision, candidate.revision);
  const target = f.advance('answer', 'different\n');
  const conflicted = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(conflicted.refresh?.state, 'conflicted'); assert.equal(conflicted.refresh?.resolutionWorkspace?.state, 'conflicted');
  assert.equal(conflicted.refresh?.sourceCandidateId, source.id);
  const resolving = f.resume();
  await coding.claim(resolving.Coordinator, 'claim-same-head-conflict', { assignee: 'Writer', instruction: 'Reconcile the accepted answer with the target.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resolving.Writer, 'edit-same-head-conflict', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const continued = await coding.continueTargetConflict(resolving.Writer, 'continue-same-head-conflict', { requestId: conflicted.refresh!.requestId });
  assert.equal(continued.state, 'ready');
  const operation = await coding.requestCompletion(resolving.Writer, 'complete-same-head-conflict', { summary: 'Reconciled the accepted answer.' });
  store.beginPause(run.id);
  for (const attempt of store.runtime(run.id).attempts) if (attempt.ownership !== 'stopped') store.recordOwnership(attempt.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const resolved = await coding.complete(run.id, operation.id);
  assert.equal(resolved.revision.base, target);
  assert.equal(resolved.refreshedFrom, source.id);
  assert.ok(resolved.implementationAttempts.includes(resolving.Writer));
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.candidateId, resolved.id);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), resolved.revision.head);
});

test('M6.3: legacy completed association recovery does not consume a distinct reasoned retry', async t => {
  const f = await setup(t, 'crewroom-refresh-legacy-recovery-');
  const { store, run, owned, git } = f;
  f.stop(); f.advance('other', 'target-only\n');
  const completed = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(completed.refresh?.state, 'completed');
  const completedCandidateId = store.codingRecord(run.id)!.candidates.at(-1)!.id;
  const base = store.repositoryWork(run.id)!.base;
  f.advance('answer', 'correct');
  const held = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(held.refresh?.state, 'conflicted');
  f.rewind(base);
  // Legacy mixed history: C has a candidate in the coding record but lost its candidateId association.
  // The current admission API does not create this mixture; seed only the persisted association gap.
  const db = new DatabaseSync(join(f.root, 'workflow.sqlite'));
  try {
    const work = store.repositoryWork(run.id)!;
    work.refreshes!.find(item => item.requestId === completed.refresh!.requestId)!.candidateId = undefined;
    db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(work), run.id);
  } finally { db.close(); }
  const retryId = randomUUID(); const reason = 'Reviewed the held conflict after recovery and verified the target rollback.';
  const candidateCount = store.codingRecord(run.id)!.candidates.length;
  const retry = await f.refresh.refresh(run.id, { requestId: retryId, operatorDecision: { conflictRequestId: held.refresh!.requestId, reason } });
  assert.equal(retry.changed, false); assert.equal(retry.refresh?.requestId, retryId);
  assert.equal(retry.refresh?.toBase, base);
  const history = store.repositoryWork(run.id)!.refreshes!;
  assert.equal(history.find(item => item.requestId === completed.refresh!.requestId)?.candidateId, completedCandidateId);
  assert.equal(history.find(item => item.requestId === held.refresh!.requestId)?.supersededBy?.requestId, retryId);
  assert.equal(store.codingRecord(run.id)!.candidates.length, candidateCount);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), completed.refresh!.toHead);
  assert.deepEqual(f.counts(), { posts: 0, puts: 0 });
});

test('M6.3: an unchanged retry retains the prior operator workspace and stopped writer', async t => {
  const f = await setup(t, 'crewroom-refresh-retained-workspace-');
  const { store, run, owned, git } = f;
  f.advance('other', 'target-only\n'); f.stop();
  const moved = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(moved.refresh?.state, 'completed');
  const base = store.repositoryWork(run.id)!.base;
  f.advance('answer', 'different\n'); f.advance('AGENTS.md', 'changed target instructions\n');
  const held = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(held.refresh?.resolutionWorkspace?.state, 'operator_required');
  const workspace = held.refresh!.resolutionWorkspace!;
  const original = readFileSync(join(workspace.path, 'answer'), 'utf8');
  const head = git(owned.path, 'rev-parse', 'HEAD'); const count = store.codingRecord(run.id)!.candidates.length;
  f.rewind(base);
  const requestId = randomUUID(); const reason = 'Reviewed and rolled back the changed instructions and target conflict.';
  const retry = await f.refresh.refresh(run.id, { requestId, operatorDecision: { conflictRequestId: held.refresh!.requestId, reason } });
  assert.equal(retry.changed, false); assert.equal(retry.refresh?.state, 'completed');
  assert.equal(store.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === held.refresh!.requestId)?.supersededBy?.requestId, requestId);
  assert.equal(readFileSync(join(workspace.path, 'answer'), 'utf8'), original);
  assert.ok(git(owned.repository.path, 'branch', '--list', workspace.branch));
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), head);
  assert.equal(store.codingRecord(run.id)!.candidates.length, count);
  assert.equal(store.repositoryWork(run.id)!.writer, undefined);
  assert.equal(store.getRun(run.id).status, 'paused'); assert.deepEqual(f.counts(), { posts: 0, puts: 0 });
});

test('M6.3: an interrupted reasoned retry retains the prior workspace and becomes the active operator hold', async t => {
  const f = await setup(t, 'crewroom-refresh-retry-crash-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); f.advance('answer', 'different\n'); f.advance('AGENTS.md', 'new target instructions\n');
  const first = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  assert.equal(first.resolutionWorkspace?.state, 'operator_required');
  const workspace = first.resolutionWorkspace!.path;
  const retained = readFileSync(join(workspace, 'answer'), 'utf8');
  const old = store.repositoryWork(run.id)!; const checkpoints = old.checkpoints.length;
  const nextTarget = f.advance('answer', 'another target answer\n');
  const retryId = randomUUID(); const reason = 'I reviewed the retained conflict and requested another attempt.';
  const targetRef = `refs/crewroom/${run.id}/refresh/${retryId}/target`;
  git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${targetRef}`);
  store.beginTargetRefresh(run.id, { requestId: retryId, at: new Date().toISOString(), fromBase: candidate.revision.base,
    fromHead: candidate.revision.head, targetRef, toBase: nextTarget, sourceCandidateId: candidate.id, state: 'rebasing',
    supersedes: { requestId: first.requestId, reason } });
  assert.throws(() => git(owned.path, 'rebase', '--onto', nextTarget, candidate.revision.base));
  store.recordTargetRebaseRecovery(run.id, retryId, 'rebase_stopped'); // The real Git command returned before the controller stopped.
  store.close();
  const reopened = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => reopened.close());
  const work = new RepositoryWork(reopened, join(f.root, 'owned'));
  const evidence = new Evidence(reopened, work, join(f.root, 'artifacts'));
  await new TargetRefresh(reopened, work, evidence).reconcile();
  const history = reopened.repositoryWork(run.id)!.refreshes!;
  const firstSaved = history.find(item => item.requestId === first.requestId)!;
  const retry = history.find(item => item.requestId === retryId)!;
  assert.deepEqual(firstSaved.supersededBy, { requestId: retryId, reason, at: retry.at });
  assert.deepEqual(retry.supersedes, { requestId: first.requestId, reason });
  assert.equal(retry.state, 'conflicted'); assert.equal(retry.holdReason, 'interrupted_retry');
  assert.deepEqual(retry.conflicts, []); assert.equal(retry.resolutionWorkspace, undefined); assert.equal(retry.toHead, undefined);
  assert.equal(work.unresolvedConflict(run.id)?.requestId, retryId);
  assert.equal(readFileSync(join(workspace, 'answer'), 'utf8'), retained);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(reopened.repositoryWork(run.id)!.base, old.base);
  assert.equal(reopened.repositoryWork(run.id)!.targetRef, old.targetRef);
  assert.equal(reopened.repositoryWork(run.id)!.checkpoints.length, checkpoints);
  assert.equal(reopened.codingRecord(run.id)!.candidates.length, 1);
  const view = await new Results(reopened, work, evidence).read(run.id);
  assert.equal(view.refreshable, true); assert.equal(view.assessment.ready, false);
  assert.match(view.assessment.blockers.join(' '), /target conflict/i);
  await assert.rejects(work.claimWriter(reopened.runtime(run.id).attempts[0].id), /operator decision/i);
  const decision = { conflictRequestId: retryId, reason: 'Retry against the current target after the interruption.' };
  const publication = new Publication(reopened, work, evidence, join(f.root, 'publication-restart'), [f.connection]);
  t.after(() => publication.close());
  const service = new TargetRefresh(reopened, work, evidence, publication);
  const replay = await service.refresh(run.id, { requestId: retryId, operatorDecision: { conflictRequestId: first.requestId, reason } });
  assert.equal(replay.changed, false); assert.deepEqual(replay.refresh, retry);
  const replacement = await service.refresh(run.id, { requestId: randomUUID(), operatorDecision: decision });
  assert.equal(replacement.refresh?.supersedes?.requestId, retryId);
  assert.equal(readFileSync(join(workspace, 'answer'), 'utf8'), retained);
  assert.deepEqual(f.counts(), { posts: 0, puts: 0 });
});

test('M6.3: a reopened retry without a recorded Git exit keeps its rebasing hold for inspection', async t => {
  const f = await setup(t, 'crewroom-refresh-unproved-child-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); f.advance('answer', 'correct');
  const first = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  assert.equal(first.state, 'conflicted');
  const target = f.advance('answer', 'different\n'); const retryId = randomUUID();
  const targetRef = `refs/crewroom/${run.id}/refresh/${retryId}/target`;
  git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${targetRef}`);
  store.beginTargetRefresh(run.id, { requestId: retryId, at: new Date().toISOString(), fromBase: candidate.revision.base,
    fromHead: candidate.revision.head, targetRef, toBase: target, sourceCandidateId: candidate.id, state: 'rebasing',
    supersedes: { requestId: first.requestId, reason: 'Reviewed the previous hold.' } });
  assert.throws(() => git(owned.path, 'rebase', '--onto', target, candidate.revision.base));
  store.close();
  const reopened = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => reopened.close());
  const work = new RepositoryWork(reopened, join(f.root, 'owned'));
  const recovery = new TargetRefresh(reopened, work, new Evidence(reopened, work, join(f.root, 'artifacts')));
  await recovery.reconcile();
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'rebasing');
  assert.equal(reopened.getRun(run.id).status, 'attention_required');
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === first.requestId)?.supersededBy?.requestId, retryId);
  assert.equal(reopened.repositoryWork(run.id)!.base, candidate.revision.base);
  assert.equal(git(owned.path, 'rev-parse', `refs/heads/${owned.branch}`), candidate.revision.head);
  assert.equal(reopened.codingRecord(run.id)!.candidates.length, 1);
  reopened.recordTargetRebaseRecovery(run.id, retryId, 'rebase_stopped');
  reopened.recordTargetRebaseRecovery(run.id, retryId, 'undo_started'); // Crash after starting abort/reset has no stopped receipt.
  await recovery.reconcile();
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'rebasing');
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.rebaseRecovery, 'undo_started');
  assert.equal(reopened.repositoryWork(run.id)!.base, candidate.revision.base);
});

test('M6.3: version-21 interrupted superseders normalize before Results exposure without losing explicit successors', async t => {
  for (const successor of [false, true]) {
    const f = await setup(t, `crewroom-refresh-v21-${successor}-`);
    const { store, run, owned, git, candidate } = f;
    f.stop(); f.advance('answer', 'different\n'); f.advance('AGENTS.md', 'changed target instructions\n');
    const first = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    const workspace = first.resolutionWorkspace!.path; const content = readFileSync(join(workspace, 'answer'), 'utf8');
    const before = store.repositoryWork(run.id)!; const originalRun = store.getRun(run.id);
    const ownership = store.runtime(run.id).attempts.map(item => item.ownership);
    const candidateCount = store.codingRecord(run.id)!.candidates.length;
    store.close();
    const legacy = join(f.root, `legacy-${successor}.sqlite`);
    copyFileSync(join(f.root, 'workflow.sqlite'), legacy);
    const db = new DatabaseSync(legacy);
    const retryId = randomUUID(), reason = 'Reviewed the first workspace and attempted another target.';
    const retry = { requestId: retryId, at: new Date().toISOString(), fromBase: candidate.revision.base,
      fromHead: candidate.revision.head, targetRef: first.targetRef, toBase: first.toBase, sourceCandidateId: candidate.id,
      state: 'interrupted' as const, supersedes: { requestId: first.requestId, reason } };
    const later = successor ? { ...retry, requestId: randomUUID(), at: new Date(Date.now() + 1000).toISOString(), state: 'completed' as const,
      supersedes: { requestId: retryId, reason: 'Reviewed the interrupted second attempt.' }, toBase: before.base,
      toHead: candidate.revision.head, candidateId: candidate.id } : undefined;
    const ordinary = { ...retry, requestId: randomUUID(), supersedes: undefined, at: new Date(Date.now() + 2000).toISOString() };
    const unrelated = { ...ordinary, requestId: randomUUID(), state: 'completed' as const,
      at: new Date(Date.now() + 3000).toISOString(), toHead: candidate.revision.head, candidateId: candidate.id };
    const history = [{ ...first, supersededBy: { requestId: retryId, reason, at: retry.at } },
      { ...retry, ...(later ? { supersededBy: { requestId: later.requestId, reason: later.supersedes.reason, at: later.at } } : {}) },
      ...(later ? [later] : []), ordinary, unrelated];
    db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify({ ...before, refreshes: history }), run.id);
    db.exec('PRAGMA user_version=21;'); db.close();
    if (!successor) {
      const corrupt = join(f.root, 'legacy-broken-link.sqlite'); copyFileSync(legacy, corrupt);
      const broken = new DatabaseSync(corrupt);
      const corruptHistory = [{ ...history[0], supersededBy: { requestId: retryId, reason: 'Contradictory operator decision', at: retry.at } }, ...history.slice(1)];
      broken.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify({ ...before, refreshes: corruptHistory }), run.id);
      broken.close();
      assert.throws(() => new Workbench(corrupt), /history is ambiguous/);
      const unchanged = new DatabaseSync(corrupt);
      assert.equal(Number(unchanged.prepare('PRAGMA user_version').get()!.user_version), 21);
      unchanged.close();
      const running = join(f.root, 'legacy-running.sqlite'); copyFileSync(legacy, running);
      const runningDb = new DatabaseSync(running);
      runningDb.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify({ ...originalRun, status: 'running', desiredState: 'running' }), run.id);
      runningDb.close();
      const paused = new Workbench(running); t.after(() => paused.close());
      assert.equal(paused.getRun(run.id).status, 'attention_required');
      assert.equal(paused.getRun(run.id).desiredState, 'paused');
      assert.equal(paused.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === retryId)?.holdReason, 'interrupted_retry');
      assert.deepEqual(paused.runtime(run.id).attempts.map(item => item.ownership), ownership);
      const cancelled = join(f.root, 'legacy-cancelled.sqlite'); copyFileSync(legacy, cancelled);
      const cancelledDb = new DatabaseSync(cancelled);
      cancelledDb.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify({ ...originalRun, status: 'cancelled', desiredState: 'cancelled' }), run.id);
      cancelledDb.close();
      const preserved = new Workbench(cancelled); t.after(() => preserved.close());
      assert.equal(preserved.getRun(run.id).status, 'cancelled');
      assert.equal(preserved.getRun(run.id).desiredState, 'cancelled');
      assert.equal(preserved.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === retryId)?.holdReason, 'interrupted_retry');
    }
    const reopened = new Workbench(legacy); t.after(() => reopened.close());
    const work = new RepositoryWork(reopened, join(f.root, 'owned'));
    const evidence = new Evidence(reopened, work, join(f.root, 'artifacts'));
    const normalized = reopened.repositoryWork(run.id)!.refreshes!;
    const saved = normalized.find(item => item.requestId === retryId)!;
    assert.equal(saved.state, 'conflicted'); assert.equal(saved.holdReason, 'interrupted_retry');
    assert.deepEqual(saved.conflicts, []); assert.equal(saved.resolutionWorkspace, undefined);
    assert.deepEqual(saved.supersededBy, later ? { requestId: later.requestId, reason: later.supersedes.reason, at: later.at } : undefined);
    assert.equal(normalized.find(item => item.requestId === first.requestId)?.resolutionWorkspace?.state, 'continuing');
    assert.equal(normalized.find(item => item.requestId === first.requestId)?.resolutionWorkspace?.legacyOwnershipUnknown, true);
    assert.equal(work.activeConflict(run.id), undefined, 'A superseded unknown workspace is never reactivated as a writer scope.');
    const view = await new Results(reopened, work, evidence).read(run.id);
    assert.equal(view.assessment.blockers.some(reason => /target conflict/i.test(reason)), true);
    assert.equal(view.refreshable, false);
    assert.equal(normalized.find(item => item.requestId === ordinary.requestId)?.state, 'interrupted');
    assert.equal(normalized.find(item => item.requestId === unrelated.requestId)?.state, 'completed');
    assert.equal(reopened.repositoryWork(run.id)!.base, before.base);
    assert.equal(reopened.codingRecord(run.id)!.candidates.length, candidateCount);
    if (!successor) {
      const attempts = reopened.runtime(run.id).assignments.map(item => item.currentAttemptId);
      const runtime = new RuntimeSupervisor(reopened, { root: join(f.root, 'migration-hosts'), socket: join(f.root, 'migration-tmux.sock'), ompBinary: '/unused',
        mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
        catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9', () => [], work);
      const register = Messaging.prototype.register;
      Messaging.prototype.register = async () => {};
      try { await assert.rejects(Promise.resolve().then(() => runtime.resume(run.id)), /Git mutation ownership is unverified/); }
      finally { Messaging.prototype.register = register; await runtime.close(); }
      for (const id of attempts) assert.ok(reopened.runtime(run.id).attempts.some(item => item.id === id), 'Prior attempt history is retained.');
      assert.equal(reopened.runtime(run.id).attempts.some(item => item.ownership === 'held'), false);
      assert.equal(reopened.repositoryWork(run.id)!.writer, undefined);
    }
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal(readFileSync(join(workspace, 'answer'), 'utf8'), content);
    const serialized = JSON.stringify(reopened.repositoryWork(run.id));
    reopened.close();
    const again = new Workbench(legacy); t.after(() => again.close());
    assert.equal(JSON.stringify(again.repositoryWork(run.id)), serialized);
    const schema = new DatabaseSync(legacy);
    assert.equal(Number(schema.prepare('PRAGMA user_version').get()!.user_version), 26); schema.close();
  }
});

test('M6.3: version-22 uncertain and unproved operator-required workspaces hold before runtime exposure', async t => {
  for (const state of ['conflicted', 'preparing', 'operator_required'] as const) {
    const f = await setup(t, `crewroom-v23-${state}-`);
    const { store, run, owned, git, candidate } = f;
    f.stop(); f.advance('answer', 'different\n');
    if (state === 'operator_required') f.advance('AGENTS.md', 'new target instructions\n');
    const saved = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    const workspace = saved.resolutionWorkspace!;
    assert.ok(workspace); const content = readFileSync(join(workspace.path, 'answer'), 'utf8');
    const workBefore = store.repositoryWork(run.id)!;
    store.close();
    const dbPath = join(f.root, `legacy-workspace-${state}.sqlite`); copyFileSync(join(f.root, 'workflow.sqlite'), dbPath);
    const old = new DatabaseSync(dbPath);
    if (state === 'preparing') {
      workBefore.refreshes!.at(-1)!.resolutionWorkspace!.state = 'preparing';
      old.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(workBefore), run.id);
    }
    old.exec('PRAGMA user_version=22;'); old.close();
    const recovered = new Workbench(dbPath); t.after(() => recovered.close());
    const recoveredWork = new RepositoryWork(recovered, join(f.root, 'owned'));
    const evidence = new Evidence(recovered, recoveredWork, join(f.root, 'artifacts'));
    const expected = state === 'preparing' ? state : 'continuing';
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, expected);
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.requestId, saved.requestId);
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal((await new Results(recovered, recoveredWork, evidence).read(run.id)).refreshable, false);
    assert.equal(readFileSync(join(workspace.path, 'answer'), 'utf8'), content);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
    {
      await new TargetRefresh(recovered, recoveredWork, evidence).reconcile();
      assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, expected);
      await assert.rejects(recoveredWork.readFile(run.id, 'answer'), /unverified|inspection|conflict/i);
      await assert.rejects(new TargetRefresh(recovered, recoveredWork, evidence).refresh(run.id, { requestId: randomUUID() }), /inspect/i);
    }
    const serialized = JSON.stringify(recovered.repositoryWork(run.id)); recovered.close();
    const again = new Workbench(dbPath); t.after(() => again.close());
    assert.equal(JSON.stringify(again.repositoryWork(run.id)), serialized);
    const schema = new DatabaseSync(dbPath); assert.equal(Number(schema.prepare('PRAGMA user_version').get()!.user_version), 26); schema.close();
  }
});

test('M6.3: a superseded legacy unknown workspace still blocks the newer completed target receipt', async t => {
  const f = await setup(t, 'crewroom-v23-superseded-unknown-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); f.advance('answer', 'different\n'); f.advance('AGENTS.md', 'changed target instructions\n');
  const first = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  assert.equal(first.resolutionWorkspace?.state, 'operator_required');
  const workspace = first.resolutionWorkspace!.path;
  const content = readFileSync(join(workspace, 'answer'), 'utf8');
  f.rewind(candidate.revision.base);
  const reason = 'The target returned to the validated base after operator review.';
  const retry = (await f.refresh.refresh(run.id, { requestId: randomUUID(), operatorDecision: { conflictRequestId: first.requestId, reason } })).refresh!;
  assert.equal(retry.state, 'completed'); assert.equal(store.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === first.requestId)?.supersededBy?.requestId, retry.requestId);
  store.close();
  const dbPath = join(f.root, 'legacy-superseded-unknown.sqlite'); copyFileSync(join(f.root, 'workflow.sqlite'), dbPath);
  const db = new DatabaseSync(dbPath); db.exec('PRAGMA user_version=22;'); db.close();
  const migrated = new Workbench(dbPath); t.after(() => migrated.close());
  const work = new RepositoryWork(migrated, join(f.root, 'owned'));
  const evidence = new Evidence(migrated, work, join(f.root, 'artifacts'));
  const history = migrated.repositoryWork(run.id)!.refreshes!;
  assert.equal(history.find(item => item.requestId === first.requestId)?.resolutionWorkspace?.state, 'continuing');
  assert.equal(history.find(item => item.requestId === first.requestId)?.resolutionWorkspace?.legacyOwnershipUnknown, true);
  assert.equal(history.find(item => item.requestId === first.requestId)?.supersededBy?.requestId, retry.requestId);
  assert.equal(history.find(item => item.requestId === retry.requestId)?.state, 'completed');
  assert.equal(migrated.getRun(run.id).status, 'attention_required');
  const results = await new Results(migrated, work, evidence).read(run.id);
  assert.equal(results.refreshable, false); assert.equal(results.assessment.ready, false);
  assert.match(results.assessment.blockers.join(' '), /target conflict/i);
  await assert.rejects(work.readFile(run.id, 'answer'), /unverified|inspection/i);
  await assert.rejects(work.reconcileRefresh(run.id), /unverified/i);
  assert.throws(() => migrated.claimRepositoryWriter(migrated.runtime(run.id).attempts[0].id,
    migrated.repositoryWork(run.id)!.checkpoints.at(-1)!), /Unverified Git mutation ownership/);
  const runtime = new RuntimeSupervisor(migrated, { root: join(f.root, 'superseded-hosts'), socket: `/tmp/crewroom-${run.id.slice(0, 8)}.sock`, ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9', () => [], work);
  const register = Messaging.prototype.register; Messaging.prototype.register = async () => {};
  try { await assert.rejects(Promise.resolve().then(() => runtime.resume(run.id)), /Git mutation ownership is unverified/); }
  finally { Messaging.prototype.register = register; await runtime.close(); }
  await assert.rejects(new TargetRefresh(migrated, work, evidence).refresh(run.id, { requestId: retry.requestId, operatorDecision: { conflictRequestId: first.requestId, reason } }), /unverified/i);
  assert.equal(readFileSync(join(workspace, 'answer'), 'utf8'), content);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(migrated.codingRecord(run.id)!.candidates.length, 1);
});

test('M6.3: project instruction conflicts remain operator-held and never grant an agent writer', async t => {
  const f = await setup(t, 'crewroom-refresh-instruction-conflict-');
  const { store, run, work, owned, git, candidate } = f;
  f.stop(); f.advance('answer', 'different\n'); f.advance('AGENTS.md', 'target instructions\n');
  const held = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(held.refresh?.state, 'conflicted');
  assert.equal(held.refresh?.resolutionWorkspace?.state, 'operator_required');
  assert.deepEqual(held.refresh?.conflicts, ['answer']);
  assert.equal(work.activeConflict(run.id), undefined);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 1);
  await assert.rejects(f.refresh.refresh(run.id, { requestId: randomUUID() }), /operator reason/i);
  const coding = new Coding(store, work, f.evidence); const resumed = f.resume();
  await assert.rejects(coding.claim(resumed.Coordinator, 'unsafe-instruction-claim', { assignee: 'Writer', instruction: 'Resolve project instructions.' }), /operator decision/i);
  assert.equal(store.repositoryWork(run.id)!.writer, undefined);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  f.stop(); await work.reconcileStopped(run.id);
  const previousWorkspace = held.refresh!.resolutionWorkspace!.path;
  const repairedTarget = join(f.root, 'operator-target');
  f.git(f.root, 'clone', '-q', f.remote, repairedTarget);
  f.git(repairedTarget, 'config', 'user.name', 'Operator'); f.git(repairedTarget, 'config', 'user.email', 'operator@example.invalid');
  f.git(repairedTarget, 'rm', '-q', 'AGENTS.md'); f.git(repairedTarget, 'commit', '-qm', 'Restore accepted project instructions');
  f.git(repairedTarget, 'push', '-q', 'origin', 'main');
  const retryId = randomUUID();
  const retry = await f.refresh.refresh(run.id, { requestId: retryId, operatorDecision: { conflictRequestId: held.refresh!.requestId, reason: 'Reviewed and reverted the target-only instruction change.' } });
  assert.equal(retry.refresh?.state, 'conflicted'); assert.equal(retry.refresh?.resolutionWorkspace?.state, 'conflicted');
  const history = store.repositoryWork(run.id)!.refreshes!;
  assert.deepEqual(history.find(item => item.requestId === held.refresh!.requestId)?.supersededBy, { requestId: retryId, reason: 'Reviewed and reverted the target-only instruction change.', at: history.at(-1)!.at });
  assert.equal(existsSync(previousWorkspace), true, 'An operator retry retains the previous isolated workspace for inspection.');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
});

test('M6.3: a legitimate separator can resolve a conflict but an unresolved marker block cannot', async t => {
  const f = await setup(t, 'crewroom-refresh-marker-');
  const { store, run, work, owned, git, candidate, evidence } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision }); f.advance('answer', 'different\n');
  const held = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'marker-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  assert.match(file.content, /^<<<<<<< .+\n[\s\S]*^=======$/m);
  await assert.rejects(coding.continueTargetConflict(resumed.Writer, 'marker-unresolved', { requestId: held.requestId }), /Resolve every conflict marker/);
  assert.equal(work.activeConflict(run.id)?.requestId, held.requestId);
  const content = 'Heading\n=======\ncorrect and different\n';
  await coding.edit(resumed.Writer, 'marker-resolution', { path: 'answer', expectedHash: file.sha256, content });
  const ready = await coding.continueTargetConflict(resumed.Writer, 'marker-resolved', { requestId: held.requestId });
  assert.equal(ready.state, 'ready');
  assert.equal(readFileSync(join(held.resolutionWorkspace!.path, 'answer'), 'utf8'), content);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
});

test('M6.3: adopting new requirements converts a retained conflict to an operator hold', async t => {
  const f = await setup(t, 'crewroom-refresh-requirements-');
  const { store, run, work, owned, candidate, git } = f;
  f.advance('answer', 'different\n'); f.stop();
  const refresh = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(refresh.refresh?.resolutionWorkspace?.state, 'conflicted');
  const task = store.getTask(run.taskId);
  const revised = store.refine(task.id, { revision: task.revision, sourceRevision: 1, brief: 'Resolve the answer on the target revision', criteria: ['Correct output', 'Preserve target behavior'] });
  store.adoptRequirements(run.id, { revision: revised.revision, requestId: randomUUID(), reason: 'The accepted output changed.' });
  await assert.rejects(work.validateConflictWorkspace(run.id), /requirements changed/i);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  const coding = new Coding(store, work, f.evidence); const resumed = f.resume();
  await assert.rejects(coding.claim(resumed.Coordinator, 'old-brief-claim', { assignee: 'Writer', instruction: 'Use the prior brief.' }), /operator decision/i);
  assert.equal(store.repositoryWork(run.id)!.writer, undefined);
});

test('M6.3: only a resumed run can grant one specialist scoped conflict edits', async t => {
  const f = await setup(t, 'crewroom-refresh-writer-');
  const { store, run, work, owned, git, candidate } = f;
  f.advance('answer', 'different\n'); f.advance('other', 'target-only\n'); f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const receipt = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(receipt.refresh?.state, 'conflicted');
  const activeConflict = receipt.refresh; assert.ok(activeConflict);
  const coding = new Coding(store, work, f.evidence);
  const stopped = store.runtime(run.id).assignments;
  const coordinator = stopped.find(item => item.coordinator)!;
  await assert.rejects(coding.claim(coordinator.currentAttemptId, 'paused-claim', { assignee: 'Writer', instruction: 'Resolve answer within accepted requirements.' }), /active coordinator/);
  const resumed = f.resume();
  const claim = await coding.claim(resumed.Coordinator, 'resolve-claim', { assignee: 'Writer', instruction: 'Resolve answer within accepted requirements.' });
  assert.equal(claim.writerAttemptId, resumed.Writer);
  const file = await work.readConflict(run.id, 'answer');
  assert.match(file.content, /<<<<<<<|>>>>>>>/);
  assert.equal((await work.readFile(run.id, 'other')).content, 'target-only\n');
  await assert.rejects(coding.edit(resumed.Reviewer, 'other-writer', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' }), /current writer/);
  await assert.rejects(coding.edit(resumed.Writer, 'outside-conflict', { path: 'elsewhere', expectedHash: null, content: 'wrong' }), /conflict|scope/);
  const changed = await coding.edit(resumed.Writer, 'resolution', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  assert.equal(changed.path, 'answer');
  await assert.rejects(coding.continueTargetConflict(resumed.Reviewer, 'other-continue', { requestId: activeConflict.requestId }), /current writer/);
  assert.equal((await work.readConflict(run.id, 'answer')).content, 'correct and different\n');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(git(owned.path, 'status', '--porcelain'), '');
});

test('M6.3: a live conflict Git add leaves a keyed continuing barrier that blocks writers and replay', async t => {
  const f = await setup(t, 'crewroom-continue-live-child-');
  const { store, run, work, evidence, owned, git, candidate } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  f.advance('answer', 'different\n');
  const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'continuing-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'continuing-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const latch = gitLatch(f.root, 'add'); t.after(() => latch.restore()); latch.arm();
  const pending = coding.continueTargetConflict(resumed.Writer, 'continue-key-1', { requestId: refresh.requestId });
  await latch.blocked();
  try {
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const recoveredWork = new RepositoryWork(recovered, join(f.root, 'owned'));
    const workspace = recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace!;
    assert.equal(workspace.state, 'continuing');
    assert.equal(workspace.continuation?.toolCallId, 'continue-key-1');
    assert.deepEqual(workspace.continuation?.paths, ['answer']);
    assert.equal(recoveredWork.activeConflict(run.id), undefined);
    await assert.rejects(coding.edit(resumed.Writer, 'continuing-late-edit', { path: 'answer', expectedHash: null, content: 'unsafe\n' }), /operator decision|unverified|continuing/i);
    await assert.rejects(coding.continueTargetConflict(resumed.Writer, 'continue-key-1', { requestId: refresh.requestId }), /continuing|unverified/i);
    await new TargetRefresh(recovered, recoveredWork, new Evidence(recovered, recoveredWork, join(f.root, 'artifacts'))).reconcile();
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'continuing');
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  } finally { latch.release(); await pending.catch(() => {}); }
});

test('M6.3: ownership stopped during isolated file validation prevents the late write', async t => {
  const f = await setup(t, 'crewroom-refresh-late-write-');
  const { store, run, work, owned, git, candidate } = f;
  f.advance('answer', 'different\n'); f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const coding = new Coding(store, work, f.evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'late-write-claim', { assignee: 'Writer', instruction: 'Resolve answer within scope.' });
  const file = await work.readConflict(run.id, 'answer');
  const latch = gitLatch(f.root, 'worktree', 2); t.after(() => latch.restore()); latch.arm();
  const write = work.writeFile(resumed.Writer, { path: 'answer', expectedHash: file.sha256, content: 'late write\n' });
  await latch.blocked(); store.recordOwnership(resumed.Writer, true); latch.release();
  await assert.rejects(write, /current writer/);
  assert.equal((await work.readConflict(run.id, 'answer')).content, file.content);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
});

test('M6.3: a detached isolated checkout is operator-held rather than promoted', async t => {
  const f = await setup(t, 'crewroom-refresh-detached-');
  const { store, run, work, owned, git, candidate, evidence } = f;
  f.advance('answer', 'different\n'); f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const result = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const refresh = result.refresh; assert.ok(refresh);
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'detached-claim', { assignee: 'Writer', instruction: 'Resolve answer within the accepted scope.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'detached-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const ready = await coding.continueTargetConflict(resumed.Writer, 'detached-continue', { requestId: refresh.requestId });
  assert.equal(ready.state, 'ready');
  git(refresh.resolutionWorkspace!.path, 'checkout', '--detach');
  await assert.rejects(work.validateConflictWorkspace(run.id), /conflict workspace or target changed/i);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
  await assert.rejects(coding.requestCompletion(resumed.Writer, 'detached-complete', { summary: 'Do not promote detached source.' }), /operator decision/i);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  f.stop(); await work.reconcileStopped(run.id);
});

test('M6.3: an interrupted isolated edit survives restart and restores one writer only after Resume', async t => {
  const f = await setup(t, 'crewroom-refresh-writer-recovery-');
  const { root, store, run, work, evidence, owned, git, candidate } = f;
  const coding = new Coding(store, work, evidence);
  const priorWriter = store.runtime(run.id).assignments.find(item => item.name === 'Writer')!;
  const coordinator = store.runtime(run.id).assignments.find(item => item.coordinator)!;
  store.updateCodingRecord(run.id, record => ({ ...record!, claims: [...(record!.claims ?? []), { id: randomUUID(), coordinatorAttemptId: coordinator.currentAttemptId,
    toolCallId: 'before-conflict', writerAttemptId: priorWriter.currentAttemptId, instruction: 'Continue the accepted source implementation.', candidateId: candidate.id, automatic: false }] }));
  f.advance('answer', 'different\n'); f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const refresh = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const conflict = refresh.refresh; assert.ok(conflict);
  const first = f.resume(); await coding.restoreWriter(run.id);
  assert.equal(store.repositoryWork(run.id)!.writer, undefined, 'A pre-conflict claim is not restored into the isolated workspace.');
  await coding.claim(first.Coordinator, 'interrupted-conflict-claim', { assignee: 'Writer', instruction: 'Resolve the recorded answer conflict.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(first.Writer, 'interrupted-conflict-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id); store.close();

  const recovered = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => recovered.close());
  const recoveredWork = new RepositoryWork(recovered, join(root, 'owned'));
  const recoveredEvidence = new Evidence(recovered, recoveredWork, join(root, 'artifacts'));
  await new TargetRefresh(recovered, recoveredWork, recoveredEvidence).reconcile();
  const requested = recovered.setDesiredState(run.id, 'running');
  const next = recovered.runtime(run.id).assignments.map(assignment => recovered.replaceAttempt(assignment.id, { automatic: false }));
  recovered.prepareContinuation(run.id, requested.controlRevision!);
  for (const [index, attempt] of next.entries()) {
    recovered.claimAttempt(attempt.id);
    recovered.recordAttemptIdentity(attempt.id, { hostPid: index + 40, ompPid: index + 50, sessionId: `recovered-${index}`, hostUrl: `http://127.0.0.1:${9200 + index}`, pane: `%${index + 4}` });
  }
  recovered.startExecution(run.id);
  await new Coding(recovered, recoveredWork, recoveredEvidence).restoreWriter(run.id);
  const writer = recovered.runtime(run.id).assignments.find(item => item.name === 'Writer')!;
  assert.equal(recovered.repositoryWork(run.id)!.writer?.attemptId, writer.currentAttemptId);
  assert.equal((await recoveredWork.readConflict(run.id, 'answer')).content, 'correct and different\n');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
});

test('M6.3: explicit Resume recovers a completed isolated rebase whose ready receipt was lost', async t => {
  const f = await setup(t, 'crewroom-refresh-ready-recovery-');
  const { root, store, run, work, evidence, owned, git, candidate } = f;
  f.advance('answer', 'different\n'); f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const result = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const refresh = result.refresh; assert.ok(refresh);
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'ready-recovery-claim', { assignee: 'Writer', instruction: 'Resolve the recorded answer conflict.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'ready-recovery-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const workspace = refresh.resolutionWorkspace!.path;
  git(workspace, 'add', '--', 'answer');
  git(workspace, '-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', '-c', 'core.editor=true', 'rebase', '--continue');
  const head = git(workspace, 'rev-parse', 'HEAD');
  store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id); store.close();

  const recovered = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => recovered.close());
  const recoveredWork = new RepositoryWork(recovered, join(root, 'owned'));
  const recoveredEvidence = new Evidence(recovered, recoveredWork, join(root, 'artifacts'));
  await new TargetRefresh(recovered, recoveredWork, recoveredEvidence).reconcile();
  const request = recovered.setDesiredState(run.id, 'running');
  for (const assignment of recovered.runtime(run.id).assignments) recovered.replaceAttempt(assignment.id, { automatic: false });
  recovered.prepareContinuation(run.id, request.controlRevision!);
  const validation = await recoveredWork.validateConflictWorkspace(run.id);
  assert.equal(validation.state, 'ready'); assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.toHead, head);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1, 'Recovery does not promote or invent a candidate before writer completion.');
});

test('M6.3: one resolver can continue successive in-scope rebase conflicts before submitting completion', async t => {
  const f = await setup(t, 'crewroom-refresh-multi-conflict-');
  const { store, run, work, evidence, owned, git } = f;
  const originalWriter = store.runtime(run.id).assignments.find(item => item.name === 'Writer')!;
  f.stop();
  writeFileSync(join(owned.path, 'answer'), 'source one\n'); git(owned.path, 'commit', '-am', 'First source refinement');
  writeFileSync(join(owned.path, 'answer'), 'source two\n'); git(owned.path, 'commit', '-am', 'Second source refinement');
  const sourceRevision = (await work.status(run.id)).revision;
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: sourceRevision });
  const sourceCandidate = await evidence.registerCandidate(run.id, [originalWriter.currentAttemptId]);
  const target = f.advance('answer', 'different\n');
  const refreshResult = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const refresh = refreshResult.refresh; assert.ok(refresh); assert.equal(refresh.state, 'conflicted');
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'multi-conflict-claim', { assignee: 'Writer', instruction: 'Resolve each in-scope answer conflict while retaining target behavior.' });
  const resolutions = ['correct and different\n', 'source one and different\n', 'source two and different\n'];
  for (const [index, content] of resolutions.entries()) {
    const file = await work.readConflict(run.id, 'answer');
    await coding.edit(resumed.Writer, `resolve-step-${index}`, { path: 'answer', expectedHash: file.sha256, content });
    const result = await coding.continueTargetConflict(resumed.Writer, `continue-step-${index}`, { requestId: refresh.requestId });
    assert.equal(result.state, index === resolutions.length - 1 ? 'ready' : 'conflicted');
    if (index < resolutions.length - 1) assert.deepEqual(result.conflicts, ['answer']);
    assert.equal((await work.validateConflictWorkspace(run.id)).state, result.state);
    assert.deepEqual(await coding.continueTargetConflict(resumed.Writer, `continue-step-${index}`, { requestId: refresh.requestId }), result);
    if (index > 0) await assert.rejects(coding.continueTargetConflict(resumed.Writer, 'continue-step-0', { requestId: refresh.requestId }), /historical/);
  }
  const completion = await coding.requestCompletion(resumed.Writer, 'finish-multi-conflict', { summary: 'Resolve all target conflicts' });
  store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const next = await coding.complete(run.id, completion.id);
  assert.equal(next.revision.base, target); assert.equal(git(owned.path, 'show', 'HEAD:answer'), 'source two and different');
  assert.equal(next.refreshedFrom, sourceCandidate.id);
  assert.ok(next.implementationAttempts.includes(originalWriter.currentAttemptId));
  assert.ok(next.implementationAttempts.includes(resumed.Writer));
});


test('M6.3: a stopped resolution promotes one in-scope candidate; the same PR waits for fresh checks and review', async t => {
  const f = await setup(t, 'crewroom-refresh-resolution-');
  const { store, run, evidence, work, candidate, owned, git } = f;
  await f.publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary });
  await f.publication.flush();
  const oldRemoteHead = f.remoteHead(`refs/heads/${owned.branch}`);
  const target = f.advance('answer', 'different\n'); f.stop();
  store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  const refresh = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const conflict = refresh.refresh; assert.ok(conflict); assert.equal(conflict.state, 'conflicted');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), oldRemoteHead);
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), oldRemoteHead);
  assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'conflict-claim', { assignee: 'Writer', instruction: 'Reconcile answer with accepted output.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'resolve-answer', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const continued = await coding.continueTargetConflict(resumed.Writer, 'continue-conflict', { requestId: conflict.requestId });
  assert.equal(continued.state, 'ready');
  const operation = await coding.requestCompletion(resumed.Writer, 'resolve-complete', { summary: 'Resolve target conflict' });
  store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const next = await coding.complete(run.id, operation.id);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), next.revision.head);
  assert.equal(next.revision.base, target);
  assert.equal(git(owned.path, 'show', 'HEAD:answer'), 'correct and different');
  assert.equal(next.refreshedFrom, candidate.id);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 2);
  assert.equal(existsSync(conflict.resolutionWorkspace!.path), false, 'The promoted isolated worktree is removed.');
  assert.equal(git(owned.path, 'branch', '--list', conflict.resolutionWorkspace!.branch), '');
  assert.equal((await coding.complete(run.id, operation.id)).id, next.id, 'Replayed completion cannot promote twice.');
  assert.equal((await evidence.assess(run.id)).ready, false);
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), oldRemoteHead, 'Publication does not push during promotion.');
  const attempts = f.resume();
  await assert.rejects(f.publication.prepare(attempts.Coordinator, { candidateId: next.id, summary: reviewerSummary }), /locally validated/);
  const check = store.codingRecord(run.id)!.requiredChecks[0];
  evidence.recordCheck(run.id, { candidateId: next.id, attemptId: attempts.Reviewer, checkId: 'unit', definition: check, environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 0, timedOut: false,
    output: evidence.artifact('Controlled receipt for the conflict resolution candidate.'), source: evidence.artifact(JSON.stringify(await work.listFiles(run.id))), sourceIntact: true, started: true, completed: true });
  evidence.assignReview(attempts.Coordinator, 'review-resolved', { assignee: 'Reviewer', candidateId: next.id, instruction: 'Review the reconciled output' });
  evidence.review(attempts.Reviewer, { candidateId: next.id, verdict: 'pass', summary: 'Correct output on the moved target', findings: [], coverage: [{ criterion: 0, assessment: 'Reviewed target conflict', checkIds: ['unit'] }] });
  await f.publication.prepare(attempts.Coordinator, { candidateId: next.id, summary: reviewerSummary });
  await f.publication.flush();
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), next.revision.head);
  assert.deepEqual(f.counts(), { posts: 1, puts: 1 });
});
test('M6.3: a second target conflict resolves against its own pinned ref while the accepted base stays stable', async t => {
  const f = await setup(t, 'crewroom-refresh-adopted-conflict-');
  const { store, run, work, evidence, candidate, owned, git } = f;
  await f.publication.prepare(f.coordinator.attempt.id, { candidateId: candidate.id, summary: reviewerSummary });
  await f.publication.flush();
  const publishedHead = f.remoteHead(`refs/heads/${owned.branch}`);
  assert.equal(publishedHead, candidate.revision.head);
  assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
  f.stop();
  const firstBase = f.advance('unrelated', 'upstream one\n');
  const first = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.ok('candidateId' in first, 'The first refresh creates an adopted candidate.');
  const source = store.codingRecord(run.id)!.candidates.at(-1)!;
  assert.equal(source.id, first.candidateId);
  assert.equal(source.refreshedFrom, candidate.id);
  assert.equal(store.repositoryWork(run.id)!.base, firstBase);
  assert.equal(store.repositoryWork(run.id)!.targetRef, first.refresh!.targetRef);
  const originalHead = source.revision.head;
  const target = f.advance('answer', 'different\n');
  const second = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const conflict = second.refresh!;
  assert.equal(conflict.state, 'conflicted');
  assert.equal(conflict.fromBase, firstBase); assert.equal(conflict.toBase, target);
  assert.equal(conflict.sourceCandidateId, source.id);
  assert.equal((await work.status(run.id)).drift.base, false, 'The accepted target snapshot remains stable while the second conflict is held.');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), originalHead);
  const coding = new Coding(store, work, evidence);
  const resumed = await f.resumeProduction();
  const claim = await coding.claim(resumed.Coordinator, 'second-conflict-claim', { assignee: 'Writer', instruction: 'Resolve only the recorded answer conflict.' });
  assert.equal(claim.writerAttemptId, resumed.Writer);
  const file = await work.readFile(run.id, 'answer');
  assert.match(file.content, /<<<<<<<|>>>>>>>/);
  assert.equal((await work.readFile(run.id, 'unrelated')).content, 'upstream one\n');
  assert.ok((await work.listFiles(run.id)).some(item => item.path === 'answer'));
  assert.ok((await work.search(run.id, { query: 'different' })).matches.some(item => item.path === 'answer'));
  await coding.edit(resumed.Writer, 'second-conflict-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  assert.equal((await coding.continueTargetConflict(resumed.Writer, 'second-conflict-continue', { requestId: conflict.requestId })).state, 'ready');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), originalHead);
  assert.equal(store.repositoryWork(run.id)!.base, firstBase, 'The fetched target is not accepted until stopped promotion.');
  assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), publishedHead, 'The existing PR branch stays pinned until explicit publication.');
  assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
  assert.equal((await evidence.assess(run.id)).ready, false);
  const operation = await coding.requestCompletion(resumed.Writer, 'second-conflict-complete', { summary: 'Reconcile answer with current target.' });
  store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  await work.commit(run.id, operation);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'completed');
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.candidateId, undefined);
  const laterTarget = f.advance('unrelated', 'target changed after branch promotion\n');
  assert.notEqual(laterTarget, target);
  const next = await coding.complete(run.id, operation.id);
  assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === operation.id)!.state, 'completed');
  assert.equal(next.revision.base, target); assert.equal(next.refreshedFrom, source.id);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), next.revision.head);
  assert.equal(git(owned.path, 'symbolic-ref', '--short', 'HEAD'), owned.branch);
  assert.equal((await work.status(run.id)).drift.base, false);
  assert.equal((await coding.complete(run.id, operation.id)).id, next.id);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 3);
  const assessment = await evidence.assess(run.id);
  assert.equal(assessment.candidate?.id, next.id); assert.equal(assessment.ready, false);
  assert.ok(assessment.blockers.includes('Required check unit is missing or failed.'));
  assert.ok(assessment.blockers.includes('Independent review is missing for this revision.'));
});


test('M6.3: second-conflict edit rejects a changed original worktree while leaving the isolated resolution intact', async t => {
  const f = await setup(t, 'crewroom-refresh-source-drift-');
  const { store, run, work, evidence, owned, git } = f;
  f.stop(); f.advance('unrelated', 'upstream one\n'); await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const source = store.codingRecord(run.id)!.candidates.at(-1)!;
  f.advance('answer', 'different\n');
  const conflict = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'source-drift-claim', { assignee: 'Writer', instruction: 'Resolve recorded answer only.' });
  const file = await work.readFile(run.id, 'answer');
  writeFileSync(join(owned.path, 'unrelated'), 'unauthorized source edit\n');
  await assert.rejects(coding.edit(resumed.Writer, 'source-drift-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' }), /workspace|target|original/i);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), source.revision.head);
  assert.equal(readFileSync(join(conflict.resolutionWorkspace!.path, 'answer'), 'utf8'), file.content);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.requestId, conflict.requestId);
  f.stop(); await work.reconcileStopped(run.id);
  assert.equal(store.repositoryWork(run.id)!.writer, undefined, 'Stopped release survives invalidation.');
  assert.equal(store.codingRecord(run.id)!.candidates.length, 2);
});

test('M6.3: corrupted snapshots block conflict admission and edit; live remote changes or outages block stopped promotion', async t => {
  for (const stage of ['admission', 'edit', 'promotion', 'late', 'late-final', 'unavailable'] as const) {
    const f = await setup(t, `crewroom-refresh-target-${stage}-`);
    const { store, run, work, evidence, owned, git } = f;
    if (stage === 'promotion') {
      await f.publication.prepare(f.coordinator.attempt.id, { candidateId: f.candidate.id, summary: reviewerSummary });
      await f.publication.flush();
      assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
    }
    f.stop(); f.advance('unrelated', 'upstream one\n');
    await f.refresh.refresh(run.id, { requestId: randomUUID() });
    const source = store.codingRecord(run.id)!.candidates.at(-1)!;
    f.advance('answer', 'different\n');
    const conflict = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    const coding = new Coding(store, work, evidence);
    let writer: string | undefined; let fileHash: string | undefined; let completionId: string | undefined;
    if (stage !== 'admission') {
      const resumed = f.resume(); writer = resumed.Writer;
      await coding.claim(resumed.Coordinator, `target-${stage}-claim`, { assignee: 'Writer', instruction: 'Resolve recorded answer only.' });
      fileHash = (await work.readFile(run.id, 'answer')).sha256;
      if (stage === 'promotion' || stage === 'late' || stage === 'late-final' || stage === 'unavailable') {
        await coding.edit(writer, 'target-promotion-edit', { path: 'answer', expectedHash: fileHash, content: 'correct and different\n' });
        assert.equal((await coding.continueTargetConflict(writer, 'target-promotion-continue', { requestId: conflict.requestId })).state, 'ready');
        completionId = (await coding.requestCompletion(writer, 'target-promotion-complete', { summary: 'Resolve the current target.' })).id;
        store.beginPause(run.id);
        for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
        store.finishPause(run.id); await work.reconcileStopped(run.id);
      }
    }
    const moved = stage === 'unavailable' || stage === 'late' || stage === 'late-final' ? undefined : f.advance('unrelated', 'upstream twice\n');
    if (stage === 'unavailable') renameSync(f.remote, `${f.remote}-offline`);
    if (stage === 'admission' || stage === 'edit') {
      git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${conflict.targetRef}`);
      assert.equal(git(owned.path, 'rev-parse', conflict.targetRef), moved);
    } else assert.equal(git(owned.path, 'rev-parse', conflict.targetRef), conflict.toBase, 'Live observation leaves the pinned snapshot unchanged.');
    if (stage === 'admission') await assert.rejects(work.validateConflictWorkspace(run.id), /target changed/);
    if (stage === 'edit') {
      await assert.rejects(coding.edit(writer!, 'moved-target-edit', { path: 'answer', expectedHash: fileHash!, content: 'unsafe\n' }), /target changed/);
      f.stop(); await work.reconcileStopped(run.id);
    }
    let lateTarget: string | undefined;
    if (stage === 'late' || stage === 'late-final') {
      let observed = 0;
      work.setTargetObserver(async () => { observed++; if (observed === (stage === 'late' ? 2 : 3)) lateTarget = f.advance('unrelated', 'upstream twice\n'); return f.remoteHead('refs/heads/main'); });
    }
    if (stage === 'promotion' || stage === 'late' || stage === 'late-final' || stage === 'unavailable') await assert.rejects(coding.complete(run.id, completionId!), /target|remote/i);
    if (stage === 'promotion') {
      const rejected = store.codingRecord(run.id)!.completions!.find(item => item.id === completionId)!;
      assert.equal(rejected.state, 'rejected');
      assert.deepEqual(rejected.rejection && { code: rejected.rejection.code, expectedTarget: rejected.rejection.expectedTarget, observedTarget: rejected.rejection.observedTarget },
        { code: 'target_changed', expectedTarget: conflict.toBase, observedTarget: moved });
      assert.equal(readFileSync(join(conflict.resolutionWorkspace!.path, 'answer'), 'utf8'), 'correct and different\n');
      assert.equal(store.deliveryNotices(run.id).filter(item => item.payload.kind === 'target_conflict_rejected' && item.payload.subjectId === completionId).length, 1);
      assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), f.candidate.revision.head);
      assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
      assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
      assert.equal(store.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === conflict.requestId)!.resolutionWorkspace?.operationId, undefined);
      const app = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath: join(f.root, 'roles.json'), publication: [f.connection] });
      t.after(app.close);
      const result = await (await fetch(`${app.origin}/api/runs/${run.id}/result`)).json();
      assert.equal(result.refreshable, true);
      assert.equal(result.completions.find((item: { id: string }) => item.id === completionId).rejection.observedTarget, moved);
      assert.equal(result.refreshes.find((item: { requestId: string }) => item.requestId === conflict.requestId).resolutionWorkspace.path, conflict.resolutionWorkspace!.path);
      const retryId = randomUUID();
      const response = await fetch(`${app.origin}/api/runs/${run.id}/target-refresh`, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' },
        body: JSON.stringify({ requestId: retryId, operatorDecision: { conflictRequestId: conflict.requestId, reason: 'Reviewed the retained resolution against the moved target.' } }) });
      assert.equal(response.status, 200);
      const retry = await response.json();
      assert.equal(retry.refresh?.requestId, retryId);
      assert.equal(retry.refresh?.toBase, moved);
      assert.equal(retry.refresh?.state, 'conflicted');
      assert.equal(store.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === conflict.requestId)?.supersededBy?.requestId, retryId);
      assert.notEqual(retry.refresh?.resolutionWorkspace?.path, conflict.resolutionWorkspace!.path);
      assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === completionId)!.state, 'rejected');
      f.rewind(conflict.toBase);
      assert.deepEqual(await coding.requestCompletion(writer!, 'target-promotion-complete', { summary: 'Resolve the current target.' }), rejected);
      await assert.rejects(coding.complete(run.id, completionId!), /rejected/);
      assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === completionId)!.state, 'rejected');
      assert.equal(f.remoteHead(`refs/heads/${owned.branch}`), f.candidate.revision.head);
      assert.deepEqual(f.counts(), { posts: 1, puts: 0 });
    }
    if (stage === 'late' || stage === 'late-final') {
      const held = store.repositoryWork(run.id)!.refreshes!.at(-1)!;
      const rejected = store.codingRecord(run.id)!.completions!.find(item => item.id === completionId)!;
      assert.equal(rejected.rejection?.observedTarget, lateTarget);
      assert.equal(held.resolutionWorkspace?.state, 'operator_required');
      assert.equal(held.resolutionWorkspace?.operationId, completionId, 'A stopped promoting intent retains its identity.');
    }
    if (stage === 'unavailable') {
      assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === completionId)!.state, 'requested');
      assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'ready');
      assert.equal(store.deliveryNotices(run.id).filter(item => item.payload.subjectId === completionId).length, 0);
    }
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), source.revision.head);
    assert.equal(store.repositoryWork(run.id)!.base, source.revision.base);
    assert.equal(store.codingRecord(run.id)!.candidates.length, 2);
    assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'conflicted');
    assert.equal(store.repositoryWork(run.id)!.writer, undefined);
  }
});

test('M6.3: a sealed pre-launch reset rejects only a newly moved target, and replay retains its workspace', async t => {
  const f = await setup(t, 'crewroom-promotion-seal-');
  const { store, run, work, evidence, owned, git, candidate } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  f.advance('answer', 'different\n');
  const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const resumed = f.resume(); const coding = new Coding(store, work, evidence);
  await coding.claim(resumed.Coordinator, 'seal-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'seal-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const ready = await coding.continueTargetConflict(resumed.Writer, 'seal-continue', { requestId: refresh.requestId });
  const completion = await coding.requestCompletion(resumed.Writer, 'seal-complete', { summary: 'Resolve answer.' });
  store.beginCodingTransition(run.id, completion.id); store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const process = await work.describePromotionProcess(store.repositoryWork(run.id)!, completion.id, ready.head!);
  store.beginTargetPromotion(run.id, refresh.requestId, completion.id, ready.head!, process);
  work.preparePromotionProcess(run.id, refresh.requestId, completion.id);
  const original = git(owned.path, 'rev-parse', 'HEAD');
  const moved = f.advance('unrelated', 'new target\n');
  await f.authority.close();
  const nextAuthority = await acquireController(join(f.root, 'promotion-controller')); t.after(() => nextAuthority.close());
  const reopened = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => reopened.close());
  const recoveredWork = new RepositoryWork(reopened, join(f.root, 'owned'), nextAuthority);
  const recoveredPublication = new Publication(reopened, recoveredWork, new Evidence(reopened, recoveredWork, join(f.root, 'artifacts')), join(f.root, 'publication-restart'), [f.connection]);
  t.after(() => recoveredPublication.close());
  await new TargetRefresh(reopened, recoveredWork, new Evidence(reopened, recoveredWork, join(f.root, 'artifacts')), recoveredPublication).reconcile();
  assert.equal((await inspectCheckedOwned(process)).state, 'stopped');
  assert.equal((await inspectCheckedOwned(process)).owner, undefined);
  assert.equal(reopened.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.rejection?.observedTarget, moved);
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
  assert.equal(reopened.getRun(run.id).codingTransition, undefined);
  assert.equal(reopened.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 1);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), original);
  assert.equal(readFileSync(join(refresh.resolutionWorkspace!.path, 'answer'), 'utf8'), 'correct and different\n');
  assert.deepEqual(f.counts(), { posts: 0, puts: 0 });
  assert.equal(existsSync(join(process.root, 'seal.json')), true);
  await assert.rejects(Promise.resolve().then(() => launchCheckedOwned(process, nextAuthority)), /Stale controller epoch/);
  await recoveredWork.reconcileRefresh(run.id);
  assert.equal(reopened.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 1);
});

test('M6.3: restart seals missing/prepared launches across Git upgrades but holds corrupt manifests', async t => {
  const baselinePath = globalThis.process.env.PATH!;
  t.after(() => { globalThis.process.env.PATH = baselinePath; });
  for (const state of ['missing', 'prepared', 'corrupt'] as const) {
    const prepared = state !== 'missing';
    const f = await setup(t, `crewroom-promotion-noop-${state}-`);
    const { store, run, work, evidence, owned, git, candidate } = f;
    f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
    f.advance('answer', 'different\n');
    const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    const resumed = f.resume(); const coding = new Coding(store, work, evidence);
    await coding.claim(resumed.Coordinator, `noop-claim-${prepared}`, { assignee: 'Writer', instruction: 'Resolve answer only.' });
    const file = await work.readConflict(run.id, 'answer');
    await coding.edit(resumed.Writer, `noop-edit-${prepared}`, { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
    const ready = await coding.continueTargetConflict(resumed.Writer, `noop-continue-${prepared}`, { requestId: refresh.requestId });
    const completion = await coding.requestCompletion(resumed.Writer, `noop-complete-${prepared}`, { summary: 'Resolve answer.' });
    store.beginCodingTransition(run.id, completion.id); store.beginPause(run.id);
    for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
    store.finishPause(run.id); await work.reconcileStopped(run.id);
    const orphan = !prepared ? await work.describePromotionProcess(store.repositoryWork(run.id)!, completion.id, ready.head!) : undefined;
    if (orphan) prepareCheckedOwned(orphan.root, orphan.token, orphan.command, orphan.args, { cwd: orphan.cwd, env: orphan.env }, f.authority);
    const process = await work.describePromotionProcess(store.repositoryWork(run.id)!, completion.id, ready.head!);
    if (orphan) assert.notEqual(orphan.root, process.root, 'An orphan prepared root cannot reserve a future attempt.');
    assert.equal(existsSync(process.root), false, 'No launch root is created before the durable intent.');
    assert.throws(() => work.preparePromotionProcess(run.id, refresh.requestId, completion.id), /Persist the exact promotion intent/);
    store.beginTargetPromotion(run.id, refresh.requestId, completion.id, ready.head!, process);
    if (prepared) work.preparePromotionProcess(run.id, refresh.requestId, completion.id);
    if (state === 'corrupt') writeFileSync(join(process.root, 'launch.json'), 'corrupt launch manifest');
    assert.equal(existsSync(join(process.root, 'launch.json')), prepared);
    await f.authority.close();
    if (state === 'prepared') {
      const upgrade = join(f.root, 'git-upgrade'); mkdirSync(upgrade);
      const executable = join(upgrade, 'git'); copyFileSync(process.command, executable); chmodSync(executable, 0o755);
      globalThis.process.env.PATH = `${upgrade}:${baselinePath}`;
      assert.notEqual(executable, process.command);
    }
    const nextAuthority = await acquireController(join(f.root, 'promotion-controller')); t.after(() => nextAuthority.close());
    const reopened = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => reopened.close());
    const recoveredWork = new RepositoryWork(reopened, join(f.root, 'owned'), nextAuthority);
    const recoveredEvidence = new Evidence(reopened, recoveredWork, join(f.root, 'artifacts'));
    const recoveredPublication = new Publication(reopened, recoveredWork, recoveredEvidence, join(f.root, 'publication-restart'), [f.connection]);
    t.after(() => recoveredPublication.close());
    const service = new TargetRefresh(reopened, recoveredWork, recoveredEvidence, recoveredPublication);
    await service.reconcile();
    if (state === 'corrupt') {
      assert.equal(reopened.getRun(run.id).status, 'attention_required');
      assert.equal(reopened.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'requested');
      assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoting');
      assert.equal((await inspectCheckedOwned(process)).state, 'uncertain');
      assert.equal(reopened.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 0);
      assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
      continue;
    }
    const terminal = reopened.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)!;
    assert.equal(terminal.state, 'rejected'); assert.equal(terminal.rejection?.code, 'promotion_interrupted');
    assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
    assert.equal(reopened.getRun(run.id).codingTransition, undefined);
    assert.equal(reopened.repositoryWork(run.id)!.base, candidate.revision.base);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal((await inspectCheckedOwned(process)).state, 'stopped');
    assert.equal(existsSync(join(process.root, 'seal.json')), true);
    assert.equal(reopened.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 1);
    assert.deepEqual(await new Coding(reopened, recoveredWork, recoveredEvidence).requestCompletion(resumed.Writer, `noop-complete-${prepared}`, { summary: 'Resolve answer.' }), terminal);
    await assert.rejects(new Coding(reopened, recoveredWork, recoveredEvidence).complete(run.id, completion.id), /rejected/);
    if (state === 'prepared') {
      const snapshot = join(f.root, 'legacy-checked-terminal.sqlite');
      const sourceDb = new DatabaseSync(join(f.root, 'workflow.sqlite'));
      try { await backup(sourceDb, snapshot); } finally { sourceDb.close(); }
      const downgrade = new DatabaseSync(snapshot); downgrade.exec('PRAGMA user_version=22;'); downgrade.close();
      const migrated = new Workbench(snapshot); t.after(() => migrated.close());
      const migratedWork = new RepositoryWork(migrated, join(f.root, 'owned'));
      const migratedEvidence = new Evidence(migrated, migratedWork, join(f.root, 'artifacts'));
      const migratedPublication = new Publication(migrated, migratedWork, migratedEvidence, join(f.root, 'publication-v22-checked'), [f.connection]);
      t.after(() => migratedPublication.close());
      assert.equal(migrated.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
      assert.equal((await new Results(migrated, migratedWork, migratedEvidence, migratedPublication).read(run.id)).refreshable, true);
      assert.deepEqual(await new Coding(migrated, migratedWork, migratedEvidence).requestCompletion(resumed.Writer, 'noop-complete-true', { summary: 'Resolve answer.' }), terminal);
      for (const mismatch of ['missing_descriptor', 'missing_seal', 'wrong_operation', 'wrong_completion', 'wrong_target', 'wrong_source', 'wrong_digest'] as const) {
        const copy = join(f.root, `legacy-checked-${mismatch}.sqlite`);
        const src = new DatabaseSync(snapshot); try { await backup(src, copy); } finally { src.close(); }
        const db = new DatabaseSync(copy);
        const record = structuredClone(migrated.repositoryWork(run.id)!);
        const conflict = record.refreshes!.find(item => item.requestId === refresh.requestId)!;
        const codingRecord = structuredClone(migrated.codingRecord(run.id)!);
        const recorded = codingRecord.completions!.find(item => item.id === completion.id)!;
        if (mismatch === 'missing_descriptor') delete conflict.resolutionWorkspace!.promotionProcess;
        if (mismatch === 'wrong_operation') conflict.resolutionWorkspace!.operationId = randomUUID();
        if (mismatch === 'wrong_completion') recorded.conflictRefreshId = randomUUID();
        if (mismatch === 'wrong_target') recorded.rejection!.expectedTarget = candidate.revision.base;
        if (mismatch === 'wrong_source') conflict.sourceCandidateId = randomUUID();
        if (mismatch === 'wrong_digest') conflict.resolutionWorkspace!.promotionProcess!.specSha256 = '0'.repeat(64);
        db.prepare('UPDATE repository_work SET data=? WHERE run_id=?').run(JSON.stringify(record), run.id);
        db.prepare('UPDATE coding SET data=? WHERE run_id=?').run(JSON.stringify(codingRecord), run.id);
        db.exec('PRAGMA user_version=22;'); db.close();
        const seal = join(process.root, 'seal.json'); const hiddenSeal = join(process.root, 'seal.migration-test');
        if (mismatch === 'missing_seal') renameSync(seal, hiddenSeal);
        try {
          const held = new Workbench(copy); t.after(() => held.close());
          const heldWork = new RepositoryWork(held, join(f.root, 'owned'));
          assert.equal(held.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === refresh.requestId)?.resolutionWorkspace?.state, 'continuing', mismatch);
          assert.equal(held.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === refresh.requestId)?.resolutionWorkspace?.legacyOwnershipUnknown, true);
          assert.equal((await new Results(held, heldWork, new Evidence(held, heldWork, join(f.root, 'artifacts'))).read(run.id)).refreshable, false);
          assert.equal(readFileSync(join(refresh.resolutionWorkspace!.path, 'answer'), 'utf8'), 'correct and different\n');
        } finally { if (mismatch === 'missing_seal') renameSync(hiddenSeal, seal); }
      }
      const migratedRetry = await new TargetRefresh(migrated, migratedWork, migratedEvidence, migratedPublication).refresh(run.id,
        { requestId: randomUUID(), operatorDecision: { conflictRequestId: refresh.requestId, reason: 'The exact checked rejection was migrated; retry against the current target.' } });
      assert.equal(migratedRetry.refresh?.supersedes?.requestId, refresh.requestId);
      assert.equal(migrated.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'rejected');
    }
    const requestId = randomUUID();
    const retry = await service.refresh(run.id, { requestId, operatorDecision: { conflictRequestId: refresh.requestId, reason: 'The checked reset did not apply; retry the resolution against the verified current target.' } });
    assert.equal(retry.refresh?.requestId, requestId);
    assert.equal(retry.refresh?.toBase, refresh.toBase);
    assert.equal(reopened.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === refresh.requestId)?.supersededBy?.requestId, requestId);
    assert.equal(reopened.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'rejected');
    assert.equal(readFileSync(join(refresh.resolutionWorkspace!.path, 'answer'), 'utf8'), 'correct and different\n');
    globalThis.process.env.PATH = baselinePath;
  }
});

test('M6.3: a stopped checked reset at the clean source terminates without a success receipt', async t => {
  for (const targetMoves of [false, true]) {
    const f = await setup(t, `crewroom-promotion-failed-${targetMoves}-`);
    const { store, run, work, evidence, owned, git, candidate } = f;
    f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
    f.advance('answer', 'different\n');
    const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    const resumed = f.resume(); const coding = new Coding(store, work, evidence);
    await coding.claim(resumed.Coordinator, `failed-claim-${targetMoves}`, { assignee: 'Writer', instruction: 'Resolve answer only.' });
    const file = await work.readConflict(run.id, 'answer');
    await coding.edit(resumed.Writer, `failed-edit-${targetMoves}`, { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
    const ready = await coding.continueTargetConflict(resumed.Writer, `failed-continue-${targetMoves}`, { requestId: refresh.requestId });
    const completion = await coding.requestCompletion(resumed.Writer, `failed-complete-${targetMoves}`, { summary: 'Resolve answer.' });
    store.beginPause(run.id);
    for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
    store.finishPause(run.id); await work.reconcileStopped(run.id);
    const operation = await work.describePromotionProcess(store.repositoryWork(run.id)!, completion.id, ready.head!);
    store.beginTargetPromotion(run.id, refresh.requestId, completion.id, ready.head!, operation);
    work.preparePromotionProcess(run.id, refresh.requestId, completion.id);
    const indexLock = git(owned.path, 'rev-parse', '--path-format=absolute', '--git-path', 'index.lock');
    writeFileSync(indexLock, 'Block this one checked Git reset.');
    const child = launchCheckedOwned(operation, f.authority); child.stderr?.resume();
    const code = await new Promise<number | null>(resolve => child.once('close', resolve));
    assert.notEqual(code, 0); rmSync(indexLock);
    assert.equal((await inspectCheckedOwned(operation)).state, 'stopped');
    assert.ok((await inspectCheckedOwned(operation)).owner, 'The checked helper executed, but Git did not apply.');
    const moved = targetMoves ? f.advance('unrelated', 'later target\n') : undefined;
    await f.authority.close();
    const nextAuthority = await acquireController(join(f.root, 'promotion-controller')); t.after(() => nextAuthority.close());
    const reopened = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => reopened.close());
    const recoveredWork = new RepositoryWork(reopened, join(f.root, 'owned'), nextAuthority);
    const recoveredEvidence = new Evidence(reopened, recoveredWork, join(f.root, 'artifacts'));
    const recoveredPublication = new Publication(reopened, recoveredWork, recoveredEvidence, join(f.root, 'publication-restart'), [f.connection]);
    t.after(() => recoveredPublication.close());
    await new TargetRefresh(reopened, recoveredWork, recoveredEvidence, recoveredPublication).reconcile();
    const terminal = reopened.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)!;
    assert.equal(terminal.state, 'rejected');
    assert.equal(terminal.rejection?.code, targetMoves ? 'target_changed' : 'promotion_interrupted');
    if (targetMoves) assert.equal(terminal.rejection?.observedTarget, moved);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
    assert.equal(reopened.repositoryWork(run.id)!.base, candidate.revision.base);
  }
});

test('M6.3: stopped real checked reset finishes once after controller restart and Git path change', async t => {
  const f = await setup(t, 'crewroom-promotion-reset-restart-');
  const { store, run, work, evidence, owned, git, candidate } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  f.advance('answer', 'different\n');
  const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const resumed = f.resume(); const coding = new Coding(store, work, evidence);
  await coding.claim(resumed.Coordinator, 'reset-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'reset-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const ready = await coding.continueTargetConflict(resumed.Writer, 'reset-continue', { requestId: refresh.requestId });
  const completion = await coding.requestCompletion(resumed.Writer, 'reset-complete', { summary: 'Resolve answer.' });
  store.beginCodingTransition(run.id, completion.id); store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const process = await work.describePromotionProcess(store.repositoryWork(run.id)!, completion.id, ready.head!);
  store.beginTargetPromotion(run.id, refresh.requestId, completion.id, ready.head!, process);
  const previousPath = globalThis.process.env.PATH!;
  t.after(() => { globalThis.process.env.PATH = previousPath; });
  const upgrade = join(f.root, 'git-upgrade'); mkdirSync(upgrade);
  const executable = join(upgrade, 'git'); copyFileSync(process.command, executable); chmodSync(executable, 0o755);
  globalThis.process.env.PATH = `${upgrade}:${previousPath}`;
  assert.notEqual(executable, process.command);
  assert.throws(() => work.preparePromotionProcess(run.id, refresh.requestId, completion.id), /Checked Git promotion identity/);
  assert.equal(existsSync(process.root), false, 'Changed Git cannot prepare the admitted launch.');
  globalThis.process.env.PATH = previousPath;
  work.preparePromotionProcess(run.id, refresh.requestId, completion.id);
  const child = launchCheckedOwned(process, f.authority); child.stderr?.resume();
  const code = await new Promise<number | null>(resolve => child.once('close', resolve));
  assert.equal(code, 0); assert.equal((await inspectCheckedOwned(process)).state, 'stopped');
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), ready.head);
  assert.equal(store.repositoryWork(run.id)!.base, refresh.fromBase, 'Git moved but the checkpoint was not recorded.');
  await f.authority.close();
  globalThis.process.env.PATH = `${upgrade}:${previousPath}`;
  assert.notEqual(executable, process.command);
  const nextAuthority = await acquireController(join(f.root, 'promotion-controller')); t.after(() => nextAuthority.close());
  const reopened = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => reopened.close());
  const recoveredWork = new RepositoryWork(reopened, join(f.root, 'owned'), nextAuthority);
  const recoveredEvidence = new Evidence(reopened, recoveredWork, join(f.root, 'artifacts'));
  const recoveredPublication = new Publication(reopened, recoveredWork, recoveredEvidence, join(f.root, 'publication-restart'), [f.connection]);
  t.after(() => recoveredPublication.close());
  const service = new TargetRefresh(reopened, recoveredWork, recoveredEvidence, recoveredPublication);
  await service.reconcile(); await service.reconcile();
  assert.equal(reopened.repositoryWork(run.id)!.base, refresh.toBase);
  assert.equal(reopened.repositoryWork(run.id)!.refreshes!.at(-1)?.resolutionWorkspace?.state, 'promoted');
  const next = await new Coding(reopened, recoveredWork, recoveredEvidence).complete(run.id, completion.id);
  assert.equal((await new Coding(reopened, recoveredWork, recoveredEvidence).complete(run.id, completion.id)).id, next.id);
  assert.equal(reopened.codingRecord(run.id)!.candidates.length, 2);
  assert.equal(reopened.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'completed');
  assert.equal(reopened.repositoryWork(run.id)!.checkpoints.filter(item => item.reason === 'refreshed').length, 1);
  assert.equal(existsSync(refresh.resolutionWorkspace!.path), false);
  assert.deepEqual(f.counts(), { posts: 0, puts: 0 });
});

test('M6.3: a live checked reset holds promotion even when the remote moves during execution', async t => {
  const f = await setup(t, 'crewroom-promotion-live-');
  const { store, run, work, evidence, owned, git, candidate } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  f.advance('answer', 'different\n');
  const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const resumed = f.resume(); const coding = new Coding(store, work, evidence);
  await coding.claim(resumed.Coordinator, 'live-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'live-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  const ready = await coding.continueTargetConflict(resumed.Writer, 'live-continue', { requestId: refresh.requestId });
  const completion = await coding.requestCompletion(resumed.Writer, 'live-complete', { summary: 'Resolve answer.' });
  store.beginCodingTransition(run.id, completion.id); store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  const latch = gitLatch(f.root, 'reset');
  t.after(() => latch.restore());
  const operation = await work.describePromotionProcess(store.repositoryWork(run.id)!, completion.id, ready.head!);
  store.beginTargetPromotion(run.id, refresh.requestId, completion.id, ready.head!, operation);
  work.preparePromotionProcess(run.id, refresh.requestId, completion.id);
  // Hold the real reset after the checked helper registered ownership. A filesystem
  // watcher plus SIGSTOP races a fast Git exit (especially with delayed observation).
  latch.arm();
  const child = launchCheckedOwned(operation, f.authority); let diagnostic = '';
  child.stderr?.on('data', chunk => { diagnostic += chunk; });
  const exited = new Promise<number | null>((resolve, reject) => { child.once('close', resolve); child.once('error', reject); });
  try {
    await Promise.race([latch.blocked(), exited.then(code => { throw new Error(`Checked helper exited before reset latch: ${code}; ${diagnostic}`); })]);
    assert.equal((await inspectCheckedOwned(operation)).state, 'live');
    f.advance('unrelated', 'target advanced during reset\n');
    await assert.rejects(work.reconcileRefresh(run.id), /ownership is live or uncertain/);
    assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoting');
    assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'requested');
    assert.equal(store.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 0);
    assert.equal(store.repositoryWork(run.id)!.base, refresh.fromBase);
  } finally { latch.release(); await exited; }
  assert.equal(await exited, 0);
  assert.equal((await inspectCheckedOwned(operation)).state, 'stopped');
  await work.reconcileRefresh(run.id);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), ready.head);
  assert.equal(store.repositoryWork(run.id)!.base, refresh.toBase);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoted');
  assert.equal(store.getRun(run.id).status, 'attention_required', 'The remote moved while reset ownership was live; local promotion is stale.');
  const before = store.runtime(run.id).assignments.map(item => item.currentAttemptId);
  const runtime = new RuntimeSupervisor(store, { root: join(f.root, 'hosts'), socket: `/tmp/crewroom-${run.id.slice(0,8)}.sock`, ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9', () => [], work, { coding, evidence });
  await runtime.initialize(); await runtime.close();
  assert.deepEqual(store.runtime(run.id).assignments.map(item => item.currentAttemptId), before);
  assert.equal(store.getRun(run.id).status, 'attention_required');
  assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'requested');
});

test('M6.3: source branch moving during the last live-target observation cannot dispatch a checked reset', async t => {
  const f = await setup(t, 'crewroom-promotion-source-race-');
  const { store, run, work, evidence, owned, git, candidate } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  f.advance('answer', 'different\n');
  const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const resumed = f.resume(); const coding = new Coding(store, work, evidence);
  await coding.claim(resumed.Coordinator, 'source-race-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  await coding.edit(resumed.Writer, 'source-race-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  await coding.continueTargetConflict(resumed.Writer, 'source-race-continue', { requestId: refresh.requestId });
  const completion = await coding.requestCompletion(resumed.Writer, 'source-race-complete', { summary: 'Resolve answer.' });
  store.beginPause(run.id);
  for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
  store.finishPause(run.id); await work.reconcileStopped(run.id);
  let observations = 0;
  work.setTargetObserver(async () => {
    if (++observations === 3) git(owned.path, 'reset', '--keep', candidate.revision.base);
    return f.remoteHead('refs/heads/main');
  });
  await assert.rejects(coding.complete(run.id, completion.id), /Original branch or target changed during promotion/);
  assert.equal(observations, 3);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.base);
  assert.equal(store.repositoryWork(run.id)!.base, candidate.revision.base);
  assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoting');
  assert.equal(store.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)?.state, 'requested');
  assert.equal(existsSync(join(store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace!.promotionProcess!.root, 'owner.json')), false);
});

test('M6.3: reopened ready requests reject a moved target; unowned promoting intent stays uncertain', async t => {
  for (const state of ['ready', 'ready-paused', 'promoting', 'moved', 'dirty', 'uncertain'] as const) {
    const f = await setup(t, `crewroom-conflict-recovery-${state}-`);
    const { store, run, work, evidence, owned, git } = f;
    f.stop(); f.advance('unrelated', 'first upstream\n'); await f.refresh.refresh(run.id, { requestId: randomUUID() });
    f.advance('answer', 'other answer\n');
    const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    const coding = new Coding(store, work, evidence); const resumed = f.resume();
    await coding.claim(resumed.Coordinator, `${state}-claim`, { assignee: 'Writer', instruction: 'Resolve only answer.' });
    const file = await work.readFile(run.id, 'answer');
    await coding.edit(resumed.Writer, `${state}-edit`, { path: 'answer', expectedHash: file.sha256, content: 'resolved on first upstream\n' });
    assert.equal((await coding.continueTargetConflict(resumed.Writer, `${state}-continue`, { requestId: refresh.requestId })).state, 'ready');
    const completion = await coding.requestCompletion(resumed.Writer, `${state}-completion`, { summary: 'Resolve answer.' });
    store.beginCodingTransition(run.id, completion.id);
    store.beginPause(run.id);
    for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
    store.finishPause(run.id); await work.reconcileStopped(run.id);
    const workspace = store.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace!;
    if (state !== 'ready' && state !== 'ready-paused') await legacyPromotion(store, work, f.root, run.id, refresh.requestId, completion.id, workspace.toHead!);
    if (state === 'moved') git(owned.path, 'reset', '--keep', workspace.toHead!);
    if (state === 'dirty') writeFileSync(join(owned.path, 'answer'), 'unauthorized dirty content\n');
    if (state === 'uncertain') store.recordOwnership(resumed.Writer, false);
    if (state === 'ready-paused') store.setDesiredState(run.id, 'paused');
    const moved = f.advance('unrelated', 'second upstream\n');
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const recoveredWork = new RepositoryWork(recovered, join(f.root, 'owned'));
    recoveredWork.setTargetObserver(async () => f.remoteHead('refs/heads/main'));
    const recoveredEvidence = new Evidence(recovered, recoveredWork, join(f.root, 'artifacts'));
    const runtime = new RuntimeSupervisor(recovered, { root: join(f.root, 'hosts'), socket: `/tmp/crewroom-${run.id.slice(0,8)}.sock`, ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9', () => [], recoveredWork,
      { coding: new Coding(recovered, recoveredWork, recoveredEvidence), evidence: recoveredEvidence });
    if (state === 'ready' || state === 'ready-paused') {
      await new TargetRefresh(recovered, recoveredWork, recoveredEvidence).reconcile();
      await runtime.initialize(); await runtime.close();
      assert.equal(recovered.getRun(run.id).status, 'paused', 'No agent restarts after the ready completion is rejected.');
      const rejected = recovered.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)!;
      assert.equal(rejected.state, 'rejected');
      assert.equal(rejected.rejection?.expectedTarget, refresh.toBase);
      assert.equal(rejected.rejection?.observedTarget, moved);
      assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'operator_required');
      assert.equal(recovered.getRun(run.id).codingTransition, undefined);
      const notices = recovered.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id);
      assert.equal(notices.length, 1);
      await recoveredWork.reconcileRefresh(run.id);
      assert.equal(recovered.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 1);
      assert.deepEqual(await new Coding(recovered, recoveredWork, new Evidence(recovered, recoveredWork, join(f.root, 'artifacts')))
        .requestCompletion(resumed.Writer, `${state}-completion`, { summary: 'Resolve answer.' }), rejected);
      assert.equal(readFileSync(join(workspace.path, 'answer'), 'utf8'), 'resolved on first upstream\n');
      assert.equal(git(owned.path, 'rev-parse', 'HEAD'), refresh.fromHead);
    } else {
      if (state === 'promoting') {
        await runtime.initialize(); await runtime.close();
        assert.equal(recovered.getRun(run.id).status, 'attention_required', 'Unknown Git mutation ownership requires attention, not replay.');
      } else await runtime.close();
      await assert.rejects(recoveredWork.reconcileRefresh(run.id));
      assert.equal(recovered.codingRecord(run.id)!.completions!.find(item => item.id === completion.id)!.state, 'requested');
      assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoting');
      assert.equal(recovered.getRun(run.id).codingTransition?.completionId, completion.id);
      assert.equal(recovered.deliveryNotices(run.id).filter(item => item.payload.subjectId === completion.id).length, 0);
    }
  }
});

test('M6.3: detaching a ready second-conflict workspace prevents promotion and preserves release', async t => {
  const f = await setup(t, 'crewroom-refresh-second-detached-');
  const { store, run, work, evidence, owned, git } = f;
  f.stop(); f.advance('unrelated', 'upstream one\n'); await f.refresh.refresh(run.id, { requestId: randomUUID() });
  const source = store.codingRecord(run.id)!.candidates.at(-1)!;
  f.advance('answer', 'different\n');
  const conflict = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const coding = new Coding(store, work, evidence); const resumed = f.resume();
  await coding.claim(resumed.Coordinator, 'second-detached-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readFile(run.id, 'answer');
  await coding.edit(resumed.Writer, 'second-detached-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
  assert.equal((await coding.continueTargetConflict(resumed.Writer, 'second-detached-continue', { requestId: conflict.requestId })).state, 'ready');
  git(conflict.resolutionWorkspace!.path, 'checkout', '--detach');
  await assert.rejects(coding.requestCompletion(resumed.Writer, 'second-detached-completion', { summary: 'Do not promote detached source.' }), /workspace or target changed/);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), source.revision.head);
  f.stop(); await work.reconcileStopped(run.id);
  assert.equal(store.repositoryWork(run.id)!.writer, undefined);
  assert.equal(store.repositoryWork(run.id)!.base, source.revision.base);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 2);
});


test('M6.3: restarting during isolated conflict preparation retains the unmerged worktree for operator inspection', async t => {
  const f = await setup(t, 'crewroom-refresh-preparation-');
  const { root, store, run, owned, git, candidate } = f;
  const target = f.advance('answer', 'different\n'); f.stop();
  const requestId = randomUUID(); const targetRef = `refs/crewroom/${run.id}/target`;
  git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${targetRef}`);
  store.beginTargetRefresh(run.id, { requestId, at: new Date().toISOString(), fromBase: candidate.revision.base, fromHead: candidate.revision.head, targetRef, toBase: target, sourceCandidateId: candidate.id, state: 'rebasing' });
  const path = join(dirname(owned.path), `${run.id}-refresh-${requestId}`);
  const branch = `${owned.branch}-refresh-${requestId}`;
  store.recordTargetRefreshWorkspace(run.id, requestId, { path, branch, state: 'preparing' }, ['answer']);
  git(owned.repository.path, 'worktree', 'add', '-b', branch, path, candidate.revision.head);
  store.recordTargetRefreshWorkspace(run.id, requestId, { path, branch, state: 'preparing', gitFile: readFileSync(join(path, '.git'), 'utf8') });
  assert.throws(() => git(path, '-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', 'rebase', '--onto', target, candidate.revision.base));
  const recovered = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => recovered.close());
  const recoveredWork = new RepositoryWork(recovered, join(root, 'owned'));
  const service = new TargetRefresh(recovered, recoveredWork, new Evidence(recovered, recoveredWork, join(root, 'artifacts')));
  await assert.rejects(recoveredWork.reconcileRefresh(run.id), /unproved|preparation|inspection/);
  const refresh = recovered.repositoryWork(run.id)!.refreshes!.at(-1)!;
  assert.equal(refresh.state, 'rebasing'); assert.equal(refresh.resolutionWorkspace?.state, 'preparing');
  assert.deepEqual(refresh.conflicts, ['answer']); assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(git(path, 'diff', '--name-only', '--diff-filter=U'), 'answer');
  await service.reconcile();
  assert.equal(recovered.getRun(run.id).status, 'attention_required');
  assert.equal(recovered.repositoryWork(run.id)!.refreshes!.length, 1);
});

test('M6.3: loopback refresh returns a durable ordinary no-op and a new request observes later remote movement', async t => {
  const f = await setup(t, 'crewroom-refresh-http-noop-');
  f.stop();
  const app = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath: join(f.root, 'roles.json'), publication: [f.connection] });
  t.after(app.close);
  const post = (requestId: string) => fetch(`${app.origin}/api/runs/${f.run.id}/target-refresh`, {
    method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ requestId }) });
  const firstId = randomUUID(); const first = await post(firstId);
  assert.equal(first.status, 200);
  const receipt = await first.json();
  assert.equal(receipt.changed, false); assert.equal(receipt.refresh.candidateId, f.candidate.id);
  const moved = f.advance('unrelated', 'target advanced\n');
  const replay = await post(firstId); assert.equal(replay.status, 200);
  assert.deepEqual(await replay.json(), receipt);
  const next = await post(randomUUID()); assert.equal(next.status, 200);
  const refreshed = await next.json(); assert.equal(refreshed.changed, true);
  assert.equal(refreshed.refresh.toBase, moved);
  assert.equal(f.store.repositoryWork(f.run.id)!.targetRef, refreshed.refresh.targetRef);
});

test('M6.3: loopback results keep an open conflict visible and reject another refresh without retouching the target', async t => {
  const f = await setup(t, 'crewroom-refresh-http-');
  const { run, candidate, owned, git } = f;
  f.advance('answer', 'different\n'); f.stop();
  const requestId = randomUUID(); const original = await f.refresh.refresh(run.id, { requestId });
  assert.equal(original.refresh?.state, 'conflicted');
  const app = await startServer({ port: 0, dataDir: f.root, serveUi: false, rolePath: join(f.root, 'roles.json') });
  t.after(app.close);
  const result = await (await fetch(`${app.origin}/api/runs/${run.id}/result`)).json();
  assert.equal(result.assessment.ready, false);
  assert.equal(result.refreshes.at(-1).resolutionWorkspace.state, 'conflicted');
  assert.equal(result.refreshable, false);
  assert.match(result.assessment.blockers.join(' '), /target conflict/i);
  const post = (id: string) => fetch(`${app.origin}/api/runs/${run.id}/target-refresh`, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ requestId: id }) });
  const replay = await post(requestId); assert.equal(replay.status, 200); assert.equal((await replay.json()).changed, false);
  const blocked = await post(randomUUID()); assert.equal(blocked.status, 409);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(f.store.repositoryWork(run.id)!.refreshes!.length, 1);
});

test('M6.3: reopened adopted-target promotion with unknown Git child ownership remains held on either side of branch movement', async t => {
  for (const movedBeforeRestart of [false, true]) {
    const f = await setup(t, `crewroom-refresh-adopted-recover-${movedBeforeRestart}-`);
    const { root, store, work, evidence, run, owned, git } = f;
    f.stop(); f.advance('unrelated', 'upstream one\n'); await f.refresh.refresh(run.id, { requestId: randomUUID() });
    const source = store.codingRecord(run.id)!.candidates.at(-1)!;
    f.advance('answer', 'different\n');
    const held = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
    assert.equal((await work.status(run.id)).drift.base, false);
    const resumed = f.resume(); const coding = new Coding(store, work, evidence);
    await coding.claim(resumed.Coordinator, `adopted-recover-claim-${movedBeforeRestart}`, { assignee: 'Writer', instruction: 'Resolve answer only.' });
    const file = await work.readConflict(run.id, 'answer');
    await coding.edit(resumed.Writer, `adopted-recover-edit-${movedBeforeRestart}`, { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
    const ready = await coding.continueTargetConflict(resumed.Writer, `adopted-recover-continue-${movedBeforeRestart}`, { requestId: held.requestId });
    assert.equal(ready.state, 'ready');
    const operation = await coding.requestCompletion(resumed.Writer, `adopted-recover-complete-${movedBeforeRestart}`, { summary: 'Resolve answer on the fetched target.' });
    store.beginPause(run.id);
    for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
    store.finishPause(run.id); await work.reconcileStopped(run.id);
    await legacyPromotion(store, work, root, run.id, held.requestId, operation.id, ready.head!);
    if (movedBeforeRestart) git(owned.path, 'reset', '--keep', ready.head!);
    const recovered = new Workbench(join(root, 'workflow.sqlite')); t.after(() => recovered.close());
    const recoveredWork = new RepositoryWork(recovered, join(root, 'owned'));
    const recoveredEvidence = new Evidence(recovered, recoveredWork, join(root, 'artifacts'));
    const recoveredPublication = new Publication(recovered, recoveredWork, recoveredEvidence, join(root, 'publication-recovery'), [f.connection]);
    t.after(() => recoveredPublication.close());
    const recovery = new TargetRefresh(recovered, recoveredWork, recoveredEvidence, recoveredPublication);
    await recovery.reconcile();
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), movedBeforeRestart ? ready.head : source.revision.head);
    assert.equal(recovered.repositoryWork(run.id)!.base, source.revision.base);
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoting');
    assert.equal(recovered.codingRecord(run.id)!.completions!.find(item => item.id === operation.id)!.state, 'requested');
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 2);
    await assert.rejects(recoveredWork.reconcileRefresh(run.id), /Checked Git promotion identity is unavailable/);
    await assert.rejects(new Coding(recovered, recoveredWork, recoveredEvidence).complete(run.id, operation.id), /Confirm stopped ownership/);
    assert.equal(recovered.repositoryWork(run.id)!.base, source.revision.base);
  }
});

test('M6.3: restarted first-conflict promotion remains uncertain until managed Git ownership can be proved', async t => {
  for (const movedBeforeRestart of [false, true]) {
    const f = await setup(t, `crewroom-refresh-promote-${movedBeforeRestart}-`);
    const { root, store, work, evidence, run, owned, candidate, git } = f;
    f.advance('answer', 'different\n'); f.stop();
    store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
    const result = await f.refresh.refresh(run.id, { requestId: randomUUID() });
    const held = result.refresh; assert.ok(held); assert.equal(held.state, 'conflicted');
    const resumed = f.resume(); const coding = new Coding(store, work, evidence);
    await coding.claim(resumed.Coordinator, 'reconcile-claim', { assignee: 'Writer', instruction: 'Resolve answer within the accepted brief.' });
    const file = await work.readConflict(run.id, 'answer');
    await coding.edit(resumed.Writer, 'reconcile-edit', { path: 'answer', expectedHash: file.sha256, content: 'correct and different\n' });
    const workspace = held.resolutionWorkspace!.path;
    git(workspace, 'add', '--', 'answer');
    git(workspace, '-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', 'rebase', '--continue');
    const toHead = git(workspace, 'rev-parse', 'HEAD');
    store.recordTargetRefreshWorkspace(run.id, held.requestId, { ...held.resolutionWorkspace!, state: 'ready', toHead });
    const operation = await coding.requestCompletion(resumed.Writer, 'reconcile-complete', { summary: 'Resolve accepted target conflict' });
    store.beginPause(run.id);
    for (const item of store.runtime(run.id).attempts) if (item.ownership !== 'stopped') store.recordOwnership(item.id, true);
    store.finishPause(run.id); await work.reconcileStopped(run.id);
    await legacyPromotion(store, work, root, run.id, held.requestId, operation.id, toHead);
    if (movedBeforeRestart) git(owned.path, 'reset', '--keep', toHead);
    const recovered = new Workbench(join(root, 'workflow.sqlite'));
    t.after(() => recovered.close());
    const recoveredWork = new RepositoryWork(recovered, join(root, 'owned'));
    const recoveredEvidence = new Evidence(recovered, recoveredWork, join(root, 'artifacts'));
    const recoveredPublication = new Publication(recovered, recoveredWork, recoveredEvidence, join(root, 'publication-recovery'), [f.connection]);
    t.after(() => recoveredPublication.close());
    const recovery = new TargetRefresh(recovered, recoveredWork, recoveredEvidence, recoveredPublication);
    await recovery.reconcile();
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), movedBeforeRestart ? toHead : candidate.revision.head);
    assert.equal(recovered.repositoryWork(run.id)!.base, candidate.revision.base);
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'conflicted');
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'promoting');
    assert.equal(recovered.codingRecord(run.id)!.completions!.find(item => item.id === operation.id)!.state, 'requested');
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
    assert.equal(existsSync(held.resolutionWorkspace!.path), true);
    await assert.rejects(recoveredWork.reconcileRefresh(run.id), /Checked Git promotion identity is unavailable/);
    await assert.rejects(new Coding(recovered, recoveredWork, recoveredEvidence).complete(run.id, operation.id), /Confirm stopped ownership/);
  }
});

test('M6.3: a refresh interrupted mid-rebase is undone, and one interrupted after Git completes is finished exactly once', async t => {
  const f = await setup(t, 'crewroom-refresh-crash-');
  const { store, run, work, owned, git, candidate } = f;
  f.stop();
  // Crash while Git is still rebasing: the conflict leaves a rebase in progress with the intent recorded.
  const conflicting = f.advance('answer', 'different\n');
  git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:refs/crewroom/${run.id}/target`);
  const ordinaryId = randomUUID();
  store.beginTargetRefresh(run.id, { requestId: ordinaryId, at: new Date().toISOString(), fromBase: candidate.revision.base, fromHead: candidate.revision.head, targetRef: `refs/crewroom/${run.id}/target`, toBase: conflicting, sourceCandidateId: candidate.id, state: 'rebasing' });
  assert.throws(() => git(owned.path, 'rebase', '--onto', conflicting, candidate.revision.base));
  store.recordTargetRebaseRecovery(run.id, ordinaryId, 'rebase_stopped');
  await f.refresh.reconcile();
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head); assert.equal(store.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'interrupted');
  assert.equal(store.repositoryWork(run.id)!.base, candidate.revision.base);

  // Crash after Git rebased but before the outcome was saved: recovery adds its candidate once; a distinct unchanged request remains a no-op.
  f.rewind(candidate.revision.base);
  const clean = f.advance('unrelated', 'target change\n');
  git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:refs/crewroom/${run.id}/target`);
  const interrupted = randomUUID();
  store.beginTargetRefresh(run.id, { requestId: interrupted, at: new Date().toISOString(), fromBase: candidate.revision.base, fromHead: candidate.revision.head, targetRef: `refs/crewroom/${run.id}/target`, toBase: clean, sourceCandidateId: candidate.id, state: 'rebasing' });
  git(owned.path, '-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', 'rebase', '-q', '--onto', clean, candidate.revision.base);
  store.recordTargetRebaseRecovery(run.id, interrupted, 'rebase_stopped');
  await f.refresh.reconcile();
  const saved = store.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === interrupted)!;
  assert.equal(saved.state, 'completed'); assert.equal(store.repositoryWork(run.id)!.base, clean); assert.equal(saved.candidateId, undefined);
  const result = await f.refresh.refresh(run.id, { requestId: randomUUID() });
  assert.equal(result.changed, false); assert.equal(store.repositoryWork(run.id)!.refreshes!.find(item => item.requestId === interrupted)?.candidateId, store.codingRecord(run.id)!.candidates.at(-1)?.id);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 2);
  assert.equal(store.codingRecord(run.id)!.candidates.at(-1)!.revision.head, git(owned.path, 'rev-parse', 'HEAD'));
  assert.equal((await f.refresh.refresh(run.id, { requestId: randomUUID() })).changed, false);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 2);
  assert.equal(Object.values((await work.status(run.id)).drift).some(Boolean), false);
  // A lost reply for that exact in-progress request reconciles Git before returning its own new candidate.
  const source = store.codingRecord(run.id)!.candidates.at(-1)!;
  const nextTarget = f.advance('another', 'next target change\n'); const targetRef = `refs/crewroom/${run.id}/target`;
  git(owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${targetRef}`);
  const replayId = randomUUID();
  store.beginTargetRefresh(run.id, { requestId: replayId, at: new Date().toISOString(), fromBase: clean, fromHead: source.revision.head,
    targetRef, toBase: nextTarget, sourceCandidateId: source.id, state: 'rebasing' });
  git(owned.path, '-c', 'user.name=Crewroom', '-c', 'user.email=crewroom@localhost', 'rebase', '-q', '--onto', nextTarget, clean);
  store.recordTargetRebaseRecovery(run.id, replayId, 'rebase_stopped');
  const recovered = await f.refresh.refresh(run.id, { requestId: replayId });
  assert.equal(recovered.changed, true); assert.equal(recovered.refresh?.state, 'completed');
  assert.equal(recovered.refresh?.candidateId, recovered.candidateId);
  assert.equal(store.codingRecord(run.id)!.candidates.length, 3);
  assert.equal((await f.refresh.refresh(run.id, { requestId: replayId })).refresh?.candidateId, recovered.refresh?.candidateId);
});

test('M6.3: a live ordinary rebase cannot be undone or terminalized by restarted recovery', async t => {
  const f = await setup(t, 'crewroom-refresh-live-child-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); f.advance('unrelated', 'new target\n');
  const latch = gitLatch(f.root, 'rebase'); t.after(() => latch.restore()); latch.arm();
  const requestId = randomUUID(); const running = f.refresh.refresh(run.id, { requestId });
  await latch.blocked();
  try {
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const work = new RepositoryWork(recovered, join(f.root, 'owned'));
    const evidence = new Evidence(recovered, work, join(f.root, 'artifacts'));
    const before = recovered.repositoryWork(run.id)!;
    assert.equal(before.refreshes!.at(-1)!.state, 'rebasing');
    await new TargetRefresh(recovered, work, evidence).reconcile();
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'rebasing');
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal(recovered.repositoryWork(run.id)!.base, candidate.revision.base);
    assert.equal(recovered.repositoryWork(run.id)!.checkpoints.length, before.checkpoints.length);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
    assert.equal((await new Results(recovered, work, evidence).read(run.id)).refreshable, false);
    await assert.rejects(new TargetRefresh(recovered, work, evidence).refresh(run.id, { requestId: randomUUID() }), /Reconcile|unproved|ownership/i);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  } finally { latch.release(); await running.catch(() => {}); }
});

test('M6.3: a killed refresh controller cannot reopen its still-live Git child to writers', async t => {
  const f = await setup(t, 'crewroom-refresh-killed-controller-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); const target = f.advance('unrelated', 'new target\n');
  const requestId = randomUUID(); const targetRef = await f.publication.fetchTarget(run.id, requestId);
  store.close();
  const latch = gitLatch(f.root, 'rebase'); t.after(() => latch.restore()); latch.arm();
  writeFileSync(join(f.root, 'git-latch', 'capture'), '');
  const script = `import { Workbench } from './server/workbench.ts'; import { RepositoryWork } from './server/repository-work.ts';
    const [db, root, runId, requestId, ref, candidateId] = JSON.parse(process.env.CREWROOM_TEST_ARGS);
    const store = new Workbench(db); const work = new RepositoryWork(store, root);
    try { await work.refreshBase(runId, requestId, ref, candidateId); } finally { store.close(); }`;
  const child = spawn(process.execPath, ['--import', 'tsx', '--input-type=module', '-e', script], {
    env: { ...process.env, CREWROOM_TEST_ARGS: JSON.stringify([join(f.root, 'workflow.sqlite'), join(f.root, 'owned'), run.id, requestId, targetRef, candidate.id]) },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  let stderr = ''; child.stderr!.on('data', data => { stderr += String(data); });
  const exit = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(resolve => child.once('exit', (code, signal) => resolve({ code, signal })));
  try {
    await latch.blocked();
    assert.equal(child.kill('SIGKILL'), true, stderr);
    assert.equal((await exit).signal, 'SIGKILL');
    assert.doesNotThrow(() => process.kill(latch.pid(), 0), 'The direct Git helper remains alive after its controller died.');
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const work = new RepositoryWork(recovered, join(f.root, 'owned'));
    const evidence = new Evidence(recovered, work, join(f.root, 'artifacts'));
    await new TargetRefresh(recovered, work, evidence).reconcile();
    const held = recovered.repositoryWork(run.id)!;
    assert.equal(held.refreshes!.at(-1)!.state, 'rebasing');
    assert.equal(held.refreshes!.at(-1)!.rebaseRecovery, undefined);
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal(held.base, candidate.revision.base);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
    assert.equal((await new Results(recovered, work, evidence).read(run.id)).refreshable, false);
    await assert.rejects(work.claimWriter(recovered.runtime(run.id).attempts[0].id), /operator decision|repository writes are held/i);
    await assert.rejects(new TargetRefresh(recovered, work, evidence).refresh(run.id, { requestId: randomUUID() }), /unproved|inspection/i);
    const runtime = new RuntimeSupervisor(recovered, { root: join(f.root, 'killed-hosts'), socket: `/tmp/crewroom-${run.id.slice(0, 8)}.sock`, ompBinary: '/unused',
      mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: f.root, retentionSeconds: 86400, atcDisabled: true },
      catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9', () => [], work);
    const register = Messaging.prototype.register; Messaging.prototype.register = async () => {};
    try { await assert.rejects(Promise.resolve().then(() => runtime.resume(run.id)), /Git mutation ownership is unverified/i); }
    finally { Messaging.prototype.register = register; await runtime.close(); }
    assert.equal(recovered.repositoryWork(run.id)!.writer, undefined);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    latch.release();
    try { await latch.finished(); }
    catch { try { process.kill(latch.pid(), 'SIGKILL'); } catch { /* Orphan already exited. */ } throw new Error('Orphan Git did not finish after release.'); }
  }
  assert.notEqual(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(git(owned.path, 'show', 'HEAD:unrelated'), 'new target');
  assert.equal(git(owned.path, 'merge-base', 'HEAD', target), target);
});

test('M6.3: a live isolated rebase leaves preparing uninspectable on restart', async t => {
  const f = await setup(t, 'crewroom-prepare-live-child-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); f.advance('answer', 'different\n');
  const latch = gitLatch(f.root, 'rebase', 2); t.after(() => latch.restore()); latch.arm();
  const requestId = randomUUID(); const running = f.refresh.refresh(run.id, { requestId });
  await latch.blocked();
  try {
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const work = new RepositoryWork(recovered, join(f.root, 'owned'));
    const evidence = new Evidence(recovered, work, join(f.root, 'artifacts'));
    const before = recovered.repositoryWork(run.id)!; const workspace = before.refreshes!.at(-1)!.resolutionWorkspace!;
    assert.equal(workspace.state, 'preparing');
    assert.equal(existsSync(workspace.path), true);
    await new TargetRefresh(recovered, work, evidence).reconcile();
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'preparing');
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'rebasing');
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal((await new Results(recovered, work, evidence).read(run.id)).refreshable, false);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
  } finally { latch.release(); await running.catch(() => {}); }
});

test('M6.3: a killed preparation controller leaves its isolated Git child operator-held', async t => {
  const f = await setup(t, 'crewroom-prepare-killed-controller-');
  const { store, run, owned, git, candidate } = f;
  f.stop(); f.advance('answer', 'different\n');
  const requestId = randomUUID(); const targetRef = await f.publication.fetchTarget(run.id, requestId);
  store.close();
  const latch = gitLatch(f.root, 'rebase', 2); t.after(() => latch.restore()); latch.arm();
  writeFileSync(join(f.root, 'git-latch', 'capture'), '');
  const script = `import { Workbench } from './server/workbench.ts'; import { RepositoryWork } from './server/repository-work.ts';
    const [db, root, runId, id, ref, candidateId] = JSON.parse(process.env.CREWROOM_TEST_ARGS);
    const store = new Workbench(db); const work = new RepositoryWork(store, root);
    try { await work.refreshBase(runId, id, ref, candidateId); } finally { store.close(); }`;
  const child = spawn(process.execPath, ['--import', 'tsx', '--input-type=module', '-e', script], {
    env: { ...process.env, CREWROOM_TEST_ARGS: JSON.stringify([join(f.root, 'workflow.sqlite'), join(f.root, 'owned'), run.id, requestId, targetRef, candidate.id]) },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  let stderr = ''; child.stderr!.on('data', data => { stderr += String(data); });
  const exit = new Promise<NodeJS.Signals | null>(resolve => child.once('exit', (_code, signal) => resolve(signal)));
  let path = '';
  try {
    await latch.blocked();
    assert.equal(child.kill('SIGKILL'), true, stderr);
    assert.equal(await exit, 'SIGKILL');
    assert.doesNotThrow(() => process.kill(latch.pid(), 0));
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const work = new RepositoryWork(recovered, join(f.root, 'owned'));
    const evidence = new Evidence(recovered, work, join(f.root, 'artifacts'));
    const before = recovered.repositoryWork(run.id)!;
    path = before.refreshes!.at(-1)!.resolutionWorkspace!.path;
    assert.equal(before.refreshes!.at(-1)!.resolutionWorkspace?.state, 'preparing');
    await new TargetRefresh(recovered, work, evidence).reconcile();
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.state, 'rebasing');
    assert.equal(recovered.repositoryWork(run.id)!.refreshes!.at(-1)!.resolutionWorkspace?.state, 'preparing');
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal((await new Results(recovered, work, evidence).read(run.id)).refreshable, false);
    await assert.rejects(work.readFile(run.id, 'answer'), /unverified|inspection/i);
    await assert.rejects(new TargetRefresh(recovered, work, evidence).refresh(run.id, { requestId: randomUUID() }), /unproved|inspect/i);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    latch.release();
    try { await latch.finished(); }
    catch { try { process.kill(latch.pid(), 'SIGKILL'); } catch { /* Already exited. */ } throw new Error('Orphan isolated Git did not finish.'); }
  }
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  assert.equal(readFileSync(join(f.root, 'git-latch', 'git.exit'), 'utf8').trim(), '1', readFileSync(join(f.root, 'git-latch', 'git.stderr'), 'utf8'));
  assert.equal(git(path, 'diff', '--name-only', '--diff-filter=U'), 'answer');
  assert.equal(readFileSync(join(path, 'answer'), 'utf8').includes('different'), true);
});

test('M6.3: a killed conflict resolver cannot reopen an orphan Git add as an editable workspace', async t => {
  const f = await setup(t, 'crewroom-continue-killed-controller-');
  const { store, run, work, evidence, owned, git, candidate } = f;
  f.stop(); store.checkpointRepository(run.id, { id: randomUUID(), at: new Date().toISOString(), reason: 'completed', revision: candidate.revision });
  f.advance('answer', 'different\n');
  const refresh = (await f.refresh.refresh(run.id, { requestId: randomUUID() })).refresh!;
  const resumed = f.resume(); const coding = new Coding(store, work, evidence);
  await coding.claim(resumed.Coordinator, 'orphan-continue-claim', { assignee: 'Writer', instruction: 'Resolve answer only.' });
  const file = await work.readConflict(run.id, 'answer');
  const resolved = 'correct and different\n';
  await coding.edit(resumed.Writer, 'orphan-continue-edit', { path: 'answer', expectedHash: file.sha256, content: resolved });
  const workspace = refresh.resolutionWorkspace!.path;
  store.close();
  const latch = gitLatch(f.root, 'add'); t.after(() => latch.restore()); latch.arm();
  writeFileSync(join(f.root, 'git-latch', 'capture'), '');
  const script = `import { Workbench } from './server/workbench.ts'; import { RepositoryWork } from './server/repository-work.ts'; import { Coding } from './server/coding.ts';
    const [db, root, attemptId, requestId] = JSON.parse(process.env.CREWROOM_TEST_ARGS);
    const store = new Workbench(db); const work = new RepositoryWork(store, root);
    try { await new Coding(store, work).continueTargetConflict(attemptId, 'orphan-continuation', { requestId }); } finally { store.close(); }`;
  const child = spawn(process.execPath, ['--import', 'tsx', '--input-type=module', '-e', script], {
    env: { ...process.env, CREWROOM_TEST_ARGS: JSON.stringify([join(f.root, 'workflow.sqlite'), join(f.root, 'owned'), resumed.Writer, refresh.requestId]) },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  let stderr = ''; child.stderr!.on('data', data => { stderr += String(data); });
  const exit = new Promise<NodeJS.Signals | null>(resolve => child.once('exit', (_code, signal) => resolve(signal)));
  try {
    await latch.blocked();
    assert.equal(child.kill('SIGKILL'), true, stderr);
    assert.equal(await exit, 'SIGKILL');
    assert.doesNotThrow(() => process.kill(latch.pid(), 0));
    const recovered = new Workbench(join(f.root, 'workflow.sqlite')); t.after(() => recovered.close());
    const recoveredWork = new RepositoryWork(recovered, join(f.root, 'owned'));
    const recoveredEvidence = new Evidence(recovered, recoveredWork, join(f.root, 'artifacts'));
    const active = recovered.repositoryWork(run.id)!.refreshes!.at(-1)!;
    assert.equal(active.resolutionWorkspace?.state, 'continuing');
    assert.equal(active.resolutionWorkspace?.continuation?.toolCallId, 'orphan-continuation');
    await new TargetRefresh(recovered, recoveredWork, recoveredEvidence).reconcile();
    assert.equal(recovered.getRun(run.id).status, 'attention_required');
    assert.equal(recoveredWork.activeConflict(run.id), undefined);
    await assert.rejects(recoveredWork.readFile(run.id, 'answer'), /unverified|inspection/i);
    await assert.rejects(new Coding(recovered, recoveredWork).continueTargetConflict(resumed.Writer, 'orphan-continuation', { requestId: refresh.requestId }), /current writer|continuing|unverified/i);
    await assert.rejects(new TargetRefresh(recovered, recoveredWork, recoveredEvidence).refresh(run.id, { requestId: randomUUID() }), /unverified|inspect/i);
    assert.equal((await new Results(recovered, recoveredWork, recoveredEvidence).read(run.id)).refreshable, false);
    assert.equal(recovered.codingRecord(run.id)!.candidates.length, 1);
    assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    latch.release();
    try { await latch.finished(); }
    catch { try { process.kill(latch.pid(), 'SIGKILL'); } catch { /* Already exited. */ } throw new Error('Orphan conflict Git did not finish.'); }
  }
  assert.equal(readFileSync(join(f.root, 'git-latch', 'git.exit'), 'utf8').trim(), '0', readFileSync(join(f.root, 'git-latch', 'git.stderr'), 'utf8'));
  assert.equal(git(workspace, 'diff', '--cached', '--name-only'), 'answer');
  assert.equal(readFileSync(join(workspace, 'answer'), 'utf8'), resolved);
  assert.equal(git(owned.path, 'rev-parse', 'HEAD'), candidate.revision.head);
});
