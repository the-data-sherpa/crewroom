import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { saveRepositoryFixture } from './fixtures/repository';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { Research } from '../server/research';
import { JiraClient } from '../server/jira-client';
import { JiraDelivery } from '../server/jira-delivery';
import { defaultRoles } from '../server/roles';
import type { Role } from '../shared/types';
import type { AssignmentModel } from '../server/runtime/model-policy';
import { resolveAssignment } from '../server/runtime/model-policy';
import { snapshotBriefs } from '../server/runtime/brief';
import { RuntimeSupervisor } from '../server/runtime/supervisor';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-research-')); const repository = join(root, 'source'); mkdirSync(repository);
  execFileSync('git', ['init', '-b', 'main', repository]);
  writeFileSync(join(repository, 'queue.md'), 'Design A: FIFO order\nDesign B: weighted priority\nTradeoff: starvation risk\n');
  execFileSync('git', ['-C', repository, 'add', 'queue.md']);
  execFileSync('git', ['-C', repository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-m', 'Queue examples']);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const source = new RepositoryWork(store, join(root, 'worktrees'));
  const evidence = new Evidence(store, source, join(root, 'artifacts'));
  const research = new Research(store, source, evidence);
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  return { root, repository, store, source, research, evidence, role, model };
}

function claim(store: Workbench, runId: string, name: string, role: Role, model: AssignmentModel, coordinator: boolean) {
  const assigned = store.admitAssignment(runId, { name, role, model, coordinator, input: `Assess ${name}`, brief: { criteria: [], dependsOn: [] } });
  store.claimAttempt(assigned.attempt.id);
  store.recordAttemptIdentity(assigned.attempt.id, { hostPid: 100, ompPid: 101, sessionId: `${name}-session`, hostUrl: 'http://127.0.0.1:9', pane: '%1' });
  return assigned;
}

test('INT-07/EVD-08: pinned research rejects unsupported source and coverage, accepts corrected artifacts without coding gates', async t => {
  const { root, repository, store, source, research, role, model } = fixture(t);
  const repo = saveRepositoryFixture(store, { name: 'Queue fixture', path: repository, targetBranch: 'main' });
  const task = store.createTask({ title: 'Compare queue designs', description: '', repositoryId: repo.id });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Assess FIFO and priority', criteria: ['Compare designs', 'Name the decision risk'] });
  const pinned = await source.pinResearch(repo);
  const run = store.queueResearch(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() }, pinned);
  assert.equal(store.queueResearch(task.id, { revision: accepted.revision, requestId: run.requestId }, pinned).id, run.id);
  await assert.rejects(source.prepare(run.id, { mode: 'new' }), /cannot prepare repository work/);
  assert.equal(store.repositoryWork(run.id), undefined);
  assert.throws(() => snapshotBriefs([{ name: 'Architect', criteria: [{ index: 0, obligation: 'Compare source designs.', checkIds: ['unit'] }], dependsOn: [] }], { criteria: run.taskSnapshot.criteria, checkIds: [], existing: [] }), /check unit is not configured/);
  assert.equal(store.codingRecord(run.id), undefined);
  assert.equal(store.publication(run.id), undefined);
  writeFileSync(join(repository, 'queue.md'), 'A later target revision omits the original design excerpts.\n');
  execFileSync('git', ['-C', repository, 'add', 'queue.md']);
  execFileSync('git', ['-C', repository, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '-m', 'Advance target branch']);
  assert.notEqual(execFileSync('git', ['-C', repository, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(), pinned.commit);
  assert.equal((await source.readResearch(run.id, 'queue.md')).content.split('\n')[1], 'Design B: weighted priority');
  assert.equal((await source.searchResearch(run.id, { query: 'starvation' })).matches[0].line, 3);
  const codingTask = store.createTask({ title: 'Independent coding task', description: '', repositoryId: repo.id });
  const codingBrief = store.refine(codingTask.id, { revision: 1, sourceRevision: 1, brief: 'Implement separately', criteria: ['Change one file'] });
  const codingRun = store.queue(codingTask.id, { revision: codingBrief.revision, requestId: crypto.randomUUID() });
  await source.prepare(codingRun.id, { mode: 'new' });
  const coder = claim(store, codingRun.id, 'CodingCoordinator', role, model, true);
  store.setDesiredState(codingRun.id, 'running'); store.startExecution(codingRun.id);
  const coordinator = claim(store, run.id, 'Coordinator', role, model, true);
  const specialist = claim(store, run.id, 'Architect', { ...role, id: 'principal-architect' }, model, false);
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  assert.deepEqual(store.executingRuns().sort((a, b) => a.runId.localeCompare(b.runId)), [{ runId: codingRun.id, coding: true }, { runId: run.id, coding: false }].sort((a, b) => a.runId.localeCompare(b.runId)));
  const citation = { path: 'queue.md', commit: pinned.commit, startLine: 1, endLine: 2, excerpt: 'Design A: FIFO order\nDesign B: weighted priority' };
  const input = { title: 'Queue comparison', kind: 'design', body: '# Comparison\nFIFO is simple, priority risks starvation.\n## Risk\nDocument the tradeoff.', citations: [citation], limitations: 'Only fictional source designs were compared.', coverage: [{ criterionId: 0, section: 'Comparison' }] } as const;
  const valid = await research.submit(specialist.attempt.id, 'valid', input);
  assert.equal(valid.state, 'verified'); assert.equal(valid.citations[0].verified, true); assert.equal(valid.body.sha256.length, 64);
  assert.equal((await research.submit(specialist.attempt.id, 'valid', input)).id, valid.id);
  await assert.rejects(research.submit(specialist.attempt.id, 'valid', { ...input, body: '# Changed' }), /different content/);
  const invalidCases = [
    [{ ...citation, path: 'missing.md' }, /does not exist/],
    [{ ...citation, startLine: 40, endLine: 42 }, /outside/],
    [{ ...citation, excerpt: 'Fabricated quote' }, /does not match/],
    [{ ...citation, commit: '0'.repeat(40) }, /not the pinned commit/],
  ] as const;
  for (const [bad, reason] of invalidCases) {
    const record = await research.submit(specialist.attempt.id, crypto.randomUUID(), { ...input, title: `Bad ${bad.path} ${bad.startLine} ${bad.commit} ${bad.excerpt}`, citations: [bad] });
    assert.equal(record.state, 'rejected'); assert.match(record.reasons.join(' '), reason); assert.equal(record.citations[0].verified, false);
  }
  for (const [entry, reason] of [[{ criterionId: 0, section: 'Missing heading' }, /heading/], [{ criterionId: 99, section: 'Comparison' }, /not in accepted revision/]] as const) {
    const invalid = await research.submit(specialist.attempt.id, crypto.randomUUID(), { ...input, title: `Invalid criterion ${entry.criterionId} ${entry.section}`, coverage: [entry] });
    assert.equal(invalid.state, 'rejected'); assert.match(invalid.reasons.join(' '), reason);
  }
  const missing = await research.submit(specialist.attempt.id, 'missing', { title: 'Missing evidence', kind: 'design', body: input.body, citations: [citation] });
  assert.equal(missing.state, 'rejected'); assert.match(missing.reasons.join(' '), /Limitations.*Criterion coverage/s);
  assert.deepEqual(research.read(run.id).assessment.covered, [0]);
  assert.throws(() => research.requestCompletion(coordinator.attempt.id, 'premature', {}), /Criterion 1/);
  const corrected = await research.submit(specialist.attempt.id, 'corrected', { ...input, title: 'Missing evidence', coverage: [{ criterionId: 1, section: 'Risk' }] });
  assert.equal(corrected.state, 'verified');
  assert.equal(store.researchArtifacts(run.id).find(item => item.id === missing.id)?.supersededBy, corrected.id);
  assert.equal(research.read(run.id).assessment.ready, true);
  const requested = research.requestCompletion(coordinator.attempt.id, 'finish', {});
  for (const item of [coordinator, specialist]) store.recordOwnership(item.attempt.id, true);
  store.beginPause(run.id); store.finishPause(run.id);
  const late = store.steer(run.id, { requestId: 'late-before-completion', body: 'Undelivered direction: investigate an unrelated queue design.' });
  const insertedRole = { ...role, id: 'tech-writer' };
  const insertion = store.proposeOperatorSpecialist(run.id, { requestId: crypto.randomUUID(), roleId: insertedRole.id, input: 'Explain the decision risk.', criteria: [1] }, insertedRole, model);
  assert.match(research.read(run.id).assessment.blockers.join(' '), /insertion awaits/);
  assert.throws(() => store.completeResearch(run.id, requested.requestId, requested.revision), /inserted specialist/);
  store.rejectUnavailableTeamProposal(insertion.id, [{ ...insertedRole, enabled: false }]);
  assert.equal(research.read(run.id).assessment.ready, true, 'An unapplied rejection leaves the verified research intact.');
  store.completeResearch(run.id, requested.requestId, requested.revision);
  assert.throws(() => store.proposeOperatorSpecialist(run.id, { requestId: crypto.randomUUID(), roleId: insertedRole.id, input: 'Too late.', criteria: [1] }, insertedRole, model), /has ended/);
  assert.equal(store.steering(run.id).find(message => message.id === late.id)?.delivery, 'not_delivered');
  assert.equal(research.read(run.id).assessment.completed, true);
  assert.equal(store.getRun(run.id).status, 'completed');
  assert.equal(store.getAttempt(coder.attempt.id).ownership, 'held', 'Research completion leaves a separate coding run executing.');
  assert.equal(store.repositoryWork(run.id), undefined); assert.equal(store.codingRecord(run.id), undefined); assert.equal(store.publication(run.id), undefined);
  const newRequirements = store.refine(task.id, { revision: 2, sourceRevision: 1, brief: 'Assess FIFO with fairness', criteria: ['Compare designs', 'Name the decision risk', 'Recommend a fairness mitigation'] });
  store.adoptRequirements(run.id, { revision: newRequirements.revision, requestId: crypto.randomUUID(), reason: 'Added fairness criteria.' });
  // STE-04: terminal steering stays in operator history, never sneaking into a reopened agent prompt.
  const runtime = new RuntimeSupervisor(store, { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:9');
  const context = (runtime as unknown as { continuationContext: (id: string) => { steering: { id: string; delivery: string; body?: string; dispositionReason?: string }[] } }).continuationContext(run.id);
  const historical = context.steering.find(message => message.id === late.id)!;
  assert.equal(historical.delivery, 'not_delivered'); assert.equal(historical.body, undefined);
  assert.match(historical.dispositionReason!, /Research completed before delivery/);
  assert.equal(JSON.stringify(context).includes(late.body), false);
  assert.equal(store.steering(run.id).find(message => message.id === late.id)?.body, late.body);
  assert.ok(context.steering.some(message => message.delivery === 'pending' && message.body?.includes('Operator adopted')));
  const after = research.read(run.id);
  assert.equal(after.assessment.completed, false); assert.deepEqual(after.assessment.covered, []); assert.equal(after.artifacts.length, 9);
  assert.match(after.assessment.blockers.join(' '), /Criterion 2/);
});

test('DEL-01/ROL-06: only a completed current research outcome yields one summary comment and no transition', async t => {
  const { repository, store, source, research, role, model, evidence } = fixture(t);
  const site = 'https://fixture.atlassian.net';
  const task = store.importJira({ siteUrl: site, issues: [{ id: '100', key: 'SIM-100', fields: { summary: 'Queue assessment', description: 'Original criteria', status: { id: '1', name: 'To Do' } } }] })[0];
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: repository, targetBranch: 'main' });
  store.assignRepository(task.id, { revision: 1, repositoryId: repo.id });
  const accepted = store.refine(task.id, { revision: 2, sourceRevision: 1, brief: 'Compare options', criteria: ['Compare queue designs'] });
  const pinned = await source.pinResearch(repo);
  const run = store.queueResearch(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() }, pinned);
  const coordinator = claim(store, run.id, 'Coordinator', role, model, true);
  const reporter = claim(store, run.id, 'Reporter', { ...role, id: 'delivery-reporter' }, model, false);
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const requests: string[] = []; const comments: { id: string; body: unknown }[] = [];
  const http = createServer(async (req, res) => {
    requests.push(`${req.method} ${req.url}`);
    if (req.method === 'POST') {
      let body = ''; for await (const chunk of req) body += chunk;
      comments.push({ id: '77', body: JSON.parse(body) }); res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify({ id: '77' })); return;
    }
    res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify({ comments, total: comments.length, startAt: 0, maxResults: 100 }));
  });
  await new Promise<void>(resolve => http.listen(0, '127.0.0.1', resolve));
  t.after(async () => { await new Promise<void>(resolve => http.close(() => resolve())); });
  const address = http.address(); assert.ok(address && typeof address !== 'string');
  const client = new JiraClient({ source: { siteUrl: site, cloudId: '00000000-0000-4000-8000-000000000022', filterId: '1', assigneeAccountId: 'operator', name: 'Fixture' }, apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'fixture' });
  const delivery = new JiraDelivery(store, client, evidence, research);
  await assert.rejects(delivery.prepare(reporter.attempt.id, { kind: 'research_outcome', eventKey: `research-outcome-${run.taskRevision}` }), /requested, fully verified research outcome/);
  await assert.rejects(delivery.prepare(reporter.attempt.id, { kind: 'in_progress', eventKey: 'execution-started' }), /Research does not report/);
  await research.submit(coordinator.attempt.id, 'assessment', { title: 'Decision', kind: 'research', body: '# Comparison\nFIFO vs priority.',
    citations: [{ path: 'queue.md', commit: pinned.commit, startLine: 1, endLine: 1, excerpt: 'Design A: FIFO order' }],
    limitations: 'Fictional alternatives only.', coverage: [{ criterionId: 0, section: 'Comparison' }] });
  const completion = research.requestCompletion(coordinator.attempt.id, 'complete', {});
  const outcomeInput = { kind: 'research_outcome', eventKey: `research-outcome-${run.taskRevision}` };
  await assert.rejects(delivery.prepare(coordinator.attempt.id, outcomeInput), /delivery reporter/);
  const update = await delivery.prepare(reporter.attempt.id, outcomeInput);
  assert.equal(update.attemptId, reporter.attempt.id);
  assert.equal((await delivery.prepare(reporter.attempt.id, outcomeInput)).id, update.id);
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'pending'); assert.equal(comments.length, 0);
  for (const item of [coordinator, reporter]) store.recordOwnership(item.attempt.id, true);
  store.beginPause(run.id); store.finishPause(run.id); store.completeResearch(run.id, completion.requestId, completion.revision);
  await delivery.flush(); await delivery.flush();
  assert.equal(delivery.read(run.id)[0].state, 'confirmed'); assert.equal(comments.length, 1);
  assert.match(JSON.stringify(comments[0]), /Fictional alternatives only/); assert.match(JSON.stringify(comments[0]), /SHA-256/);
  assert.equal(requests.some(item => item.includes('transitions')), false);
  assert.equal(store.getTask(task.id).description, 'Original criteria');
});
