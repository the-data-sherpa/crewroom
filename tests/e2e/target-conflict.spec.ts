import { test, expect } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { publicationFixture } from '../fixtures/publication';
import { startServer } from '../../server/app';

test('a paused target conflict shows the held branch, retained workspace and explicit continuation gate', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-conflict-browser-'));
  const f = await publicationFixture(root);
  let app: { origin: string; close(): Promise<void> } | undefined;
  try {
    const other = join(root, 'other');
    f.git(root, 'clone', '-q', f.remote, other);
    f.git(other, 'config', 'user.name', 'Other'); f.git(other, 'config', 'user.email', 'other@example.invalid');
    writeFileSync(join(other, 'answer'), 'different\n'); f.git(other, 'add', 'answer');
    f.git(other, 'commit', '-qm', 'Move target'); f.git(other, 'push', '-q', 'origin', 'main');
    f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id);
    for (const item of f.store.runtime(f.run.id).attempts) f.store.recordOwnership(item.id, true);
    f.store.finishPause(f.run.id);
    const targetRef = `refs/crewroom/${f.run.id}/target`;
    f.git(f.owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${targetRef}`);
    const result = await f.work.refreshBase(f.run.id, randomUUID(), targetRef, f.candidate.id);
    expect(result.refresh?.state).toBe('conflicted');
    f.store.close();
    // Exercise the current results renderer; a prebuilt dist can belong to a different source snapshot.
    app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json') });
    await page.goto(app.origin + '/#' + f.run.taskId);
    await page.getByRole('tab', { name: 'Change & evidence' }).click();
    await expect(page.getByText('Validation incomplete', { exact: true })).toBeVisible();
    await expect(page.getByText('Conflicted · branch unchanged', { exact: true })).toBeVisible();
    await expect(page.getByText('Conflicts: answer', { exact: true })).toBeVisible();
    await expect(page.getByText('Resolution workspace retained. Resume the run to let the coordinator assign an in-scope specialist; the original branch and PR remain unchanged until promotion.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Refresh from target' })).toHaveCount(0);
  } finally { await app?.close(); rmSync(root, { recursive: true, force: true }); }
});

test('a target change to project instructions holds a conflict for the operator', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-instruction-conflict-browser-'));
  const f = await publicationFixture(root);
  const publication = { repositoryPath: f.checkout, fixtureGitPath: f.remote,
    repository: { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' },
    policy: { statusKeys: ['unit'], pipelineSteps: ['Unit Tests'], optionalSteps: [] },
    apiUrl: 'http://127.0.0.1:9', credentials: async () => 'synthetic' };
  let app: { origin: string; close(): Promise<void> } | undefined;
  try {
    const other = join(root, 'other');
    f.git(root, 'clone', '-q', f.remote, other);
    f.git(other, 'config', 'user.name', 'Other'); f.git(other, 'config', 'user.email', 'other@example.invalid');
    writeFileSync(join(other, 'answer'), 'different\n'); writeFileSync(join(other, 'AGENTS.md'), 'Changed project instructions\n');
    f.git(other, 'add', '.'); f.git(other, 'commit', '-qm', 'Move target and instructions'); f.git(other, 'push', '-q', 'origin', 'main');
    f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id);
    for (const item of f.store.runtime(f.run.id).attempts) f.store.recordOwnership(item.id, true);
    f.store.finishPause(f.run.id);
    const targetRef = `refs/crewroom/${f.run.id}/target`;
    f.git(f.owned.path, 'fetch', '-q', f.remote, `+refs/heads/main:${targetRef}`);
    const result = await f.work.refreshBase(f.run.id, randomUUID(), targetRef, f.candidate.id);
    expect(result.refresh?.state).toBe('conflicted');
    expect(result.refresh?.resolutionWorkspace?.state).toBe('operator_required');
    f.store.close();
    app = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json'), publication: [publication] });
    await page.goto(app.origin + '/#' + f.run.taskId);
    await page.getByRole('tab', { name: 'Change & evidence' }).click();
    await expect(page.getByText('Validation incomplete', { exact: true })).toBeVisible();
    await expect(page.getByText('Conflicts: answer', { exact: true })).toBeVisible();
    await expect(page.getByText('Operator decision required: the conflict affects project instructions, exceeds confirmed scope or its Git state changed. The original branch and PR remain unchanged.', { exact: true })).toBeVisible();
    const retry = page.getByRole('button', { name: 'Retry from current target' });
    await expect(retry).toBeDisabled();
    // The operator restores the local bare target to the accepted base before verifying a fresh retry.
    f.git(other, 'reset', '--hard', f.candidate.revision.base);
    f.git(other, 'push', '--force', 'origin', 'main');
    const reason = page.getByLabel('Reason for a fresh target attempt');
    await expect(reason).toBeVisible(); await reason.fill('Reviewed the target instruction change');
    await expect(retry).toBeEnabled();
    await retry.click();
    await expect(page.getByText('Current target verified; retry recorded without rebasing or creating a candidate.', { exact: true })).toBeVisible();
    // Wait for the refreshed receipt before opening its disclosure; the success toast precedes the async result reload.
    const refreshHistory = page.locator('details').filter({ has: page.getByText('Target refreshes', { exact: true }) });
    await expect(refreshHistory).not.toHaveAttribute('open');
    await page.getByText('Target refreshes', { exact: true }).click();
    await expect(page.getByText('The reasoned retry verified the unchanged target and closed the prior hold. No rebase or new candidate was needed; the retry did not resume the run or change evidence gates.', { exact: true })).toBeVisible();
    await expect(page.getByText('An operator chose a fresh target attempt: Reviewed the target instruction change. The isolated workspace remains in history.', { exact: true })).toBeVisible();
    await expect(retry).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Refresh from target' })).toHaveCount(1);
  } finally { await app?.close(); rmSync(root, { recursive: true, force: true }); }
});
