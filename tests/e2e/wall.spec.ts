import { test, expect } from '@playwright/test';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, appendFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Workbench } from '../../server/workbench';
import { seedRepository, saveRepositoryFixture } from '../fixtures/repository';
import { defaultRoles, RoleCatalog } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { startServer } from '../../server/app';
import { checkKey } from '../../server/live';

const hasTmux = spawnSync('tmux', ['-V']).status === 0;

test('the run view is a live, read-only agent wall with proposed specialists, live checks and explicit repair allowances', async ({ page }) => {
  test.skip(!hasTmux, 'tmux is not installed');
  const root = mkdtempSync(join(tmpdir(), 'crwall-')); const socket = join(root, 't.sock'); const runtimeRoot = join(root, 'runtime');
  seedRepository(root);
  const tmux = (...args: string[]) => execFileSync('tmux', ['-S', socket, '-f', '/dev/null', ...args], { encoding: 'utf8' }).trim();
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Scroll the investigator dialog', description: 'Source requirement', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Make the dialog scroll', criteria: ['Start run stays reachable'] });
  const run = store.queue(task.id, { revision: 2 });
  const catalog = [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }];
  const admit = (name: string, roleIndex: number, coordinator: boolean, brief?: { criteria: { index: number; criterion: string; obligation: string; checkIds: string[] }[]; dependsOn: string[] }) => {
    const role = { ...defaultRoles[roleIndex], model: 'fixture/model', effort: 'low' as const };
    return store.admitAssignment(run.id, { name, role, model: resolveAssignment(role, catalog, {}, {}), coordinator, input: 'Start', brief });
  };
  const coordinator = admit('Coordinator', 0, true);
  const implementer = admit('Implementer', 1, false, { criteria: [{ index: 0, criterion: 'Start run stays reachable', obligation: 'Make the dialog body scroll.', checkIds: ['modal-accessibility'] }], dependsOn: ['Coordinator'] });
  const rolePath = join(root, 'roles.json');
  new RoleCatalog(rolePath).update('implementer', { ...defaultRoles[1], revision: 1, model: 'fixture/changed-model', effort: 'high', instructions: 'New instructions for future assignments.' });
  const script = (label: string) => { const path = join(root, `${label}.sh`); writeFileSync(path, `printf "\\033[32m✓ host ready\\033[0m ${label}\\n"; i=0; while true; do echo "${label} working $i"; i=$((i+1)); sleep 0.2; done`); return path; };
  const panes = [coordinator, implementer].map((item, index) => {
    mkdirSync(join(runtimeRoot, item.attempt.id), { recursive: true });
    const pane = index === 0 ? tmux('new-session', '-d', '-s', 'crewroom', '-x', '80', '-y', '24', '-P', '-F', '#{pane_id}', 'sh', script('coordinator'))
      : tmux('new-window', '-d', '-t', 'crewroom', '-P', '-F', '#{pane_id}', 'sh', script('implementer'));
    const saved = store.getAttempt(item.attempt.id);
    (store as any).db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify({ ...saved, pane, ompPid: 4000 + index, hostPid: 3000 + index, ownership: 'held', state: index === 0 ? 'executing' : 'idle' }), item.attempt.id);
    return pane;
  });
  const exhausted = store.runtime(run.id).assignments.find(item => item.id === implementer.assignment.id)!;
  (store as any).db.prepare('UPDATE assignments SET data=? WHERE id=?').run(JSON.stringify({ ...exhausted, automaticCycles: 2 }), exhausted.id);
  store.startExecution(run.id);
  const writerRole = { ...defaultRoles.find(role => role.id === 'tech-writer')!, model: 'fixture/model', effort: 'low' as const };
  const rejected = store.proposeTeam(coordinator.attempt.id, 'rejected-1', [{ name: 'Writer', role: writerRole, model: resolveAssignment(writerRole, catalog, {}, {}), input: 'Document the change' }]);
  store.rejectUnavailableTeamProposal(rejected.id, [{ ...writerRole, enabled: false }]);
  const reporterRole = { ...defaultRoles.find(role => role.id === 'delivery-reporter')!, model: 'fixture/model', effort: 'low' as const };
  store.proposeTeam(coordinator.attempt.id, 'propose-1', [{ name: 'Reporter', role: reporterRole, model: resolveAssignment(reporterRole, catalog, {}, {}), input: 'Post progress' }]);
  const checkRoot = join(runtimeRoot, implementer.attempt.id, 'tools', checkKey('check-call')); mkdirSync(checkRoot, { recursive: true });
  writeFileSync(join(checkRoot, 'live.log'), 'Running 4 tests using 1 worker\n');
  store.recordAgentEvent(implementer.attempt.id, 1, { type: 'tool_started', toolCallId: 'check-call', commandId: 'modal-accessibility' });
  store.recordAgentEvent(coordinator.attempt.id, 1, { type: 'message_end', message: { role: 'assistant', timestamp: Date.now(), content: [{ type: 'text', text: 'Candidate **b4febc06** recorded.\n\n| Check | Result |\n| --- | --- |\n| typecheck | Passed |' }] } });
  store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath, observation: { socket, root: runtimeRoot } });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(app.origin + '/#' + task.id);
    const wall = page.getByRole('region', { name: 'Agent supervision' });
    await expect(wall.getByRole('heading', { name: 'Coordinator', exact: true })).toBeVisible();
    await expect(wall.getByRole('heading', { name: 'Implementer', exact: true })).toBeVisible();
    await expect(wall.getByRole('heading', { name: 'Reporter', exact: true })).toBeVisible();
    await expect(wall.getByText('Proposed · not started', { exact: true })).toBeVisible();
    const rejectedTile = wall.getByRole('article', { name: 'Rejected team proposal' });
    await expect(rejectedTile.getByText('Rejected · not admitted')).toBeVisible();
    await expect(rejectedTile.getByText(/Selected role Technical writer is disabled or unavailable/)).toBeVisible();
    await expect(rejectedTile.getByText(/No specialists from this proposal were admitted/)).toBeVisible();
    await expect(wall.getByRole('heading', { name: 'Check · modal-accessibility' })).toBeVisible();

    // Terminals stream: bytes keep arriving without any browser polling.
    const coordinatorTerminal = wall.getByRole('img', { name: 'Coordinator terminal output (read-only)' });
    await expect(coordinatorTerminal).toHaveAttribute('data-state', 'live');
    const received = async () => Number(await coordinatorTerminal.getAttribute('data-received'));
    const before = await received();
    const requests: string[] = []; page.on('request', request => { if (request.url().includes('/api/')) requests.push(request.url()); });
    await expect.poll(received, { timeout: 5000 }).toBeGreaterThan(before);
    await page.waitForTimeout(2500);
    expect(requests, 'An idle run view makes no HTTP requests: updates arrive over the push channel.').toEqual([]);
    const check = wall.getByRole('img', { name: 'Check modal-accessibility terminal output (read-only)' });
    await expect(check).toHaveAttribute('data-state', 'live');
    const checkBefore = Number(await check.getAttribute('data-received'));
    appendFileSync(join(checkRoot, 'live.log'), '  ✓ 1280×720  scrolling exposes Start run\n');
    await expect.poll(async () => Number(await check.getAttribute('data-received'))).toBeGreaterThan(checkBefore);

    // Coordinator replies render Markdown instead of raw syntax.
    await expect(page.getByRole('tabpanel').locator('strong', { hasText: 'b4febc06' })).toBeVisible();
    await expect(page.getByRole('tabpanel').getByRole('cell', { name: 'Passed' })).toBeVisible();
    await page.evaluate(() => scrollTo(0, 0)); await page.screenshot({ path: '.crewroom/test-results/wall-desktop.png', fullPage: true });

    // Exhausted automatic cycles need an explicit, recorded allowance.
    await expect(wall.getByText(/Automatic repair cycles used: 2 \/ 2/)).toBeVisible();
    await wall.getByRole('button', { name: 'Allow another repair cycle…' }).click();
    await wall.getByLabel(/Why allow another automatic cycle for Implementer/).fill('The browser fixture was at fault.');
    await wall.getByRole('button', { name: 'Allow one more cycle' }).click();
    await expect(page.getByRole('status')).toContainText('One more automatic cycle allowed for Implementer.');
    await expect(wall.getByText(/Automatic repair cycles used/)).toHaveCount(0);

    // The structured brief snapshotted at admission is visible on the agent tile.
    const implementerTile = wall.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Implementer', exact: true }) });
    await implementerTile.getByText('Brief', { exact: true }).click();
    await expect(implementerTile.getByText('Criterion 1: Start run stays reachable')).toBeVisible();
    await expect(implementerTile.getByText('Make the dialog body scroll.')).toBeVisible();
    await expect(implementerTile.getByText('Checks: modal-accessibility')).toBeVisible();
    await expect(implementerTile.getByText('Depends on: Coordinator')).toBeVisible();
    await implementerTile.getByText('Assignment definition').focus();
    await page.keyboard.press('Enter');
    await expect(implementerTile.getByRole('definition').filter({ hasText: 'fixture/model · low' })).toHaveCount(2);
    await expect(implementerTile.getByText(/Current definition differs: revision 2.*retains revision 1/)).toBeVisible();
    await expect(implementerTile.getByText(/Make the smallest complete change that meets the accepted brief/)).toBeVisible();
    await expect(implementerTile.getByText('New instructions for future assignments.')).toHaveCount(0);

    // Focus one agent, then return to the wall.
    await wall.getByRole('button', { name: 'Enlarge Implementer' }).click();
    await expect(wall.getByRole('heading', { name: 'Coordinator', exact: true })).toHaveCount(0);
    await wall.getByRole('button', { name: 'Show all agents' }).click();
    await expect(wall.getByRole('heading', { name: 'Coordinator', exact: true })).toBeVisible();

    // Viewer reconnect: the pane kept running and the new view is live again.
    await page.reload();
    await expect(page.getByRole('img', { name: 'Implementer terminal output (read-only)' })).toHaveAttribute('data-state', 'live');
    expect(tmux('capture-pane', '-p', '-t', panes[1])).toContain('implementer working');

    await page.setViewportSize({ width: 390, height: 844 });
    const narrowTile = wall.getByRole('article').filter({ has: page.getByRole('heading', { name: 'Implementer', exact: true }) });
    await narrowTile.getByText('Assignment definition').focus();
    await page.keyboard.press('Enter');
    await expect(narrowTile.getByText(/Current definition differs: revision 2/)).toBeVisible();
    const wide = await page.evaluate(() => [...document.querySelectorAll('body *')].filter(el => !el.closest('.terminal-frame')).filter(el => el.getBoundingClientRect().right > innerWidth + 1).slice(0, 8).map(el => `${el.tagName}.${el.className} ${Math.round(el.getBoundingClientRect().right)}`));
    expect(wide, 'No element extends past a narrow viewport.').toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: '.crewroom/test-results/wall-narrow.png', fullPage: true });
  } finally { await app.close(); try { tmux('kill-server'); } catch {} rmSync(root, { recursive: true, force: true }); }
});
