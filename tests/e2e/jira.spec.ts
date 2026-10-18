import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startServer } from '../../server/app';

test('Jira refresh shows saved-filter order, outages and retained work without queuing tickets', async ({ page }) => {
  let offline = false, empty = false;
  const provider = createServer((_req, res) => {
    if (offline) { res.statusCode = 503; res.end('fixture error'); return; }
    res.end(JSON.stringify({ isLast: true, issues: empty ? [] : ['2', '1'].map(id => ({ id, key: `SIM-${id}`, fields: { summary: `Imported ${id}`, description: 'Source requirement', status: { name: 'To Do' } } })) }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const root = mkdtempSync(join(tmpdir(), 'crewroom-jira-browser-'));
  const app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json'), production: true,
    jira: { source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator-1', name: 'Assigned work' },
      apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'browser-synthetic-secret' } });
  try {
    await page.goto(app.origin);
    await expect(page.getByText('Not refreshed yet', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Refresh Jira', exact: true }).click();
    await expect(page.getByRole('navigation', { name: 'Tickets' }).getByRole('link').first()).toContainText('SIM-2');
    await page.getByRole('navigation', { name: 'Tickets' }).getByRole('link').first().click();
    await page.getByRole('textbox', { name: 'Local brief', exact: true }).fill('Keep my local refinement');
    await page.getByLabel('Acceptance criteria').fill('Actions remain accessible');
    await page.getByRole('button', { name: 'Save brief', exact: true }).click();
    await expect(page.getByRole('status')).toContainText('Brief saved');
    offline = true; await page.getByRole('button', { name: 'Refresh Jira', exact: true }).click();
    await expect(page.getByText(/Jira returned HTTP 503/)).toBeVisible();
    await expect(page.getByRole('textbox', { name: 'Local brief', exact: true })).toHaveValue('Keep my local refinement');
    await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
    const workspace = await (await page.request.get(app.origin + '/api/workspace')).json();
    expect(workspace.runs).toEqual([]); expect(JSON.stringify(workspace)).not.toContain('browser-synthetic-secret');
    offline = false; empty = true; await page.getByRole('button', { name: 'Refresh Jira', exact: true }).click();
    await expect(page.getByText('This ticket is outside the current Jira filter. Saved work is retained.', { exact: true })).toBeVisible();
    await page.reload(); await expect(page.getByRole('textbox', { name: 'Local brief', exact: true })).toHaveValue('Keep my local refinement');
  } finally { await app.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); }
});
