import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { Research } from '../server/research';
import { JiraClient } from '../server/jira-client';
import { JiraDelivery } from '../server/jira-delivery';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-review-repair-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const work = new RepositoryWork(store, join(root, 'worktrees'));
  const evidence = new Evidence(store, work, join(root, 'artifacts'));
  const research = new Research(store, work, evidence);
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const task = store.importJira({ siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '100', key: 'SIM-100', fields: { summary: 'Compare queues', description: 'Fictional designs', status: { id: '1', name: 'To Do' } } }] })[0];
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Compare both designs', criteria: ['Contrast FIFO and priority'] });
  const run = store.queueResearch(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() });
  const admit = (name: string, reporter: boolean) => {
    const assigned = store.admitAssignment(run.id, { name, role: reporter ? { ...role, id: 'delivery-reporter' } : role, model, coordinator: !reporter, input: 'Assess the queue.' });
    store.claimAttempt(assigned.attempt.id);
    store.recordAttemptIdentity(assigned.attempt.id, { hostPid: 101, ompPid: 102, sessionId: name, hostUrl: 'http://127.0.0.1:9', pane: '%1' });
    return assigned;
  };
  const coordinator = admit('Coordinator', false); const reporter = admit('Reporter', true);
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  return { root, store, research, evidence, task, run, coordinator, reporter };
}

test('DEL-01 review repair: a superseded blocked outcome releases the next revision comment lane', async t => {
  const { store, research, evidence, task, run, coordinator, reporter } = fixture(t);
  const comments: { id: string; body: unknown }[] = []; const posts: string[] = [];
  const server = createServer(async (req, res) => {
    if (req.method === 'POST') {
      posts.push(req.url!); let body = ''; for await (const chunk of req) body += chunk;
      comments.push({ id: String(comments.length + 1), body: JSON.parse(body).body });
      res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify({ id: String(comments.length) })); return;
    }
    res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify({ comments, startAt: 0, total: comments.length }));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => { await new Promise<void>(resolve => server.close(() => resolve())); });
  const address = server.address(); assert.ok(address && typeof address !== 'string');
  const delivery = new JiraDelivery(store, new JiraClient({ source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '1', assigneeAccountId: 'operator', name: 'Fixture' }, apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'fixture' }), evidence, research);
  await research.submit(coordinator.attempt.id, 'first', { title: 'Initial assessment', kind: 'research', body: '# First\nCompare designs.', citations: [], limitations: 'Fixture only.', coverage: [{ criterionId: 0, section: 'First' }] });
  research.requestCompletion(coordinator.attempt.id, 'initial', {});
  await delivery.prepare(reporter.attempt.id, { kind: 'research_outcome', eventKey: `research-outcome-${run.taskRevision}` });
  const revised = store.refine(task.id, { revision: store.getTask(task.id).revision, sourceRevision: 1, brief: 'Compare both and risks', criteria: ['Contrast FIFO and priority', 'Name starvation risk'] });
  store.adoptRequirements(run.id, { revision: revised.revision, requestId: crypto.randomUUID(), reason: 'Added risk criterion.' });
  for (const item of [coordinator, reporter]) store.recordOwnership(item.attempt.id, true);
  store.finishPause(run.id);
  store.setDesiredState(run.id, 'running');
  const next = [coordinator, reporter].map(item => store.replaceAttempt(item.assignment.id, { automatic: false }));
  store.prepareContinuation(run.id, store.getRun(run.id).controlRevision!);
  for (const [index, attempt] of next.entries()) {
    store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 201 + index, ompPid: 301 + index, sessionId: `replacement-${index}`, hostUrl: 'http://127.0.0.1:9', pane: `%${index + 2}` });
  }
  store.startExecution(run.id);
  await research.submit(next[0].id, 'replacement', { title: 'Revised assessment', kind: 'design', body: '# Both designs\nCompared.\n# Risk\nStarvation.', citations: [], limitations: 'Fictional source only.', coverage: [{ criterionId: 0, section: 'Both designs' }, { criterionId: 1, section: 'Risk' }] });
  const completion = research.requestCompletion(next[0].id, 'new-outcome', {});
  await delivery.prepare(next[1].id, { kind: 'research_outcome', eventKey: `research-outcome-${revised.revision}` });
  for (const attempt of next) store.recordOwnership(attempt.id, true);
  store.beginPause(run.id); store.finishPause(run.id); store.completeResearch(run.id, completion.requestId, completion.revision);
  await delivery.flush(); await delivery.flush();
  const updates = delivery.read(run.id);
  assert.deepEqual(updates.map(item => [item.eventKey, item.state]), [[`research-outcome-${run.taskRevision}`, 'blocked'], [`research-outcome-${revised.revision}`, 'confirmed']]);
  assert.equal(posts.length, 1); assert.equal(comments.length, 1);
  assert.match(JSON.stringify(comments[0]), /Revised assessment/);
});

test('EVD-08 review repair: empty adoption and rejected-only research cannot complete', async t => {
  const { root, store, research, task, run, coordinator, reporter } = fixture(t);
  const rejected = await research.submit(coordinator.attempt.id, 'unsupported', { title: 'Unsupported', kind: 'research', body: '# First\nNo cited source.', citations: [], limitations: '', coverage: [] });
  assert.equal(rejected.state, 'rejected');
  const empty = store.refine(task.id, { revision: store.getTask(task.id).revision, sourceRevision: 1, brief: '', criteria: [] });
  assert.throws(() => store.adoptRequirements(run.id, { revision: empty.revision, requestId: crypto.randomUUID(), reason: 'Empty scope.' }), /brief and acceptance criteria/);
  const requested = store.requestResearchCompletion(run.id, coordinator.attempt.id, 'attempt-to-bypass-gate');
  for (const item of [coordinator, reporter]) store.recordOwnership(item.attempt.id, true);
  store.beginPause(run.id); store.finishPause(run.id);
  assert.throws(() => store.completeResearch(run.id, requested.requestId, requested.revision), /verified.*current/i);
  assert.equal(store.getRun(run.id).status, 'paused');
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const codingTask = store.createTask({ title: 'Separate coding work', description: '', repositoryId: repo.id });
  const codingBrief = store.refine(codingTask.id, { revision: 1, sourceRevision: 1, brief: 'Write a change', criteria: ['Change is reviewed'] });
  const coding = store.queue(codingTask.id, { revision: codingBrief.revision, requestId: crypto.randomUUID() });
  const codingEmpty = store.refine(codingTask.id, { revision: codingBrief.revision, sourceRevision: 1, brief: '', criteria: [] });
  assert.throws(() => store.adoptRequirements(coding.id, { revision: codingEmpty.revision, requestId: crypto.randomUUID() }), /brief and acceptance criteria/);
});
