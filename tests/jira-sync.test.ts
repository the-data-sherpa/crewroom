import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startServer } from '../server/app';
import { seedRepository } from './fixtures/repository';

const source = { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator-1', name: 'Assigned work', inProgressStatusId: '3', inProgressTransitionId: '6' };
const issue = (id: string, summary: string, status = 'To Do') => ({ id, key: `SIM-${id}`, fields: { summary, description: 'Imported requirement', status: { id: status === 'To Do' ? '1' : '3', name: status }, updated: '2026-09-28T00:00:00.000Z' } });

test('INT-01/02/04: live paginated intake preserves source order and local refinements across refresh and restart', async t => {
  let refreshed = false; const observed: string[] = [];
  const provider = createServer((req, res) => {
    assert.equal(req.headers.authorization, 'Bearer synthetic-jira-token');
    const url = new URL(req.url!, 'http://fixture'); observed.push(url.pathname);
    assert.equal(url.pathname, '/rest/api/3/search/jql');
    assert.equal(url.searchParams.get('jql'), 'filter = 12345 AND assignee = "operator-1" ORDER BY Rank ASC');
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify(url.searchParams.get('nextPageToken') ? { issues: [issue('1', 'Second in source order')], isLast: true } : {
      issues: [issue('2', 'First in source order', refreshed ? 'In Progress' : 'To Do')], isLast: false, nextPageToken: 'page-two',
    }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const address = provider.address() as { port: number };
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-'));
  const options = { port: 0, dataDir: root, rolePath: join(root, 'roles.json'), serveUi: false,
    jira: { source, apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'synthetic-jira-token' } };
  let app = await startServer(options);
  t.after(async () => { await app.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const post = async (path: string, body = {}) => {
    const response = await fetch(app.origin + '/api' + path, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    assert.equal(response.status, 200); return response.json();
  };
  await post('/jira/sync');
  let workspace = await (await fetch(app.origin + '/api/workspace')).json();
  assert.deepEqual(workspace.tasks.map((task: any) => task.key), ['SIM-2', 'SIM-1']);
  assert.equal(workspace.runs.length, 0, 'Import never queues work.');
  assert.equal(workspace.jira.state, 'connected');
  assert.equal(workspace.jira.source.filterId, '12345');
  assert.equal(JSON.stringify(workspace).includes('synthetic-jira-token'), false);
  const first = workspace.tasks[0];
  await post(`/tasks/${first.id}/refinement`, { revision: first.revision, sourceRevision: first.sourceRevision, brief: 'Operator refinement', criteria: ['Retain this criterion'] });
  refreshed = true; await post('/jira/sync'); await app.close(); app = await startServer(options);
  workspace = await (await fetch(app.origin + '/api/workspace')).json();
  assert.deepEqual(workspace.tasks.map((task: any) => task.key), ['SIM-2', 'SIM-1']);
  assert.equal(workspace.tasks[0].brief, 'Operator refinement');
  assert.equal(workspace.tasks[0].jiraStatus, 'In Progress');
  assert.equal(workspace.tasks[0].needsReacceptance, false, 'A Jira status update is not a requirements change.');
  assert.equal(workspace.jira.lastSyncedAt.length > 0, true);
  assert.equal(workspace.runs.length, 0);
  assert.equal(observed.length, 4);
});

test('INT-05/DEL-02: partial outages retain the last complete source and refreshed requirements stop the affected run', async t => {
  let mode = 'initial';
  const provider = createServer((req, res) => {
    const url = new URL(req.url!, 'http://fixture'); res.setHeader('Content-Type', 'application/json');
    if (url.pathname === '/rest/api/3/issue/2') { res.end(JSON.stringify(issue('2', 'Changed after leaving the filter'))); return; }
    if (mode === 'partial') {
      if (url.searchParams.has('nextPageToken')) { res.statusCode = 503; res.end('synthetic-jira-token must not be echoed'); return; }
      res.end(JSON.stringify({ issues: [issue('2', 'Uncommitted partial change')], isLast: false, nextPageToken: 'broken' })); return;
    }
    res.end(JSON.stringify({ issues: mode === 'empty' ? [] : [issue('2', mode === 'changed' ? 'New acceptance requirement' : 'Original requirement')], isLast: true }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-outage-'));
  seedRepository(root);
  const app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json'), serveUi: false,
    jira: { source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic-jira-token' } });
  t.after(async () => { await app.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const post = async (path: string, body = {}) => { const response = await fetch(app.origin + '/api' + path, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) }); assert.ok(response.ok); return response.json(); };
  const read = async () => (await fetch(app.origin + '/api/workspace')).json();
  await post('/jira/sync'); let workspace = await read(); const ticket = workspace.tasks[0];
  const repo = await post('/repositories', { name: 'Fixture', path: root, targetBranch: 'main' });
  const assigned = await post(`/tasks/${ticket.id}/repository`, { revision: 1, repositoryId: repo.id });
  const accepted = await post(`/tasks/${ticket.id}/refinement`, { revision: assigned.revision, sourceRevision: 1, brief: 'Keep the accepted brief', criteria: ['Keep the accepted criterion'] });
  const run = await post(`/tasks/${ticket.id}/runs`, { revision: accepted.revision });
  mode = 'partial'; await post('/jira/sync'); workspace = await read();
  assert.equal(workspace.jira.state, 'unavailable'); assert.equal(workspace.tasks[0].title, 'Original requirement');
  assert.equal(JSON.stringify(workspace).includes('synthetic-jira-token'), false);
  assert.equal(workspace.runs[0].status, 'awaiting_runner', 'An outage does not change an accepted plan.');
  mode = 'changed'; await post('/jira/sync'); workspace = await read();
  assert.equal(workspace.tasks[0].needsReacceptance, true); assert.equal(workspace.tasks[0].brief, 'Keep the accepted brief');
  assert.equal(workspace.runs[0].id, run.id); assert.equal(workspace.runs[0].status, 'awaiting_runner', 'The unadmitted plan remains queued, not running.');
  assert.equal(workspace.runs[0].desiredState, 'paused');
  assert.equal(workspace.runs[0].taskSnapshot.title, 'Original requirement');
  assert.match(workspace.runs[0].attentionReason, /requirements/i);
  assert.equal(workspace.queue.plans[0].id, run.id);
  assert.equal(workspace.queue.plans[0].holdReason, 'Requirements changed');
  mode = 'empty'; await post('/jira/sync'); workspace = await read();
  assert.deepEqual(workspace.jira.taskIds, []); assert.equal(workspace.tasks[0].title, 'Changed after leaving the filter', 'Admitted work remains synchronized after leaving the intake filter.'); assert.equal(workspace.tasks[0].brief, 'Keep the accepted brief', 'Leaving the filter retains ticket work and history.');
});

test('DEL-06: stale observations and malformed pagination cannot replace a complete saved intake', async t => {
  let mode = 'initial';
  const provider = createServer((_req, res) => {
    const row = issue('1', mode === 'initial' ? 'Current title' : 'Stale title');
    row.fields.updated = mode === 'stale' ? '2026-09-27T00:00:00.000Z' : '2026-09-28T00:00:00.000Z';
    res.end(JSON.stringify({ issues: [row], isLast: mode !== 'incomplete' }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-stale-'));
  const app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json'), serveUi: false,
    jira: { source, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' } });
  t.after(async () => { await app.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const refresh = () => fetch(app.origin + '/api/jira/sync', { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: '{}' });
  await refresh();
  for (mode of ['stale', 'incomplete']) {
    await refresh(); const workspace = await (await fetch(app.origin + '/api/workspace')).json();
    assert.equal(workspace.jira.state, 'unavailable'); assert.equal(workspace.tasks[0].title, 'Current title'); assert.equal(workspace.tasks[0].sourceRevision, 1);
  }
});
