import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import type { Workspace } from '../../shared/types';

async function fixture() {
  const root = join(tmpdir(), `crb-${randomUUID().slice(0, 8)}`);
  const child = spawn('python3', ['scripts/test-application-continuity.py', '--scenario', 'refinement-browser', '--output', root], {
    env: { ...process.env, PATH: `/opt/homebrew/opt/node/bin:${process.env.PATH}` }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = ''; child.stdout.on('data', chunk => { output += chunk; }); child.stderr.on('data', chunk => { output += chunk; });
  await expect.poll(() => existsSync(join(root, 'ready.json')) || child.exitCode !== null, { timeout: 25000 }).toBe(true);
  if (!existsSync(join(root, 'ready.json'))) throw new Error(`Browser fixture failed: ${output}`);
  const origin: string = JSON.parse(readFileSync(join(root, 'ready.json'), 'utf8')).origin;
  return { origin, root, async close() {
    writeFileSync(join(root, 'stop'), 'stop');
    if (child.exitCode === null) await once(child, 'exit');
    if (child.exitCode !== 0 || !existsSync(join(root, 'report.json'))) throw new Error(`Browser fixture failed: ${output}`);
    const report = JSON.parse(readFileSync(join(root, 'report.json'), 'utf8'));
    expect(report).toMatchObject({ passed: true, providerRequests: 5, unexpectedSurvivingProcesses: [] });
  } };
}

test('INT-03: direct queued coding starts automatically while Explore and Grill retain decisions independently', async ({ page }) => {
  test.setTimeout(120000);
  const app = await fixture();
  let failed = false;
  try {
    await page.goto(app.origin);
    await page.getByRole('button', { name: 'Repositories', exact: true }).click();
    await page.getByLabel('Repository name').fill('Coding fixture');
    await page.getByLabel('Checkout path').fill(join(app.root, 'repository'));
    await page.getByRole('button', { name: 'Add repository' }).click();
    await page.getByRole('button', { name: 'New ticket' }).click();
    await page.getByLabel('Ticket title').fill('Direct Run');
    await page.getByRole('button', { name: 'Create ticket' }).click();
    await page.getByRole('combobox', { name: 'Repository', exact: true }).selectOption({ label: 'Coding fixture' });
    await page.getByRole('button', { name: 'Save target' }).click();
    await page.getByLabel('Local brief').fill('Directly implement the request');
    await page.getByLabel('Acceptance criteria').fill('Direct path is available');
    await page.getByRole('button', { name: 'Save brief' }).click();
    await page.getByRole('button', { name: 'Queue run' }).click();
    await expect(page.getByText('Run adopted revision 3')).toBeVisible();
    const directId = (await (await page.request.get(app.origin + '/api/workspace')).json() as Workspace).tasks.find(item => item.title === 'Direct Run')!.id;
    const codingRun = (await (await page.request.get(app.origin + '/api/workspace')).json() as Workspace).runs.find(run => run.taskId === directId)!;
    await expect.poll(async () => JSON.stringify(await (await page.request.get(`${app.origin}/api/runs/${codingRun.id}/events?after=0`)).json()).includes('CODING_ACTIVE'), { timeout: 35000 }).toBe(true);

    await page.getByRole('button', { name: 'New ticket' }).click();
    await page.getByLabel('Ticket title').fill('Explore ticket');
    await page.getByRole('button', { name: 'Create ticket' }).click();
    await page.getByRole('button', { name: 'Explore', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Explore proposal · pending' })).toBeVisible({ timeout: 20000 });
    await page.getByLabel('Edit before accepting').check();
    await page.getByLabel('Proposed brief').fill('Accepted and edited Explore brief');
    await page.getByRole('button', { name: 'Accept proposal' }).click();
    await expect(page.getByRole('heading', { name: 'Explore proposal · accepted' })).toBeVisible();
    await page.getByRole('button', { name: 'Close refinement' }).click();
    await expect(page.getByRole('status')).toContainText('Refinement closed.');
    await expect(page.getByText('Explore based on revision 1.')).toBeVisible();
    await page.reload();
    await expect(page.getByText('Explore decision').first()).toBeVisible();
    await expect(page.getByText('Explore question?').first()).toBeVisible();
    await page.getByRole('tab', { name: 'Brief' }).click();
    await expect(page.getByLabel('Local brief')).toHaveValue('Accepted and edited Explore brief');
    await expect(page.getByText('Revision 2 · Current accepted')).toBeVisible();
    const exploredId = (await (await page.request.get(app.origin + '/api/workspace')).json() as Workspace).tasks.find(item => item.title === 'Explore ticket')!.id;

    await page.getByRole('button', { name: 'New ticket' }).click();
    await page.getByLabel('Ticket title').fill('Grill ticket');
    await page.getByRole('button', { name: 'Create ticket' }).click();
    await page.getByRole('button', { name: 'Grill', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Grill proposal · pending' })).toBeVisible({ timeout: 20000 });
    await expect(page.getByRole('button', { name: 'Discard proposal' })).toBeDisabled();
    await page.getByLabel('Decision reason (required to discard)').fill('Need evidence before deciding.');
    await page.getByRole('button', { name: 'Discard proposal' }).click();
    await page.getByRole('button', { name: 'Close refinement' }).click();
    await expect(page.getByRole('status')).toContainText('Refinement closed.');
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Grill proposal · discarded' })).toBeVisible();
    await expect(page.getByText('Grill question?', { exact: true })).toBeVisible();
    await expect(page.getByText('Operator decision: Need evidence before deciding.')).toBeVisible();
    const workspace = await (await page.request.get(app.origin + '/api/workspace')).json() as Workspace;
    expect(workspace.runs.find(run => run.taskId === directId)?.status).toBe('running');
    for (const id of [exploredId, workspace.tasks.find(item => item.title === 'Grill ticket')!.id]) {
      const run = workspace.runs.find(item => item.taskId === id)!;
      expect(run.status).toBe('closed');
      const runtime = await (await page.request.get(`${app.origin}/api/runs/${run.id}/runtime`)).json();
      expect(runtime.assignments).toHaveLength(1); expect(runtime.attempts).toHaveLength(1);
      expect(runtime.attempts[0].ownership).toBe('stopped');
      expect((await page.request.get(`${app.origin}/api/runs/${run.id}/repository`)).status()).toBe(404);
    }
    await page.screenshot({ path: join(app.root, 'refinement.png'), fullPage: true });
  } catch (error) { failed = true; throw error; }
  finally { try { await app.close(); } catch (error) { if (!failed) throw error; } }
});
