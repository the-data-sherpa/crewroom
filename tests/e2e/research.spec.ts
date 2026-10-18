import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

async function fixture() {
  const root = join(tmpdir(), `cr-research-browser-${randomUUID().slice(0, 8)}`);
  const child = spawn('python3', ['scripts/test-application-continuity.py', '--scenario', 'research-browser', '--output', root], {
    env: { ...process.env, PATH: `/opt/homebrew/opt/node/bin:${process.env.PATH}` }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = ''; child.stdout.on('data', chunk => { output += chunk; }); child.stderr.on('data', chunk => { output += chunk; });
  await expect.poll(() => existsSync(join(root, 'ready.json')) || child.exitCode !== null, { timeout: 25000 }).toBe(true);
  if (!existsSync(join(root, 'ready.json'))) throw new Error(`Research browser fixture did not start: ${output}`);
  const origin: string = JSON.parse(readFileSync(join(root, 'ready.json'), 'utf8')).origin;
  return { origin, root, async close() {
    writeFileSync(join(root, 'stop'), 'stop');
    if (child.exitCode === null) await once(child, 'exit');
    if (child.exitCode !== 0 || !existsSync(join(root, 'report.json'))) throw new Error(`Research browser fixture failed: ${output}`);
    const report = JSON.parse(readFileSync(join(root, 'report.json'), 'utf8'));
    expect(report).toMatchObject({ passed: true, providerRequests: 15, unexpectedSurvivingProcesses: [] });
  } };
}

test('INT-07/EVD-08: reloaded research results retain citations, limitations, coverage and rejected history without coding sections', async ({ page }) => {
  test.setTimeout(120000);
  const app = await fixture(); let failed = false;
  const post = async (path: string, body: unknown) => {
    const response = await page.request.post(`${app.origin}/api${path}`, { data: body, headers: { Origin: app.origin } });
    expect(response.ok(), `${path}: ${await response.text()}`).toBe(true);
    return response.json();
  };
  try {
    const repository = await post('/repositories', { name: 'Queue source', path: join(app.root, 'repository'), targetBranch: 'main' });
    const task = await post('/tasks', { title: 'Compare queue designs', description: 'Use checked-in fictional examples' });
    await post(`/tasks/${task.id}/repository`, { revision: 1, repositoryId: repository.id });
    await post(`/tasks/${task.id}/refinement`, { revision: 2, sourceRevision: 1, brief: 'Compare fictional FIFO and priority queues.', criteria: ['Compare both queue options', 'Define decision criteria and limitations'] });
    await page.goto(`${app.origin}/#${task.id}`);
    await page.getByRole('button', { name: 'Queue research' }).click();
    await expect(page.getByText('Research queued. Start the coordinator when ready.')).toBeVisible();
    await page.getByLabel('Coordinator role').selectOption('triage');
    await page.getByRole('button', { name: 'Start coordinator' }).click();
    const run = (await (await page.request.get(`${app.origin}/api/workspace`)).json()).runs.find((item: { taskId: string }) => item.taskId === task.id);
    await expect.poll(async () => (await (await page.request.get(`${app.origin}/api/runs/${run.id}/research`)).json()).artifacts.length, { timeout: 30000 }).toBe(2);
    await page.getByRole('tab', { name: 'Research evidence' }).click();
    await expect(page.getByRole('heading', { name: 'Research outcome and evidence' })).toBeVisible();
    await expect(page.getByText('Research incomplete', { exact: true })).toBeVisible();
    await expect(page.getByText('Rejected citation', { exact: true })).toBeVisible();
    await post(`/runs/${run.id}/steering`, { requestId: randomUUID(), body: 'Try completion; do not claim a source-less finding.' });
    await expect.poll(async () => JSON.stringify((await (await page.request.get(`${app.origin}/api/runs/${run.id}/events?limit=200`)).json()).events).includes('INCOMPLETE_HANDOFF'), { timeout: 15000 }).toBe(true);
    await post(`/runs/${run.id}/steering`, { requestId: randomUUID(), body: 'Request a correction from Architect.' });
    await expect.poll(async () => (await (await page.request.get(`${app.origin}/api/runs/${run.id}/research`)).json()).artifacts.length, { timeout: 15000 }).toBe(3);
    await post(`/runs/${run.id}/steering`, { requestId: randomUUID(), body: 'Complete the verified outcome.' });
    await expect.poll(async () => (await (await page.request.get(`${app.origin}/api/workspace`)).json()).runs.find((item: { id: string }) => item.id === run.id).status, { timeout: 30000 }).toBe('completed');
    await page.reload();
    await page.getByRole('tab', { name: 'Research evidence' }).click();
    const outcome = page.getByRole('region', { name: 'Research outcome and evidence' });
    await expect(outcome.getByText('Completed research outcome', { exact: true })).toBeVisible();
    await expect(outcome.getByRole('heading', { name: 'Queue design assessment · design' })).toHaveCount(2);
    await expect(outcome.getByText('Verified citation', { exact: true })).toHaveCount(2);
    await expect(outcome.getByText('Rejected citation', { exact: true })).toHaveCount(1);
    await expect(outcome.getByText('Fixture-only comparison; no production workloads.')).toHaveCount(2);
    await expect(outcome.getByText('Criterion 0 → Comparison')).toHaveCount(2);
    await expect(outcome.getByText('Criterion 1 → Decision criteria')).toBeVisible();
    await expect(outcome.getByText('FIFO is predictable; priority may starve lower-ranked work.')).toHaveCount(2);
    await expect(outcome.getByText('Superseded by artifact')).toBeVisible();
    await expect(outcome.getByText('SHA-256').first()).toBeVisible();
    await expect(outcome.getByText('pinned commit', { exact: false }).first()).toBeVisible();
    await expect(outcome.getByRole('heading', { name: 'Repository checks' })).toHaveCount(0);
    await expect(outcome.getByRole('heading', { name: 'Independent review' })).toHaveCount(0);
    await expect(outcome.getByText('Candidate commit')).toHaveCount(0);
    await expect(outcome.getByText('Pull request')).toHaveCount(0);
    await page.screenshot({ path: join(app.root, 'research-results.png'), fullPage: true });
  } catch (error) { failed = true; throw error; }
  finally { try { await app.close(); } catch (error) { if (!failed) throw error; } }
});
