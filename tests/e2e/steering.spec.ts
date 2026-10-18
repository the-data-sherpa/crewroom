import { test, expect } from '@playwright/test';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { steeringFixture } from '../fixtures/steering';
import { startServer } from '../../server/app';

const evidence = '.crewroom/test-results/m64-slice-d-gates';
test('STE-01/02/05: steering states, recorded times and replies stay distinct; paused composer works by keyboard at 390 px', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-steering-browser-')); const f = steeringFixture(root);
  const notes = ['Stored direction', 'Unconfirmed direction', 'Received direction', 'Applied direction', 'Rejected direction'];
  for (const [index, body] of notes.entries()) {
    const note = f.store.steer(f.run.id, { requestId: String(index), body });
    if (index === 0) continue;
    f.tick(); f.store.dispatchSteering(f.attempt.id, [note.id], `input-${index}`);
    if (index === 1) continue;
    f.tick(); f.store.receiveSteering(f.attempt.id, `input-${index}`);
    if (index > 2) { f.tick(); f.store.applySteering(f.attempt.id, `receipt-${index}`, { messageId: note.id, disposition: index === 3 ? 'applied' : 'rejected', reason: index === 3 ? 'Added a check to the plan.' : 'Outside the accepted scope.' }); }
  }
  f.store.recordAgentEvent(f.attempt.id, 1, { type: 'message_end', message: { role: 'assistant', timestamp: f.clock(), content: [{ type: 'text', text: 'I have applied everything and finished the work.' }] } });
  f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id); f.store.recordOwnership(f.attempt.id, true); f.store.finishPause(f.run.id);
  f.store.close(); const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
  try {
    await page.setViewportSize({ width: 390, height: 844 }); await page.goto(`${app.origin}/#${f.run.taskId}`);
    const conversation = page.getByRole('region', { name: 'Coordinator conversation' });
    const message = (body: string) => conversation.locator('li.operator-message').filter({ has: page.getByText(body, { exact: true }) });
    await expect(message(notes[0])).toContainText('Stored · Not delivered');
    await expect(message(notes[1])).toContainText('Dispatched · Receipt unconfirmed');
    await expect(message(notes[2])).toContainText('Received by coordinator · Not yet applied');
    await expect(message(notes[3])).toContainText('Applied to the plan');
    await expect(message(notes[3])).toContainText('This does not confirm completed work.');
    await expect(message(notes[4])).toContainText('Rejected by coordinator');
    await expect(message(notes[4])).toContainText('Outside the accepted scope.');
    for (const [index, body] of notes.entries()) {
      await expect(message(body).locator('.steering-times time')).toHaveCount(index === 0 ? 1 : index === 1 ? 2 : index === 2 ? 3 : 4);
      await expect(message(body).locator('.steering-times time').first()).toHaveAttribute('datetime', /2026-10-01T/);
    }
    await expect(conversation.getByText('I have applied everything and finished the work.')).toBeVisible();
    await expect(conversation.getByText('Coordinator reply · Steering application requires a recorded disposition.')).toBeVisible();
    await expect(page.getByText('Saved now; delivered after Resume.')).toBeVisible();
    const composer = page.getByLabel('Message to coordinator'); await expect(composer).toBeEnabled();
    await composer.focus(); await page.keyboard.type('Keyboard steering'); await page.keyboard.press('Control+Enter');
    await expect(message('Keyboard steering')).toContainText('Stored · Not delivered'); await expect(composer).toHaveValue('');
    await page.reload(); await expect(message('Keyboard steering')).toContainText('Stored · Not delivered');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    mkdirSync(evidence, { recursive: true }); await page.screenshot({ path: `${evidence}/steering-narrow.png`, fullPage: true });
    await page.setViewportSize({ width: 1440, height: 1000 }); await page.screenshot({ path: `${evidence}/steering-desktop.png`, fullPage: true });
    const cancelled = await page.request.post(`${app.origin}/api/runs/${f.run.id}/cancel`, { headers: { Origin: app.origin }, data: {} });
    expect(cancelled.ok()).toBe(true);
    await expect(composer).toBeDisabled(); await expect(page.getByText('This run is cancelled. New steering is unavailable.')).toBeVisible();
    await expect(message('Keyboard steering').getByText('Not delivered', { exact: true }).first()).toBeVisible();
    await expect(message(notes[1])).toContainText('Dispatched · Receipt unconfirmed');
    await expect(message(notes[2])).toContainText('Received by coordinator · Not yet applied');
  } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
});

test('STE-04: closed and completed runs explain disabled steering; legacy transition times stay unknown', async ({ page }) => {
  for (const status of ['closed', 'completed'] as const) {
    const root = mkdtempSync(join(tmpdir(), 'crewroom-terminal-steering-')); const f = steeringFixture(root);
    const old = f.store.steer(f.run.id, { requestId: 'old', body: 'Legacy received message' });
    f.store.recordOwnership(f.attempt.id, true); f.store.close();
    const db = new DatabaseSync(f.path);
    db.prepare('UPDATE steering SET data=? WHERE id=?').run(JSON.stringify({ ...old, delivery: 'received', transitions: undefined }), old.id);
    db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify({ ...f.run, status, desiredState: 'cancelled', ...(status === 'completed' ? { purpose: 'research' } : {}) }), f.run.id);
    db.close();
    const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json') });
    try {
      await page.goto(`${app.origin}/#${f.run.taskId}`);
      await expect(page.getByLabel('Message to coordinator')).toBeDisabled();
      await expect(page.getByText(status === 'closed' ? 'This run is closed. New steering is unavailable.' : 'This research run is completed. New steering is unavailable.')).toBeVisible();
      const message = page.getByRole('region', { name: 'Coordinator conversation' }).locator('li.operator-message');
      await expect(message).toContainText('Received by coordinator · Not yet applied');
      await expect(message.getByText('Time unknown', { exact: true })).toHaveCount(2);
      const response = await page.request.post(`${app.origin}/api/runs/${f.run.id}/steering`, { headers: { Origin: app.origin }, data: { requestId: 'late', body: 'Late instruction' } });
      expect(response.status()).toBe(409);
    } finally { await app.close(); rmSync(root, { recursive: true, force: true }); }
  }
});
