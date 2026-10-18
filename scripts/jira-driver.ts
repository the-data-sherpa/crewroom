import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { startServer } from '../server/app';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
let offline = true, changed = false, posts = 0; const comments: any[] = [];
const issue = () => ({ id: '100', key: 'SIM-1', fields: { summary: 'Jira delivery fixture', description: changed ? 'Changed source requirement' : 'Original requirement', status: { name: 'To Do' } } });
const provider = createServer(async (req, res) => {
  res.setHeader('Content-Type', 'application/json');
  if (req.url!.includes('/search/jql')) { res.end(JSON.stringify({ issues: [issue()], isLast: true })); return; }
  if (offline) { res.statusCode = 503; res.end('{}'); return; }
  if (req.method === 'POST') {
    let body = ''; for await (const chunk of req) body += chunk;
    comments.push({ ...JSON.parse(body), id: 'comment-1' }); posts++; req.socket.destroy(); return;
  }
  res.end(JSON.stringify({ comments, startAt: 0, total: comments.length }));
});
await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
const options = { port: config.serverPort, dataDir: config.root, production: true, rolePath: config.rolePath, runtimeConfig: config.runtime,
  jira: { source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator-1', name: 'Assigned work' },
    apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'private-jira-fixture-token' } };
let app = await startServer(options); const browser = await chromium.launch();
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); if (!response.ok) throw new Error(JSON.stringify(result)); return result;
};
const observedRuntime = createRuntimeObserver(request);
async function until<T>(read: () => Promise<T>, ready: (value: T) => boolean) {
  const deadline = Date.now() + 30000; let value: T;
  do { value = await read(); if (ready(value)) return value; await new Promise(resolve => setTimeout(resolve, 100)); } while (Date.now() < deadline);
  throw new Error('Expected fixture state did not arrive: ' + JSON.stringify(value!));
}
try {
  await request('/jira/sync', {}); const task = (await request('/workspace')).tasks[0];
  const repository = await request('/repositories', { name: 'Fixture', path: config.root, targetBranch: 'main' });
  await request(`/tasks/${task.id}/repository`, { revision: 1, repositoryId: repository.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 2, sourceRevision: 1, brief: 'Report through Jira', criteria: ['One confirmed comment'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 3 });
  await request(`/runs/${run.id}/start`, { team: [
    { name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Ask Reporter to report progress.' },
    { name: 'Reporter', roleId: 'delivery-reporter', coordinator: false, input: 'Queue the assigned milestone using Jira reporting tools.' },
  ] });
  const runtime = await until(() => observedRuntime(run.id), value => JSON.stringify(value.events).includes('JIRA_REPORT_COMPLETE') && value.attempts.every((item: any) => item.state === 'idle'));
  assert.equal(new Set(runtime.attempts.map((item: any) => item.ompPid)).size, 2);
  assert.equal(new Set(runtime.attempts.map((item: any) => item.sessionId)).size, 2);
  assert.equal(runtime.messages.length, 2); assert.ok(runtime.messages.every((item: any) => item.disposition === 'received'));
  assert.equal(JSON.stringify(runtime).includes('private-jira-fixture-token'), false);
  await request('/jira/reconcile', {}); assert.equal((await request('/workspace')).jiraUpdates[0].state, 'pending');
  const page = await browser.newPage(); await page.goto(app.origin + '/#' + task.id);
  await page.getByRole('tab', { name: 'Change & evidence', exact: true }).click();
  await page.getByRole('region', { name: 'Jira delivery', exact: true }).getByText('milestone · pending', { exact: true }).waitFor();
  offline = false; await request('/jira/reconcile', {});
  assert.equal((await request('/workspace')).jiraUpdates[0].state, 'uncertain'); assert.equal(posts, 1);
  changed = true; await request('/jira/sync', {});
  const stopped = await until(() => request(`/runs/${run.id}/runtime`), value => value.attempts.every((item: any) => item.ownership === 'stopped'));
  assert.equal((await request('/workspace')).runs[0].status, 'attention_required');
  await app.close(); app = await startServer(options); await request('/jira/reconcile', {});
  const workspace = await request('/workspace');
  assert.equal(workspace.jiraUpdates[0].state, 'confirmed'); assert.equal(workspace.jiraUpdates[0].externalId, 'comment-1'); assert.equal(posts, 1);
  assert.equal(workspace.runs[0].taskSnapshot.description, 'Original requirement'); assert.equal(workspace.tasks[0].brief, 'Report through Jira');
  const resume = await fetch(app.origin + `/api/runs/${run.id}/resume`, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: '{}' });
  assert.equal(resume.status, 409);
  await page.reload(); await page.getByRole('region', { name: 'Jira delivery', exact: true }).getByText('milestone · confirmed', { exact: true }).waitFor();
  console.log(JSON.stringify({ passed: true, independentProcesses: 2, mailReceipts: 2, commentPosts: posts, restartReconciled: true, changedRequirementsStoppedProcesses: stopped.attempts.length, staleResumeBlocked: true, browserReceipts: ['pending', 'confirmed'] }));
} finally { await browser.close(); await app.close(); await new Promise<void>(resolve => provider.close(() => resolve())); }
