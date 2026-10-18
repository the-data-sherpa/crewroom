import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { JiraClient } from '../server/jira-client';
import { JiraDelivery } from '../server/jira-delivery';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

const source = { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator-1', name: 'Assigned work', inProgressStatusId: '3', inProgressTransitionId: '6' };
function admitted(store: Workbench, root: string, includeWriter = false) {
  const task = store.importJira({ siteUrl: source.siteUrl, issues: [{ id: '100', key: 'SIM-1', fields: { summary: 'Fix scrolling', description: 'Keep buttons visible', status: { id: '1', name: 'To Do' } } }] })[0];
  seedRepository(root);
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  store.assignRepository(task.id, { revision: 1, repositoryId: repo.id }); store.refine(task.id, { revision: 2, sourceRevision: 1, brief: 'Constrain the modal', criteria: ['Actions stay accessible'] });
  const run = store.queue(task.id, { revision: 3 });
  const role = { ...defaultRoles.find(role => role.id === 'delivery-reporter')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const reporter = store.admitAssignment(run.id, { name: 'Reporter', role, model, coordinator: false, input: 'Report progress' });
  const other = includeWriter ? store.admitAssignment(run.id, { name: 'Writer', role: { ...role, id: 'implementer' }, model, coordinator: false, input: 'Implement' }) : undefined;
  if (other) { store.claimAttempt(other.attempt.id); store.recordAttemptIdentity(other.attempt.id, { hostPid: 3, ompPid: 4, sessionId: 'writer-session', hostUrl: 'http://127.0.0.1:5', pane: '%2' }); }
  store.claimAttempt(reporter.attempt.id); store.recordAttemptIdentity(reporter.attempt.id, { hostPid: 1, ompPid: 2, sessionId: 'fixture-session', hostUrl: 'http://127.0.0.1:3', pane: '%1' });
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  return { run, reporter, task, other };
}

test('DEL-01/02/03: reporter updates survive an outage and a lost create reply without duplicate comments', async t => {
  let offline = true, posts = 0; const comments: any[] = [];
  const provider = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture');
    assert.equal(url.pathname, '/rest/api/3/issue/100/comment');
    if (offline) { res.statusCode = 503; res.end('private provider detail'); return; }
    if (req.method === 'POST') {
      const parts: Buffer[] = []; for await (const part of req) parts.push(part);
      comments.push({ ...JSON.parse(Buffer.concat(parts).toString()), id: '77' }); posts++;
      req.socket.destroy(); return;
    }
    res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify({ comments, total: comments.length, startAt: 0, maxResults: 100 }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-delivery-')); const path = join(root, 'db'); let store = new Workbench(path);
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const client = new JiraClient({ source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' });
  let delivery = new JiraDelivery(store, client); const { run, reporter } = admitted(store, root);
  const input = { kind: 'milestone', eventKey: 'impact-reviewed', body: 'The modal overflow cause is identified.' };
  const action = (await delivery.prepare(reporter.attempt.id, input));
  assert.equal(action.state, 'pending'); assert.equal(posts, 0);
  assert.equal((await delivery.prepare(reporter.attempt.id, input)).id, action.id);
  await assert.rejects(async () => (await delivery.prepare(reporter.attempt.id, { ...input, body: 'Different request' })), /different content/);
  await assert.rejects(async () => (await delivery.prepare(reporter.attempt.id, { ...input, description: 'Rewrite Jira requirements' })), /Unrecognized/i);
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'pending'); assert.equal(posts, 0);
  offline = false; await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'uncertain'); assert.equal(posts, 1);
  store.close(); store = new Workbench(path); delivery = new JiraDelivery(store, client);
  await delivery.flush();
  assert.equal(delivery.read(run.id)[0].state, 'confirmed'); assert.equal(delivery.read(run.id)[0].externalId, '77');
  await delivery.flush(); assert.equal(posts, 1); assert.equal(comments.length, 1);
  assert.equal(store.getTask(run.taskId).description, 'Keep buttons visible');
});

test('DEL-01/07: only the admitted reporter may request the configured progress transition; Jira status remains an observed fact', async t => {
  let remoteStatus = '1', writes = 0, permissionDenied = false;
  const provider = createServer(async (req, res) => {
    res.setHeader('Content-Type', 'application/json');
    if (permissionDenied) { res.statusCode = 403; res.end('private permission details'); return; }
    if (req.method === 'POST') {
      let body = ''; for await (const chunk of req) body += chunk;
      assert.deepEqual(JSON.parse(body), { transition: { id: '6' } });
      writes++; remoteStatus = '3'; req.socket.destroy(); return;
    }
    res.end(JSON.stringify(req.url!.includes('transitions') ? { transitions: [{ id: '6', to: { id: '3' }, fields: {} }] }
      : { id: '100', key: 'SIM-1', fields: { summary: 'Fix scrolling', status: { id: remoteStatus, name: remoteStatus === '1' ? 'To Do' : 'In Progress' } } }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-transition-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const delivery = new JiraDelivery(store, new JiraClient({ source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }));
  const { run, reporter, other } = admitted(store, root, true);
  await assert.rejects(async () => (await delivery.prepare(other!.attempt.id, { kind: 'in_progress', eventKey: 'execution-started' })), /reporter/);
  await assert.rejects(async () => (await delivery.prepare(reporter.attempt.id, { kind: 'in_progress', eventKey: 'execution-started', transitionId: '2' })), /Unrecognized/);
  await assert.rejects(async () => (await delivery.prepare(reporter.attempt.id, { kind: 'evidence', eventKey: 'evidence-ready', candidateId: run.id })), /evidence is unavailable/i);
  (await delivery.prepare(reporter.attempt.id, { kind: 'in_progress', eventKey: 'execution-started' }));
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'uncertain'); assert.equal(writes, 1);
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'confirmed'); assert.equal(writes, 1);
  assert.equal(store.getTask(run.taskId).jiraStatus, 'To Do', 'A delivery receipt does not invent an imported Jira status.');
  permissionDenied = true;
  (await delivery.prepare(reporter.attempt.id, { kind: 'blocker', eventKey: 'blocked', body: 'Awaiting access' }));
  await delivery.flush(); const blocked = delivery.read(run.id)[1];
  assert.equal(blocked.state, 'blocked'); assert.match(blocked.reason!, /403/); assert.equal(JSON.stringify(blocked).includes('private permission'), false);
});

test('DEL-03: an unknown comment write stays uncertain when reads cannot establish its outcome', async t => {
  let posts = 0;
  const provider = createServer((req, res) => {
    if (req.method === 'POST') { posts++; req.socket.destroy(); return; }
    res.end(JSON.stringify({ comments: [], startAt: 0, total: 0 }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-unknown-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const delivery = new JiraDelivery(store, new JiraClient({ source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }));
  const { run, reporter } = admitted(store, root);
  (await delivery.prepare(reporter.attempt.id, { kind: 'milestone', eventKey: 'started', body: 'Investigating' }));
  (await delivery.prepare(reporter.attempt.id, { kind: 'milestone', eventKey: 'next', body: 'Next milestone' }));
  await delivery.flush(); await delivery.flush(); await delivery.flush();
  assert.equal(delivery.read(run.id)[1].state, 'pending', 'Later updates retain their order while the first write is uncertain.');
  assert.equal(delivery.read(run.id)[0].state, 'uncertain'); assert.equal(posts, 1);
});

test('DEL-02/03: unavailable credentials before dispatch leave a retryable intent rather than an unknown write', async t => {
  let credentials = 0, posts = 0;
  const provider = createServer((req, res) => {
    if (req.method === 'POST') { posts++; res.statusCode = 201; res.end(JSON.stringify({ id: '88' })); return; }
    res.end(JSON.stringify({ comments: [], startAt: 0, total: 0 }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-credential-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const delivery = new JiraDelivery(store, new JiraClient({ source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`,
    credentials: async () => { if (++credentials === 2) throw new Error('private keychain detail'); return 'synthetic'; } }));
  const { run, reporter } = admitted(store, root);
  (await delivery.prepare(reporter.attempt.id, { kind: 'milestone', eventKey: 'started', body: 'Investigating' }));
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'pending'); assert.equal(posts, 0);
  await delivery.flush(); assert.equal(delivery.read(run.id)[0].state, 'confirmed'); assert.equal(posts, 1);
});

test('DEL-02/LIF-03: pausing during credential resolution prevents a later Jira mutation', async t => {
  let posts = 0, calls = 0; let release!: () => void, entered!: () => void;
  const ready = new Promise<void>(resolve => { entered = resolve; }), held = new Promise<void>(resolve => { release = resolve; });
  const provider = createServer((req, res) => {
    if (req.method === 'POST') { posts++; res.end(JSON.stringify({ id: '99' })); return; }
    res.end(JSON.stringify({ comments: [], startAt: 0, total: 0 }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-pause-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const delivery = new JiraDelivery(store, new JiraClient({ source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`,
    credentials: async () => { if (++calls === 2) { entered(); await held; } return 'synthetic'; } }));
  const { run, reporter } = admitted(store, root);
  (await delivery.prepare(reporter.attempt.id, { kind: 'milestone', eventKey: 'started', body: 'Investigating' }));
  const flush = delivery.flush(); await ready; store.setDesiredState(run.id, 'paused'); release(); await flush;
  assert.equal(posts, 0); assert.equal(delivery.read(run.id)[0].state, 'pending');
});

test('DEL-04: a confirmed PR moves the ticket to review and a verified merge closes it, once each, without waiting for agents', async t => {
  let status = '3'; const writes: string[] = [];
  const provider = createServer(async (req, res) => {
    res.setHeader('Content-Type', 'application/json');
    if (req.method === 'POST') { let body = ''; for await (const chunk of req) body += chunk; const id = JSON.parse(body).transition.id; writes.push(id); status = ({ '8': '10004', '2': '10003', '6': '3' } as Record<string, string>)[id]; res.statusCode = 204; res.end(); return; }
    // The workflow offers every transition from every status, so only Crewroom's own rules prevent a reopen.
    res.end(JSON.stringify(req.url!.includes('transitions') ? { transitions: [{ id: '8', to: { id: '10004' }, fields: {} }, { id: '2', to: { id: '10003' }, fields: {} }, { id: '6', to: { id: '3' }, fields: {} }] }
      : { id: '100', key: 'SIM-1', fields: { summary: 'Fix scrolling', status: { id: status, name: 'status' } } }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-policy-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const policy = { ...source, reviewStatusId: '10004', reviewTransitionId: '8', doneStatusId: '10003', doneTransitionId: '2' };
  const delivery = new JiraDelivery(store, new JiraClient({ source: policy, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }));
  const { run, reporter } = admitted(store, root);
  await assert.rejects(delivery.prepare(reporter.attempt.id, { kind: 'done', eventKey: 'agent-close' }), /Invalid/i, 'Agents cannot request review or close transitions.');
  // Only the fields the policy reads: a confirmed PR receipt, then the provider-observed merge.
  const record: any = { id: randomUUID(), runId: run.id, attemptId: reporter.attempt.id, candidateId: randomUUID(), push: { state: 'confirmed', expectedHead: null }, pr: { state: 'pending' }, ready: false, blockers: [] };
  store.savePublication(record); await delivery.flush(); assert.deepEqual(writes, [], 'No transition before the PR exists.');
  store.savePublication({ ...record, pr: { state: 'confirmed', id: 9, providerState: 'DECLINED' } }); await delivery.flush();
  assert.deepEqual(writes, [], 'A declined PR never moves the ticket to review.');
  // The PR's own state is enough: hosted observation (statuses, pipelines) may still be unavailable.
  store.savePublication({ ...record, pr: { state: 'confirmed', id: 9, providerState: 'OPEN' } }); await delivery.flush(); await delivery.flush();
  assert.deepEqual(writes, ['8']); assert.equal(delivery.read(run.id).find(item => item.kind === 'in_review')!.state, 'confirmed');
  // A reporter comment and a late progress transition waiting for the paused run do not hold the receipt-derived close.
  await delivery.prepare(reporter.attempt.id, { kind: 'milestone', eventKey: 'late-note', body: 'Waiting on review.' });
  await delivery.prepare(reporter.attempt.id, { kind: 'in_progress', eventKey: 'execution-started' });
  store.setDesiredState(run.id, 'paused'); store.beginPause(run.id); store.recordOwnership(reporter.attempt.id, true); store.finishPause(run.id);
  store.savePublication({ ...record, pr: { state: 'confirmed', id: 9 }, merged: { commit: 'c'.repeat(40), observedAt: new Date().toISOString() } });
  await delivery.flush(); await delivery.flush();
  assert.deepEqual(writes, ['8', '2'], 'The merge closes the ticket once, although the run is paused.');
  assert.equal(delivery.read(run.id).find(item => item.kind === 'done')!.state, 'confirmed');
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id); await delivery.flush();
  assert.deepEqual(writes, ['8', '2'], 'The late progress transition never reopens the merged ticket.');
  assert.equal(delivery.read(run.id).find(item => item.kind === 'in_progress')!.reason, 'Superseded by the merge.');
});

test('DEL-04: a merge observed before the review transition is sent closes the ticket without passing through review', async t => {
  let status = '3'; const writes: string[] = [];
  const provider = createServer(async (req, res) => {
    res.setHeader('Content-Type', 'application/json');
    if (req.method === 'POST') { let body = ''; for await (const chunk of req) body += chunk; const id = JSON.parse(body).transition.id; writes.push(id); status = id === '8' ? '10004' : '10003'; res.statusCode = 204; res.end(); return; }
    res.end(JSON.stringify(req.url!.includes('transitions') ? { transitions: [{ id: '8', to: { id: '10004' }, fields: {} }, { id: '2', to: { id: '10003' }, fields: {} }] }
      : { id: '100', key: 'SIM-1', fields: { summary: 'Fix scrolling', status: { id: status, name: 'status' } } }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-merge-first-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const policy = { ...source, reviewStatusId: '10004', reviewTransitionId: '8', doneStatusId: '10003', doneTransitionId: '2' };
  let offline = true;
  const delivery = new JiraDelivery(store, new JiraClient({ source: policy, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => { if (offline) throw new Error('offline'); return 'synthetic'; } }));
  const { run, reporter } = admitted(store, root);
  const record: any = { id: randomUUID(), runId: run.id, attemptId: reporter.attempt.id, candidateId: randomUUID(), push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 9, providerState: 'OPEN' }, ready: false, blockers: [] };
  store.savePublication(record); await delivery.flush();
  assert.equal(delivery.read(run.id).find(item => item.kind === 'in_review')!.state, 'pending', 'Jira was unavailable, so review is still pending.');
  store.savePublication({ ...record, merged: { commit: 'c'.repeat(40), observedAt: new Date().toISOString() } }); offline = false;
  await delivery.flush(); await delivery.flush();
  assert.deepEqual(writes, ['2']); assert.equal(delivery.read(run.id).find(item => item.kind === 'in_review')!.state, 'blocked');
  assert.equal(delivery.read(run.id).find(item => item.kind === 'done')!.state, 'confirmed');
});

test('DEL-04: a review transition with an unknown outcome is not resent, and does not stall the close after merge', async t => {
  let status = '3'; const writes: string[] = [];
  const provider = createServer(async (req, res) => {
    res.setHeader('Content-Type', 'application/json');
    // The review POST is lost before Jira applies it; the later close is answered normally.
    if (req.method === 'POST') { let body = ''; for await (const chunk of req) body += chunk; const id = JSON.parse(body).transition.id; writes.push(id); if (id === '8') { req.socket.destroy(); return; } status = '10003'; res.statusCode = 204; res.end(); return; }
    res.end(JSON.stringify(req.url!.includes('transitions') ? { transitions: [{ id: '8', to: { id: '10004' }, fields: {} }, { id: '2', to: { id: '10003' }, fields: {} }] }
      : { id: '100', key: 'SIM-1', fields: { summary: 'Fix scrolling', status: { id: status, name: 'status' } } }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-uncertain-review-')); const store = new Workbench(join(root, 'db'));
  t.after(async () => { store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const policy = { ...source, reviewStatusId: '10004', reviewTransitionId: '8', doneStatusId: '10003', doneTransitionId: '2' };
  const delivery = new JiraDelivery(store, new JiraClient({ source: policy, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }));
  const { run, reporter } = admitted(store, root);
  const record: any = { id: randomUUID(), runId: run.id, attemptId: reporter.attempt.id, candidateId: randomUUID(), push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 9, providerState: 'OPEN' }, ready: false, blockers: [] };
  store.savePublication(record); await delivery.flush(); await delivery.flush();
  assert.equal(delivery.read(run.id).find(item => item.kind === 'in_review')!.state, 'uncertain'); assert.deepEqual(writes, ['8'], 'An unknown review outcome is not resent.');
  store.savePublication({ ...record, merged: { commit: 'c'.repeat(40), observedAt: new Date().toISOString() } });
  await delivery.flush(); await delivery.flush();
  assert.deepEqual(writes, ['8', '2']); assert.equal(delivery.read(run.id).find(item => item.kind === 'done')!.state, 'confirmed');
  assert.match(delivery.read(run.id).find(item => item.kind === 'in_review')!.reason!, /outcome is unknown/);
});
