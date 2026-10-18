import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { chromium, expect } from '@playwright/test';
import { startServer } from '../server/app';
import { routeLiveConnection } from './live-connection';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await startServer({ port: 0, dataDir: config.root, production: true, rolePath: config.rolePath });
const browser = await chromium.launch(); const page = await browser.newPage({ viewport: { width: 1440, height: 1080 } });
const setConnection = await routeLiveConnection(page);
let source: string | undefined, original: string | undefined;
try {
  const workspace = await (await fetch(app.origin + '/api/workspace')).json(); const run = workspace.runs[0];
  const result = await (await fetch(app.origin + `/api/runs/${run.id}/result`)).json();
  assert.equal(result.assessment.ready, true);
  await page.goto(`${app.origin}/#${run.taskId}`);
  // Evidence lives behind the run's "Change & evidence" tab; the selection persists across reloads.
  await page.getByRole('tab', { name: 'Change & evidence', exact: true }).click();
  const view = page.getByRole('region', { name: 'Change and evidence' });
  await expect(view.getByText('Locally validated', { exact: true })).toBeVisible();
  await expect(view.getByText('PR publication is unavailable. Local validation can complete; no PR has been created.')).toBeVisible();
  await expect(view.getByText('Independent review passed', { exact: true })).toBeVisible();
  await expect(view.getByText('Document the empty-list contract', { exact: true })).toBeVisible();
  for (const check of result.checks.filter((item: any) => item.candidateId === result.candidates.at(-1).id && item.environment.memoryMiB !== undefined)) {
    await expect(view.getByText(`Memory limit ${check.environment.memoryMiB} MiB`, { exact: false })).toBeVisible();
    if (check.environment.tmpExecutable !== undefined) {
      await view.getByText('Check identity and artifacts', { exact: true }).click();
      await expect(view.getByText(`Temporary file execution: ${check.environment.tmpExecutable ? 'enabled' : 'disabled'}`, { exact: true })).toBeVisible();
      await view.getByText('Check identity and artifacts', { exact: true }).click();
    }
  }
  await view.getByRole('button', { name: 'View change', exact: true }).click();
  await expect(view.getByLabel('Candidate diff')).toContainText('+    return sum(xs)');
  await view.getByRole('button', { name: 'View check output', exact: true }).click();
  await expect(view.getByLabel('Evidence content')).toContainText('test_empty');
  await view.getByText('Recorded usage and cost', { exact: true }).click();
  await expect(view.getByText('OMP cost estimate (USD)', { exact: true })).toBeVisible();
  await expect(view.getByText('provider requests reported usage', { exact: false }).first()).toBeVisible();
  await expect(view.getByText(/^\d[\d,]* actual/).first()).toBeVisible();
  await expect(view.getByText('Provider billing unavailable', { exact: false })).toBeVisible();
  if (result.candidates.length > 1) {
    await view.getByLabel('Candidate revision').selectOption(result.candidates[0].id);
    await expect(view.getByText('Historical revision', { exact: true })).toBeVisible();
    await expect(view.getByText('Failed', { exact: true })).toBeVisible();
    await view.getByLabel('Candidate revision').selectOption(result.candidates[1].id);
    await expect(view.getByText('Independent review requested changes', { exact: true })).toBeVisible();
    await expect(view.getByText('Document the empty-list contract.', { exact: true })).toBeVisible();
    await view.getByLabel('Candidate revision').selectOption(result.candidates.at(-1).id);
  }
  await view.screenshot({ path: join(config.root, 'results.png') });
  const owned = await (await fetch(app.origin + `/api/runs/${run.id}/repository`)).json();
  source = join(owned.path, 'sum.py'); original = readFileSync(source, 'utf8');
  writeFileSync(source, 'def sum_values(xs): return 99\n');
  await expect(view.getByText('Validation incomplete', { exact: true })).toBeVisible({ timeout: 10000 });
  await expect(view.getByText('The repository revision or accepted requirements changed.', { exact: true })).toBeVisible();
  writeFileSync(source, original); original = undefined;
  await expect(view.getByText('Locally validated', { exact: true })).toBeVisible();
  await setConnection(false);
  await expect(view.getByText('Evidence may be out of date. Reconnecting…', { exact: true })).toBeVisible();
  await expect(view.getByText('Locally validated', { exact: true })).toHaveCount(0);
  await setConnection(true);
  await expect(view.getByText('Locally validated', { exact: true })).toBeVisible();
  await page.reload(); await expect(view.getByText('Locally validated', { exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await view.screenshot({ path: join(config.root, 'results-mobile.png') });
  console.log(JSON.stringify({ passed: true, checks: ['actual revision diff', 'hash-verified check output', 'independent review and criteria', 'failed revision history', 'drift invalidates readiness', 'reconnect without stale readiness', 'recorded usage provenance', 'publication unavailable', 'mobile layout'] }));
} finally { if (source && original !== undefined) writeFileSync(source, original); await browser.close(); await app.close(); }
