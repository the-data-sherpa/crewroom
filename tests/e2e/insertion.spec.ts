import { test, expect } from '@playwright/test';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { insertionFixture } from '../fixtures/insertion';
import { startServer } from '../../server/app';

const evidence = '.crewroom/test-results/m64-slice-e-gates';
test('INS-01/02/05: queued insertion by keyboard, replay, coordinator notice, restart and narrow layout', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-browser-')); const f = insertionFixture(root); f.store.close();
  const options = { port: 0, dataDir: root, production: true, rolePath: f.rolePath, runtimeConfig: f.runtimeConfig };
  let app = await startServer(options);
  try {
    await page.setViewportSize({ width: 390, height: 844 }); await page.goto(`${app.origin}/#${f.run.taskId}`);
    const summary = page.getByText('Add a specialist', { exact: true }); await summary.focus(); await page.keyboard.press('Enter');
    const form = page.getByRole('form', { name: 'Add a specialist' });
    await expect(form.getByRole('option', { name: /^SRE/ })).toHaveCount(0);
    await form.getByLabel('Specialist role').selectOption('tech-writer');
    await form.getByLabel('Specialist input').focus(); await page.keyboard.type('Explain the current result to the coordinator.');
    await page.keyboard.press('Tab'); await page.keyboard.press('Space');
    await page.keyboard.press('Tab'); await page.keyboard.press('Space');
    await page.keyboard.press('Tab');
    await expect(form.getByRole('button', { name: 'Propose specialist' })).toBeFocused();
    const posted = page.waitForRequest(request => request.url().endsWith('/specialists') && request.method() === 'POST');
    await page.keyboard.press('Enter'); const sent = await posted;
    await expect(page.getByText('Proposed · not started', { exact: true })).toBeVisible();
    await expect(page.getByText('You proposed this specialist.', { exact: false })).toBeVisible();
    await expect(form.getByRole('button')).toBeDisabled();
    const conversation = page.getByRole('region', { name: 'Coordinator conversation' });
    await expect(conversation).toContainText('Specialist insertion'); await expect(conversation).toContainText('Stored · Coordinator receipt unconfirmed');
    const replay = await page.request.post(sent.url(), { headers: { Origin: app.origin }, data: sent.postDataJSON() }); expect(replay.status()).toBe(201);
    let runtime = await (await page.request.get(`${app.origin}/api/runs/${f.run.id}/runtime`)).json();
    expect(runtime.teamProposals).toHaveLength(1); expect(runtime.assignments).toHaveLength(0); expect(runtime.deliveryNotices).toHaveLength(1);
    expect(runtime.teamProposals[0].specialists[0].brief.dependsOn).toEqual(['Coordinator']);
    expect(runtime.teamProposals[0].specialists[0].brief.criteria[0].criterion).toBe('Explain the result');
    await expect(page.getByText('Wait for the existing team proposal to be applied.')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    mkdirSync(evidence, { recursive: true }); await page.screenshot({ path: `${evidence}/insertion-narrow.png`, fullPage: true });
    await app.close(); app = await startServer(options); await page.goto(`${app.origin}/#${f.run.taskId}`);
    await expect(page.getByText('Proposed · not started', { exact: true })).toBeVisible();
    runtime = await (await page.request.get(`${app.origin}/api/runs/${f.run.id}/runtime`)).json(); expect(runtime.assignments).toHaveLength(0);
    await page.setViewportSize({ width: 1440, height: 1000 }); await page.screenshot({ path: `${evidence}/insertion-desktop.png`, fullPage: true });
    const cancel = await page.request.post(`${app.origin}/api/runs/${f.run.id}/cancel`, { headers: { Origin: app.origin }, data: {} }); expect(cancel.ok()).toBe(true);
    await expect(page.getByRole('article', { name: 'Rejected team proposal' })).toContainText('The run ended before this specialist was admitted.');
    await summary.click(); await expect(page.getByText('This run has ended. Specialist insertion is unavailable.')).toBeVisible();
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});

test('INS-04/05: insertion refuses an unavailable runner without losing the explanation', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-offline-')); const f = insertionFixture(root); f.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: f.rolePath });
  try {
    await page.goto(`${app.origin}/#${f.run.taskId}`); await page.getByText('Add a specialist', { exact: true }).click();
    await expect(page.getByText('Runner integration is unavailable; specialist models cannot be validated.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Propose specialist' })).toBeDisabled();
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});

test('INS-02/04: after adoption rejects a proposal, an identical submission uses a new identity without reloading', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-insertion-resubmit-')); const f = insertionFixture(root); f.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: f.rolePath, runtimeConfig: f.runtimeConfig });
  try {
    await page.goto(`${app.origin}/#${f.run.taskId}`); await page.getByText('Add a specialist', { exact: true }).click();
    const form = page.getByRole('form', { name: 'Add a specialist' });
    const submit = async () => {
      await form.getByLabel('Specialist role').selectOption('tech-writer');
      await form.getByLabel('Specialist input').fill('Explain the current result.');
      const response = page.waitForResponse(response => response.url().endsWith('/specialists') && response.request().method() === 'POST');
      await form.getByRole('button').click(); const result = await response; expect(result.status()).toBe(201); return result.json();
    };
    const first = await submit(); await expect(page.getByText('Wait for the existing team proposal to be applied.')).toBeVisible();
    const post = (path: string, data: unknown) => page.request.post(`${app.origin}/api${path}`, { headers: { Origin: app.origin }, data });
    expect((await post(`/tasks/${f.run.taskId}/refinement`, { revision: 2, sourceRevision: 1, brief: 'Clarified accepted work', criteria: ['Explain the result'] })).ok()).toBe(true);
    expect((await post(`/runs/${f.run.id}/requirements/adopt`, { requestId: crypto.randomUUID(), revision: 3 })).ok()).toBe(true);
    await expect(page.getByRole('article', { name: 'Rejected team proposal' })).toContainText('Accepted requirements changed');
    await expect(form.getByRole('button')).toBeEnabled();
    const second = await submit(); expect(second.id).not.toBe(first.id); expect(second.request.requestId).not.toBe(first.request.requestId);
    expect(second.request.input).toBe(first.request.input);
    await expect(page.getByText('Proposed · not started', { exact: true })).toHaveCount(1);
    const runtime = await (await page.request.get(`${app.origin}/api/runs/${f.run.id}/runtime`)).json();
    expect(runtime.teamProposals.map((proposal: { status: string }) => proposal.status)).toEqual(['rejected', 'pending']);
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});
