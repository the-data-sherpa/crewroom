import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { cancelledFixture } from '../fixtures/cancel-continuation';
import { startServer } from '../../server/app';

test('CAN-02/04/05: keyboard continuation keeps one successor, both history links and original evidence across restart at 390px', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-browser-')); const f = await cancelledFixture(root, true);
  await f.cancel(); await f.publication.close(); f.store.close();
  const rolePath = join(root, 'roles.json'); writeFileSync(rolePath, JSON.stringify(f.roles));
  const options = { port: 0, dataDir: root, production: true, rolePath, publication: f.connections };
  let app = await startServer(options);
  try {
    await page.setViewportSize({ width: 390, height: 844 }); await page.goto(`${app.origin}/#${f.run.taskId}`);
    const panel = page.getByRole('region', { name: 'Run continuation' });
    await expect(panel).toContainText(f.owned.branch); await expect(panel.getByRole('link', { name: 'Existing PR' })).toBeVisible();
    const button = panel.getByRole('button', { name: 'Continue later' }); await expect(button).toBeEnabled();
    const request = page.waitForRequest(item => item.url().endsWith('/continue-later') && item.method() === 'POST');
    await button.focus(); await page.keyboard.press('Enter'); const sent = await request;
    await expect(panel.getByRole('link', { name: 'cancelled predecessor' })).toBeVisible();
    const replay = await page.request.post(sent.url(), { headers: { Origin: app.origin }, data: sent.postDataJSON() });
    expect(replay.status()).toBe(201); const next = await replay.json();
    const launchSave = page.waitForRequest(item => item.url().endsWith('/launch') && item.method() === 'POST');
    await page.getByRole('button', { name: 'Save launch details', exact: true }).click();
    const launch = (await launchSave).postDataJSON().launch;
    expect(launch.team[0].input).toBe(next.launch.team[0].input);
    expect(JSON.parse(launch.team[0].input).previousRunId).toBe(f.run.id);
    await expect(page.getByRole('button', { name: 'Resume run' })).toHaveCount(0);
    await expect(page.getByText('Earlier evidence stays in that run;', { exact: false })).toBeVisible();
    await panel.getByRole('link', { name: 'cancelled predecessor' }).click();
    await expect(panel.getByRole('link', { name: 'linked successor' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Continue later' })).toHaveCount(0);
    await page.getByRole('tab', { name: 'Change & evidence' }).click();
    await expect(page.getByText(f.candidate.revision.head.slice(0, 12), { exact: false }).first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    mkdirSync('.crewroom/test-results/m64-slice-f-gates', { recursive: true });
    await page.screenshot({ path: '.crewroom/test-results/m64-slice-f-gates/history-narrow.png', fullPage: true });
    await app.close(); app = await startServer(options); await page.goto(`${app.origin}/#${f.run.taskId}`);
    const workspace = await (await page.request.get(`${app.origin}/api/workspace`)).json();
    expect(workspace.runs).toHaveLength(2); expect(workspace.runs.find((run: { id: string }) => run.id === f.run.id).status).toBe('cancelled');
    expect(workspace.runs.find((run: { id: string }) => run.id === next.id).continuedFrom.runId).toBe(f.run.id);
    const denied = await page.request.post(`${app.origin}/api/runs/${f.run.id}/continue-later`, { data: sent.postDataJSON() }); expect(denied.status()).toBe(403);
  } finally { await app.close(); await f.close(); rmSync(root, { recursive: true, force: true }); }
});

test('CAN-01/03/05: cancel confirmation is keyboard accessible and unowned cancelled plans offer a new branch only', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-cancel-plan-browser-')); const f = await cancelledFixture(root);
  await f.cancel();
  const task = f.store.createTask({ title: 'Cancel an unstarted plan', description: '', repositoryId: f.run.taskSnapshot.repositoryId });
  const brief = f.store.refine(task.id, { revision: task.revision, sourceRevision: 1, brief: 'Retain the plan', criteria: ['No execution'] });
  f.store.queue(task.id, { revision: brief.revision, requestId: randomUUID() });
  await f.publication.close(); f.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
  try {
  await page.setViewportSize({ width: 390, height: 844 }); await page.goto(`${app.origin}/#${task.id}`);
  await page.getByRole('button', { name: 'Cancel run', exact: true }).focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('group', { name: 'Confirm cancellation' })).toBeVisible();
  await page.getByRole('button', { name: 'Keep run', exact: true }).click();
  await page.getByRole('button', { name: 'Cancel run', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm cancel', exact: true }).focus(); await page.keyboard.press('Enter');
  await expect(page.getByRole('region', { name: 'Run continuation' })).toContainText('Queue a new-branch run instead');
  await expect(page.getByRole('button', { name: 'Continue later' })).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await app.close(); await f.close(); rmSync(root, { recursive: true, force: true }); }
});
