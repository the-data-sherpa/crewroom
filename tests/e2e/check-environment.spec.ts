import { test, expect } from '@playwright/test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { publicationFixture } from '../fixtures/publication';
import { startServer } from '../../server/app';

test('correcting a paused check environment shows stale evidence and the recorded reason', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-image-browser-')); const f = (await publicationFixture(root));
  f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id);
  for (const attempt of f.store.runtime(f.run.id).attempts) f.store.recordOwnership(attempt.id, true);
  f.store.finishPause(f.run.id); await f.work.reconcileStopped(f.run.id);
  const check = f.store.codingRecord(f.run.id)!.requiredChecks[0]; f.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
  try {
    const response = await page.request.post(app.origin + `/api/runs/${f.run.id}/check-environment`, { headers: { Origin: app.origin }, data: {
      requestId: 'correct-fixture', checkId: check.id, expectedImage: check.image, image: `sha256:${'b'.repeat(64)}`, reason: 'Correct overlong browser fixture input.' } });
    expect(response.ok()).toBe(true);
    await page.goto(app.origin + '/#' + f.run.taskId);
    await page.getByRole('tab', { name: 'Change & evidence' }).click();
    await expect(page.getByText('Validation incomplete', { exact: true })).toBeVisible();
    await expect(page.getByText('Out of date', { exact: true })).toBeVisible();
    await expect(page.getByText('Review predates the current check environment.', { exact: true })).toBeVisible();
    await page.getByText('Check environment changes', { exact: true }).click();
    await expect(page.getByText('Correct overlong browser fixture input.', { exact: true })).toBeVisible();
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});
