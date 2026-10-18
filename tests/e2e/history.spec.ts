import { test, expect } from '@playwright/test';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { historyFixture } from '../fixtures/history';
import { startServer } from '../../server/app';

test('HIS-04: history survives reload and Back; prior evidence is read-only and tabs work at 390 px', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-history-browser-'));
  const fixture = await historyFixture(root);
  fixture.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
  try {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`${app.origin}/#${fixture.run.taskId}`);
    const history = page.getByRole('combobox', { name: 'Run history' });
    await expect(history).toHaveValue(fixture.next.id);
    await expect(history.locator('option', { hasText: 'Cancelled; work retained' })).toContainText('Started');
    await history.selectOption(fixture.run.id);
    await expect(page).toHaveURL(new RegExp(`run=${fixture.run.id}`));
    const wall = page.getByRole('region', { name: 'Agent supervision' });
    await wall.getByText('Recorded usage and cost', { exact: true }).click();
    const wallUsage = wall.getByRole('region', { name: 'Run usage' });
    await expect(wallUsage).toContainText('Provider tokens · partial · 20 recorded');
    await expect(wallUsage).toContainText('Reviewer attempt 2');
    await page.getByRole('tab', { name: 'Change & evidence' }).click();
    await page.getByRole('tabpanel').getByText('Recorded usage and cost', { exact: true }).click();
    await expect(page.getByRole('tabpanel').getByRole('region', { name: 'Run usage' })).toHaveText(await wallUsage.innerText(), { useInnerText: true });
    await page.getByRole('navigation', { name: 'Attempts', exact: true }).getByRole('link', { name: 'Reviewer · attempt 1 · prior · stopped', exact: true }).focus();
    await page.keyboard.press('Enter');
    const detail = page.getByRole('article', { name: 'Reviewer attempt 1 details', exact: true });
    await expect(detail).toContainText('Read-only prior attempt');
    await expect(detail).toContainText('Fixture turn interrupted');
    await expect(detail).toContainText('signal SIGTERM');
    await expect(detail).toContainText('Fixture process identity was uncertain');
    await expect(detail).toContainText('Provider tokens · actual · 20 recorded');
    await expect(detail).toContainText(fixture.candidate.revision.head);
    await expect(page.getByRole('navigation', { name: 'Attempts', exact: true }).getByRole('link', { name: /Reporter · attempt 1 · retired/ })).toBeVisible();
    await expect(detail.getByRole('button', { name: /Resume|Cancel|Start/ })).toHaveCount(0);
    await detail.getByRole('button', { name: 'View saved artifact' }).first().click();
    await expect(detail.getByLabel('Saved attempt evidence')).toContainText('Controlled controller check receipt');
    await expect(page).toHaveURL(new RegExp(`attempt=${fixture.oldAttemptId}`));
    await page.reload(); await expect(detail).toBeVisible(); await expect(page.getByRole('tab', { name: 'Change & evidence' })).toHaveAttribute('aria-selected', 'true');
    await page.goBack(); await expect(wall).toBeVisible(); await expect(detail).toHaveCount(0);
    await page.goBack(); await expect(page.getByRole('tab', { name: 'Coordinator', exact: true })).toHaveAttribute('aria-selected', 'true');
    await page.goBack(); await expect(history).toHaveValue(fixture.next.id);
    await history.selectOption(fixture.run.id);
    const tabs = page.getByRole('tablist', { name: 'Run details' });
    const coordinator = tabs.getByRole('tab', { name: 'Coordinator', exact: true });
    await coordinator.focus(); await page.keyboard.press('End');
    await expect(tabs.getByRole('tab', { name: 'Brief', exact: true })).toBeFocused();
    await expect(tabs.getByRole('tab', { name: 'Brief', exact: true })).toHaveAttribute('aria-selected', 'true');
    await page.keyboard.press('ArrowRight'); await expect(coordinator).toBeFocused();
    await page.keyboard.press('ArrowLeft'); await expect(tabs.getByRole('tab', { name: 'Brief', exact: true })).toBeFocused();
    await page.keyboard.press('Home'); await expect(coordinator).toBeFocused();
    await expect(tabs.locator('[tabindex="0"]')).toHaveCount(1);
    await page.getByRole('navigation', { name: 'Attempts', exact: true }).getByRole('link', { name: /Reviewer · attempt 1/ }).click();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    mkdirSync('.crewroom/test-results/m64-slice-c-gates', { recursive: true });
    await page.screenshot({ path: '.crewroom/test-results/m64-slice-c-gates/history-narrow.png', fullPage: true });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.screenshot({ path: '.crewroom/test-results/m64-slice-c-gates/history-desktop.png', fullPage: true });
    await page.goto(`${app.origin}/#${fixture.run.taskId}?run=${fixture.foreign.id}&attempt=${fixture.oldAttemptId}`);
    await expect(page.getByText('That run is unavailable for this ticket. Showing the default run.')).toBeVisible();
    await expect(history).toHaveValue(fixture.next.id); await expect(detail).toHaveCount(0);
    await page.goto(`${app.origin}/#${fixture.run.taskId}?run=${fixture.run.id}&attempt=missing`);
    await expect(page.getByText('That attempt is unavailable in this run. Showing the agent wall.')).toBeVisible();
    await expect(wall).toBeVisible();
    // Creating a successor from history navigates to that new run, not the pinned older URL.
    await page.getByRole('button', { name: `Remove ${fixture.run.taskSnapshot.key} from queue`, exact: true }).click();
    await page.getByRole('button', { name: 'Queue research', exact: true }).click();
    await expect(history).not.toHaveValue(fixture.run.id); await expect(history).not.toHaveValue(fixture.next.id);
    await expect(history.locator('option:checked')).toContainText('research');
    await expect(page).toHaveURL(new RegExp(`run=${await history.inputValue()}`));
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});

