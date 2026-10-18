import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { chromium, expect } from '@playwright/test';
import { startServer } from '../server/app';
import { routeLiveConnection } from './live-connection';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await startServer({ port: config.serverPort, dataDir: config.root, production: true, rolePath: config.rolePath, runtimeConfig: config.runtime });
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const setConnection = await routeLiveConnection(page);
async function request(path: string, body?: unknown) {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
}
try {
  const repo = await request('/repositories', { name: 'Fixture', path: config.root, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Supervise a three-agent exchange', description: 'A guarded local fixture', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Observe and steer independent processes', criteria: ['Preserve messages through pause and resume'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  await request(`/runs/${run.id}/start`, { team: [
    { name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Coordinate the peer exchange.' },
    { name: 'SpecialistOne', roleId: 'implementer', coordinator: false, input: 'Relay through SpecialistTwo.' },
    { name: 'SpecialistTwo', roleId: 'reviewer', coordinator: false, input: 'Reply to SpecialistOne.' },
  ] });
  await page.goto(`${app.origin}/#${task.id}`);
  await expect(page.getByRole('heading', { name: 'Agent supervision' })).toBeVisible();
  await expect(page.getByText('ROUNDTRIP_COMPLETE', { exact: true })).toBeVisible({ timeout: 20000 });
  await expect(page.getByRole('heading', { name: 'SpecialistOne', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'SpecialistTwo', exact: true })).toBeVisible();
  await expect(page.getByText('anthropic.claude-sonnet-5', { exact: false }).first()).toBeVisible();
  await expect(page.getByText('gemini-3.8-flash', { exact: false }).first()).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Peer messages' })).toBeVisible();
  const initial = await request(`/runs/${run.id}/runtime`);
  const observationUrl = await page.getByRole('link', { name: 'Open observation view' }).getAttribute('href');
  const observer = await browser.newPage(); const mutations: string[] = [];
  observer.on('request', request => { if (request.url().includes('/api/') && request.method() !== 'GET') mutations.push(request.url()); });
  await observer.goto(new URL(observationUrl!, app.origin).href);
  await expect(observer.getByRole('heading', { name: 'Observation view' })).toBeVisible();
  await expect(observer.getByRole('button', { name: 'Pause run', exact: true })).toHaveCount(0);
  await expect(observer.getByLabel('Message to coordinator')).toHaveCount(0);
  await observer.reload();
  await expect(observer.getByText('ROUNDTRIP_COMPLETE', { exact: true })).toBeVisible();
  assert.deepEqual(mutations, []);
  assert.deepEqual((await request(`/runs/${run.id}/runtime`)).messages, initial.messages);
  await page.getByRole('button', { name: 'Pause run', exact: true }).click();
  await expect(page.getByText('Paused', { exact: true }).first()).toBeVisible({ timeout: 15000 });
  await page.getByLabel('Message to coordinator').fill('Document the fixture.');
  await setConnection(false);
  await expect(page.getByText('Connection lost. Retained activity may be out of date. Reconnecting…', { exact: true })).toBeVisible();
  await setConnection(true);
  await expect(page.getByText('Connection lost. Retained activity may be out of date. Reconnecting…', { exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Message to coordinator')).toHaveValue('Document the fixture.');
  await page.getByRole('button', { name: 'Send to coordinator', exact: true }).click();
  await expect(page.getByText('Stored · Not delivered', { exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByText('Stored · Not delivered', { exact: true })).toBeVisible();
  await page.screenshot({ path: `${config.root}/paused.png`, fullPage: true });
  await page.getByRole('button', { name: 'Resume run', exact: true }).click();
  await expect(page.getByText('DOCUMENTATION_PLANNED', { exact: true })).toBeVisible({ timeout: 30000 });
  await expect(page.getByText('Received by coordinator', { exact: true })).toBeVisible();
  const resumed = await request(`/runs/${run.id}/runtime`);
  assert.equal(resumed.attempts.length, 6);
  for (let index = 0; index < 3; index++) assert.equal(resumed.attempts[index].ownership, 'stopped');
  assert.notEqual(resumed.attempts[3].ompPid, initial.attempts[0].ompPid);
  await page.getByRole('button', { name: 'Cancel run', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm cancel', exact: true }).click();
  await expect(page.getByText('Cancelled', { exact: true }).first()).toBeVisible({ timeout: 15000 });
  await expect(page.getByRole('button', { name: 'Resume run', exact: true })).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await page.screenshot({ path: `${config.root}/mobile.png`, fullPage: true });
  const next = await request('/tasks', { title: 'Start from the browser', description: '', repositoryId: repo.id });
  await request(`/tasks/${next.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Start a configured coordinator', criteria: ['Show its actual reply'] });
  await request(`/tasks/${next.id}/runs`, { revision: 2 });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(`${app.origin}/#${next.id}`);
  await expect(page.getByRole('button', { name: 'Start coordinator', exact: true })).toBeVisible();
  await page.getByLabel('Coordinator role').selectOption('triage');
  await page.getByRole('button', { name: 'Start coordinator', exact: true }).click();
  await expect(page.getByText('COORDINATOR_STARTED_FROM_BROWSER', { exact: true })).toBeVisible({ timeout: 20000 });
  console.log(JSON.stringify({ passed: true, checks: ['real heterogeneous processes visible', 'passive observation and reconnect', 'pause and retain steering', 'resume new attempts', 'cancel preserves history', 'mobile layout', 'connection loss retains draft', 'start coordinator from browser'] }));
} finally { await browser.close(); await app.close(); }