test('HIS-05: an observation outage is labelled separately from recorded agent failures', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-observation-browser-')); const fixture = await historyFixture(root);
  const legacy = fixture.store.getRun(fixture.run.id); delete legacy.startedAt; legacy.launchCompletedAt = legacy.createdAt;
  (fixture.store as any).db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify(legacy), legacy.id);
  fixture.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
  try {
    await page.routeWebSocket('**/api/live', socket => {
      const server = socket.connectToServer();
      server.onMessage(message => {
        const frame = JSON.parse(String(message));
        if (frame.kind === 'state' && frame.topic === `run:${fixture.run.id}`) {
          frame.data.error = 'Fixture host launch failed';
          socket.send(JSON.stringify(frame));
          socket.send(JSON.stringify({ kind: 'error', topic: frame.topic, error: 'Fixture host observation timed out' }));
        } else socket.send(JSON.stringify(frame));
      });
    });
    await page.goto(`${app.origin}/#${fixture.run.taskId}?run=${fixture.run.id}`);
    await expect(page.getByText('Observation unavailable: Fixture host observation timed out. This does not establish an agent failure.')).toBeVisible();
    await expect(page.getByText(/Last controller error: Fixture host launch failed/)).toBeVisible();
    await expect(page.getByRole('combobox', { name: 'Run history' }).locator('option:checked')).toContainText('Start unknown');
    await page.getByRole('navigation', { name: 'Attempts', exact: true }).getByRole('link', { name: /Reporter · attempt 1 · retired/ }).click();
    const retired = page.getByRole('article', { name: 'Reporter attempt 1 details', exact: true });
    await expect(retired).toContainText('Fixture reporting complete'); await expect(retired).toContainText('It cannot resume.');
    await retired.getByRole('link', { name: 'Back to agent wall' }).click();
    await expect(page.getByRole('region', { name: 'Agent supervision' }).getByText('Cause unknown: no reason was recorded for this attempt.').first()).toBeVisible();
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});


test('RTE-09: fallback history and agent tiles keep each attempt\'s own model after reload', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-fallback-browser-'));
  const fixture = await historyFixture(root, { fallback: true }); fixture.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
  try {
    await page.goto(`${app.origin}/#${fixture.run.taskId}?run=${fixture.run.id}`);
    const wall = page.getByRole('region', { name: 'Agent supervision' });
    const reviewer = wall.locator('article.tile').filter({ has: page.getByRole('heading', { name: 'Reviewer', exact: true }) });
    await expect(reviewer).toContainText('fixture/alternate · low · Attempt 2');
    await expect(reviewer).toContainText('Configured fallback from fixture/model');
    await page.getByRole('navigation', { name: 'Attempts', exact: true }).getByRole('link', { name: /Reviewer · attempt 1/ }).click();
    const prior = page.getByRole('article', { name: 'Reviewer attempt 1 details', exact: true });
    await expect(prior).toContainText('Model: fixture/model · low');
    await page.reload(); await expect(prior).toContainText('Model: fixture/model · low');
    await page.getByRole('navigation', { name: 'Attempts', exact: true }).getByRole('link', { name: /Reviewer · attempt 2/ }).click();
    const alternate = page.getByRole('article', { name: 'Reviewer attempt 2 details', exact: true });
    await expect(alternate).toContainText('Model: fixture/alternate · low');
    await expect(alternate).toContainText('Configured fallback from fixture/model');
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});
