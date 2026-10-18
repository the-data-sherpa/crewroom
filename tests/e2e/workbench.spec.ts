import { test, expect } from '@playwright/test';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { launch } from './process-fixture';
import { startServer } from '../../server/app';
import { Workbench } from '../../server/workbench';
import { seedRepository, saveRepositoryFixture } from '../fixtures/repository';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import { RepositoryWork } from '../../server/repository-work';
import type { PublicationRecord } from '../../shared/publication';

test('local ticket and brief persist while unavailable execution remains explicit', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'New ticket' }).click();
  await page.getByLabel('Ticket title').fill('Handle zero inputs');
  await page.getByLabel('Description', { exact: true }).fill('A local coding request');
  await page.getByRole('button', { name: 'Create ticket' }).click();
  await expect(page.getByRole('heading', { name: 'Handle zero inputs' })).toBeVisible();
  await page.getByLabel('Local brief').fill('Return zero for empty input');
  await page.getByLabel('Acceptance criteria').fill('Empty input returns zero\nExisting inputs still work');
  await page.getByRole('button', { name: 'Save brief' }).click();
  await expect(page.getByRole('status')).toContainText('Brief saved');
  await page.reload();
  await expect(page.getByLabel('Local brief')).toHaveValue('Return zero for empty input');
  await expect(page.getByText('Not linked', { exact: true })).toBeVisible();
  await expect(page.getByRole('definition').filter({ hasText: /^Refining$/ })).toBeVisible();
  await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
  await expect(page.getByText('Runner unavailable', { exact: true })).toBeVisible();
  await page.screenshot({ path: '.crewroom/test-results/m1/workspace.png', fullPage: true });
});
test('QUE-05: queue panel reorders two captured plans across reload and removes without starting agents', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-queue-browser-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const tasks = ['First queued task', 'Second queued task'].map(title => {
    const task = store.createTask({ title, description: '', repositoryId: repository.id });
    return store.refine(task.id, { revision: task.revision, sourceRevision: 1, brief: title, criteria: ['Keep behavior'] });
  });
  store.close();
  const server = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json') });
  try {
    for (const task of tasks) {
      await page.goto(`${server.origin}/#${task.id}`);
      await page.getByRole('button', { name: 'Queue run' }).click();
      await expect(page.getByRole('status')).toContainText('Plan queued');
      await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
    }
    const panel = page.getByRole('region', { name: 'Queue' });
    await expect(panel.locator('li')).toHaveCount(2);
    await expect(panel.locator('li').first()).toContainText(tasks[0].title);
    await expect(panel.locator('li').first()).toContainText('Fixture · coordinator');
    await expect(panel.locator('li').first()).toContainText('Launch details saved · Queued plan, not running');
    await panel.getByRole('button', { name: `Move ${tasks[1].key} up` }).click();
    await expect(panel.locator('li').first()).toContainText(tasks[1].title);
    await page.reload();
    await expect(panel.locator('li').first()).toContainText(tasks[1].title);
    await panel.getByRole('button', { name: `Remove ${tasks[0].key} from queue` }).click();
    await expect(panel.locator('li')).toHaveCount(1);
    await expect(panel.locator('li').first()).toContainText(tasks[1].title);
    await page.reload();
    await expect(panel.locator('li')).toHaveCount(1);
    await expect(panel.locator('li')).toContainText('Queued plan, not running');
    await page.goto(`${server.origin}/#${tasks[0].id}`);
    await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
    await expect(page.getByRole('definition').filter({ hasText: /^Cancelled$/ })).toBeVisible();
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});

test('QUE-04: declare a dependency and retain its queue hold across reload', async ({ page }, testInfo) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-dependency-browser-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const tasks = ['Upstream delivery', 'Dependent follow-up'].map(title => {
    const task = store.createTask({ title, description: '', repositoryId: repository.id });
    return store.refine(task.id, { revision: task.revision, sourceRevision: 1, brief: title, criteria: ['One result'] });
  });
  store.queue(tasks[1].id, { revision: tasks[1].revision, requestId: crypto.randomUUID(),
    launch: { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' } } }, defaultRoles);
  store.close();
  const server = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json') });
  try {
    await page.goto(`${server.origin}/#${tasks[1].id}`);
    const dependencies = page.getByRole('region', { name: 'Ticket dependencies' });
    await dependencies.getByLabel('Add dependency').selectOption(tasks[0].id);
    await dependencies.getByRole('button', { name: 'Add dependency' }).click();
    await expect(page.getByRole('status')).toContainText('Dependencies saved');
    await expect(dependencies).toContainText(`${tasks[0].key} · none`);
    await expect(page.getByRole('region', { name: 'Queue' })).toContainText(`${tasks[0].key}: no coding run or PR yet`);
    await expect(page.getByRole('region', { name: 'Requirement adoption' })).toHaveCount(0);
    await page.reload();
    await expect(dependencies).toContainText(`${tasks[0].key} · none`);
    await expect(page.getByRole('region', { name: 'Queue' })).toContainText(`${tasks[0].key}: no coding run or PR yet`);
    await page.screenshot({ path: testInfo.outputPath('dependency-hold.png'), fullPage: true });
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});

test('QUE-03: paused review release and waiting ticket hold are truthful in the workbench', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-review-browser-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const launch = { team: [{ name: 'Coordinator', roleId: 'coordinator', coordinator: true, input: 'Coordinate.' }], repository: { mode: 'new' } };
  const queue = (title: string) => {
    const task = store.createTask({ title, description: '', repositoryId: repository.id });
    const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['One result'] });
    return store.queue(task.id, { revision: accepted.revision, requestId: crypto.randomUUID(), launch }, defaultRoles);
  };
  const first = queue('Awaiting human review'); const second = queue('Holding coding slot'); const third = queue('Next queued plan');
  store.claimCodingSlot(first.id);
  const work = await new RepositoryWork(store, join(root, 'worktrees')).prepare(first.id, launch.repository);
  const publication: PublicationRecord = { id: crypto.randomUUID(), runId: first.id, attemptId: crypto.randomUUID(), candidateId: crypto.randomUUID(),
    repositoryPath: root, repository: { uuid: `{${crypto.randomUUID()}}`, workspace: 'fixture', slug: 'repository' },
    branch: work.branch, target: 'main', revision: work.checkpoints.at(-1)!.revision, title: 'Review', description: 'Fixture',
    policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 1 }, ready: true, blockers: [] };
  store.savePublication(publication);
  store.releaseCodingSlot(first.id, 'review', publication.candidateId); store.finishPause(first.id);
  store.claimCodingSlot(second.id);
  store.close();
  const server = await startServer({ port: 0, dataDir: root, rolePath: join(root, 'roles.json') });
  try {
    await page.goto(`${server.origin}/#${first.taskId}`);
    await expect(page.getByRole('definition').filter({ hasText: /^Paused · awaiting human review$/ })).toBeVisible();
    const queuePanel = page.getByRole('region', { name: 'Queue' });
    await expect(queuePanel).toContainText(`Coding slot: ${second.taskSnapshot.key}`);
    const waitingPlan = queuePanel.getByRole('listitem').filter({ hasText: third.taskSnapshot.key });
    await expect(waitingPlan).toContainText('Hold reason:');
    await expect(waitingPlan).toContainText(second.taskSnapshot.key);
    await page.goto(`${server.origin}/#${third.taskId}`);
    await expect(page.getByText('Queued plan, not running.', { exact: false }).first()).toContainText(second.taskSnapshot.key);
    await page.screenshot({ path: '.crewroom/test-results/m62/scheduler-review.png', fullPage: true });
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});

test('INT-04: brief history and queued/active adoption decisions remain truthful after reload', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-adoption-browser-'));
  seedRepository(root);
  let store = new Workbench(join(root, 'workflow.sqlite'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Adopt edited criteria', description: '', repositoryId: repository.id });
  store.close(); let server = await launch(root);
  try {
    await page.goto(`${server.origin}/#${task.id}`);
    await page.getByLabel('Local brief').fill('First brief');
    await page.getByLabel('Acceptance criteria').fill('Keep zero');
    await page.getByRole('button', { name: 'Save brief' }).click();
    await expect(page.getByText('Revision 2 · Current accepted')).toBeVisible();
    await page.getByRole('button', { name: 'Queue run' }).click();
    await page.getByRole('tab', { name: 'Brief' }).click();
    await page.getByLabel('Local brief').fill('Queued refinement');
    await page.getByLabel('Acceptance criteria').fill('Keep zero\nReject negatives');
    await page.getByRole('button', { name: 'Save brief' }).click();
    await expect(page.getByRole('region', { name: 'Requirement adoption' })).toContainText('Revision 3 awaits an adoption decision');
    await expect(page.getByText('Run adopted revision 2')).toBeVisible();
    await page.reload();
    await page.getByRole('button', { name: 'Adopt revision 3' }).click();
    await expect(page.getByText('Run adopted revision 3')).toBeVisible();
    await page.reload();
    await expect(page.getByText('Revision 3 · Current accepted')).toBeVisible();
    await expect(page.getByText('Run adopted revision 3')).toBeVisible();

    await server.close(); store = new Workbench(join(root, 'workflow.sqlite'));
    const run = store.snapshot().runs[0];
    const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
    const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
    const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Continue.' });
    store.claimAttempt(attempt.id);
    store.recordAttemptIdentity(attempt.id, { hostPid: 1, ompPid: 2, sessionId: 'fixture', hostUrl: 'http://127.0.0.1:9', pane: '%0' });
    store.startExecution(run.id); store.close(); server = await launch(root);
    await page.goto(`${server.origin}/#${task.id}`);
    await page.getByRole('tab', { name: 'Brief' }).click();
    await page.getByLabel('Local brief').fill('Active refinement');
    await page.getByLabel('Acceptance criteria').fill('Keep zero\nReject negatives\nHandle empty input');
    await page.getByRole('button', { name: 'Save brief' }).click();
    await expect(page.getByRole('button', { name: 'Adopt revision 4' })).toBeDisabled();
    await page.getByLabel('Adoption reason (required for active work)').fill('Empty input is now in scope.');
    await page.getByRole('button', { name: 'Adopt revision 4' }).click();
    await page.reload();
    await expect(page.getByText('Run adopted revision 4')).toBeVisible();
    await expect(page.getByText('Revision 4 · Current accepted')).toBeVisible();
    await page.getByRole('tab', { name: 'Coordinator' }).click();
    await expect(page.getByText('Requirement decision')).toBeVisible();
    await expect(page.getByText('Stored · Not delivered')).toBeVisible();
    await expect(page.getByRole('definition').filter({ hasText: /^Ownership unconfirmed$/ })).toBeVisible();
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});

test('M1 exit gate: import, refine, refresh, queue and steering survive a real server restart', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-restart-'));
  seedRepository(root);
  let server = await launch(root);
  try {
    await page.goto(server.origin);
    await page.getByRole('button', { name: 'Repositories', exact: true }).click();
    await page.getByLabel('Repository name').fill('Disposable fixture');
    await page.getByLabel('Checkout path').fill(root);
    await page.getByRole('button', { name: 'Add repository' }).click();
    await expect(page.getByRole('status')).toContainText('Repository saved');
    const imported = (description: string) => JSON.stringify({ siteUrl: 'https://fixture.atlassian.net', issues: [
      { id: '100', key: 'SIM-1', fields: { summary: 'Fix addition', description, status: { name: 'Backlog' } } },
    ] });
    await page.getByRole('button', { name: 'Import Jira snapshot' }).click();
    await page.getByLabel('Jira JSON').fill(imported('Original source'));
    await page.getByRole('button', { name: 'Import snapshot', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Fix addition' })).toBeVisible();
    await page.getByRole('combobox', { name: 'Repository', exact: true }).selectOption({ label: 'Disposable fixture' });
    await page.getByRole('button', { name: 'Save target' }).click();
    await expect(page.getByRole('status')).toContainText('Target repository saved');
    await page.getByLabel('Local brief').fill('Keep zero behavior correct');
    await page.getByLabel('Acceptance criteria').fill('0 + 0 returns 0');
    await page.getByRole('button', { name: 'Save brief' }).click();
    await expect(page.getByRole('status')).toContainText('Brief saved');
    await page.getByRole('button', { name: 'Import Jira snapshot' }).click();
    await page.getByLabel('Jira JSON').fill(imported('Refreshed source'));
    await page.getByRole('button', { name: 'Import snapshot', exact: true }).click();
    await expect(page.getByText('Refreshed source', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Local brief')).toHaveValue('Keep zero behavior correct');
    await expect(page.getByRole('button', { name: 'Queue run' })).toBeDisabled();
    await page.getByRole('button', { name: 'Save brief' }).click();
    await expect(page.getByRole('status')).toContainText('Brief saved');
    await page.getByRole('button', { name: 'Queue run' }).click();
    await expect(page.getByRole('definition').filter({ hasText: /^Queued$/ })).toBeVisible();
    await page.getByLabel('Message to coordinator').fill('Keep the patch small.');
    await page.getByRole('button', { name: 'Send to coordinator' }).click();
    await expect(page.getByText('Stored · Not delivered')).toBeVisible();
    const fragment = new URL(page.url()).hash;
    await server.close();
    server = await launch(root);
    await page.goto(server.origin + '/' + fragment);
    await page.getByRole('tab', { name: 'Brief' }).click();
    await expect(page.getByLabel('Local brief')).toHaveValue('Keep zero behavior correct');
    await expect(page.getByText('Refreshed source', { exact: true })).toBeVisible();
    await expect(page.getByRole('definition').filter({ hasText: /^Queued$/ })).toBeVisible();
    await expect(page.getByText('No agents executing', { exact: true })).toBeVisible();
    await page.getByRole('tab', { name: 'Coordinator' }).click();
    await expect(page.getByText('Keep the patch small.', { exact: true })).toBeVisible();
    await expect(page.getByText('Stored · Not delivered')).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});

test('ROL-02: role editor persists revisions, protects drafts, resolves conflicts and fits 390px', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('button', { name: 'Roles', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'User roles', exact: true })).toBeVisible();
  await page.getByLabel('Role to edit').selectOption('new');
  await page.getByLabel('Role ID', { exact: true }).fill('performance-checker');
  await page.getByLabel('Role name', { exact: true }).fill('Performance checker');
  await page.getByLabel('Category', { exact: true }).fill('Quality');
  await page.getByLabel('Purpose', { exact: true }).fill('Inspect performance regressions');
  await page.getByRole('textbox', { name: 'Instructions', exact: true }).fill('Measure before recommending a change.');
  await page.getByLabel('Model selector', { exact: true }).fill('fixture/model');
  await page.getByRole('combobox', { name: 'Reasoning effort', exact: true }).selectOption('high');
  await page.getByRole('button', { name: 'Save role', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('status')).toContainText('Role saved');
  await expect(page.getByText('Editing revision 1')).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Roles', exact: true }).click();
  await page.getByLabel('Role to edit').selectOption('performance-checker');
  await expect(page.getByRole('textbox', { name: 'Instructions', exact: true })).toHaveValue('Measure before recommending a change.');
  await expect(page.getByLabel('Model selector', { exact: true })).toHaveValue('fixture/model');
  await expect(page.getByRole('combobox', { name: 'Reasoning effort', exact: true })).toHaveValue('high');
  await page.getByLabel('Model selector', { exact: true }).fill('fixture/updated-model');
  await page.getByLabel('Role to edit').selectOption('coordinator');
  await expect(page.getByRole('group', { name: 'Unsaved role changes' })).toBeVisible();
  await page.getByRole('button', { name: 'Keep editing' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByLabel('Model selector', { exact: true })).toHaveValue('fixture/updated-model');
  await page.getByRole('button', { name: 'Learning', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Instructions', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Roles', exact: true }).click();
  await expect(page.getByLabel('Model selector', { exact: true })).toHaveValue('fixture/updated-model');
  const original = (await (await page.request.get('/api/workspace')).json()).roles.find((role: { id: string }) => role.id === 'performance-checker');
  const competing = await page.request.post('/api/roles/performance-checker', { headers: { Origin: new URL(page.url()).origin }, data: { ...original, revision: 1, instructions: 'A second tab saved this revision.', model: 'fixture/other-model' } });
  expect(competing.status()).toBe(200);
  await page.getByRole('button', { name: 'Save role', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('current revision is 2');
  await expect(page.getByLabel('Model selector', { exact: true })).toHaveValue('fixture/updated-model');
  await expect(page.getByText(/Current definition is revision 2/)).toBeVisible();
  const revision2 = (await competing.json()).find((role: { id: string }) => role.id === 'performance-checker');
  const newer = await page.request.post('/api/roles/performance-checker', { headers: { Origin: new URL(page.url()).origin }, data: { ...revision2, model: 'fixture/newer-model' } });
  expect(newer.status()).toBe(200);
  await expect(page.getByText(/Current definition is revision 3/)).toBeVisible();
  await page.getByRole('button', { name: 'Reload current definition (discard draft)' }).click();
  await expect(page.getByLabel('Model selector', { exact: true })).toHaveValue('fixture/newer-model');
  await page.getByLabel('Model selector', { exact: true }).fill('fixture/final-model');
  await page.getByRole('button', { name: 'Save role', exact: true }).click();
  await expect(page.getByText('Editing revision 4')).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Roles', exact: true }).click();
  await page.getByLabel('Role to edit').selectOption('performance-checker');
  await expect(page.getByLabel('Model selector', { exact: true })).toHaveValue('fixture/final-model');
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('combobox', { name: 'Reasoning effort', exact: true }).focus();
  await page.keyboard.press('Tab');
  await expect(page.getByRole('checkbox', { name: 'Enabled for new assignments' })).toBeFocused();
  await page.getByRole('checkbox', { name: 'Enabled for new assignments' }).uncheck();
  await page.getByRole('button', { name: 'Save role', exact: true }).click();
  await expect(page.getByText('Editing revision 5')).toBeVisible();
  await page.getByRole('button', { name: 'New ticket' }).click();
  await page.getByLabel('Ticket title').fill('Check available roles');
  await page.getByRole('button', { name: 'Create ticket' }).click();
  const queuedRole = page.getByLabel('Role for queued coordinator');
  await expect(queuedRole.locator('option[value="coordinator"]')).toHaveCount(1);
  await expect(queuedRole.locator('option[value="performance-checker"]')).toHaveCount(0);
  await page.screenshot({ path: '.crewroom/test-results/m3-roles.png', fullPage: true });
});

test('observation view reconnects through event pages without losing coordinator replies or latest progress', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-paged-browser-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Observe old replies', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Retain observed conversation', criteria: ['Show reply history'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Continue.' });
  store.recordAgentEvent(attempt.id, 1, { type: 'message_end', message: { role: 'assistant', timestamp: 1, content: [{ type: 'text', text: 'First saved reply' }] } });
  for (let sequence = 2; sequence <= 231; sequence++) store.recordAgentEvent(attempt.id, sequence, { type: 'message_update', phase: 'text' });
  store.recordAgentEvent(attempt.id, 232, { type: 'message_end', message: { role: 'assistant', timestamp: 2, content: [{ type: 'text', text: 'Final saved reply' }] } });
  store.recordAgentEvent(attempt.id, 233, { type: 'tool_started', commandId: 'fixture-check' });
  store.close();
  const server = await launch(root);
  try {
    await page.goto(`${server.origin}/?observe=${run.id}`);
    await expect(page.getByText('First saved reply', { exact: true })).toBeVisible();
    await expect(page.getByText('Final saved reply', { exact: true })).toBeVisible();
    await expect(page.locator('.agent-progress')).toHaveText('Command started: fixture-check');
    expect(await page.getByText('Final saved reply', { exact: true }).count()).toBe(1);
    await page.reload();
    await expect(page.getByText('First saved reply', { exact: true })).toBeVisible();
    await expect(page.getByText('Final saved reply', { exact: true })).toBeVisible();
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});

test('REP-01: configure a checked repository, show target validation, and select its truthful publication state', async ({ page }, testInfo) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-repository-browser-'));
  const checkout = join(root, 'checkout'); seedRepository(checkout);
  const server = await launch(root);
  try {
    await page.goto(server.origin);
    await page.getByRole('button', { name: 'Repositories', exact: true }).click();
    await page.getByLabel('Repository name').fill('Release checkout');
    await page.getByLabel('Checkout path').fill(checkout);
    await page.getByLabel('Target branch').fill('missing-branch');
    await page.getByRole('button', { name: 'Add repository' }).click();
    await expect(page.getByRole('alert')).toContainText('Target branch must resolve');
    await page.getByLabel('Target branch').fill('main');
    await page.getByLabel('Branch prefix').fill('feature/crewroom/');
    await page.getByRole('button', { name: 'Add check' }).click();
    const row = page.getByRole('group', { name: 'Check 1' });
    await row.getByLabel('Check ID').fill('unit');
    await row.getByLabel('Image digest').fill(`sha256:${'a'.repeat(64)}`);
    await row.getByLabel('Command', { exact: true }).fill('/bin/sh');
    await row.getByLabel('Arguments').fill('-c\nexit 0');
    await row.getByLabel('Memory (MiB)').fill('256');
    await page.getByRole('button', { name: 'Add repository' }).click();
    await expect(page.getByRole('status')).toContainText('Repository saved');
    const list = page.locator('.repository-list');
    await expect(list).toContainText('feature/crewroom/');
    await expect(list).toContainText('1 checks');
    await expect(list).toContainText('PR publication unavailable (no Bitbucket connection)');
    await page.getByRole('button', { name: 'New ticket' }).click();
    await page.getByLabel('Ticket title').fill('Choose release checkout');
    await page.getByRole('button', { name: 'Create ticket' }).click();
    await page.getByRole('combobox', { name: 'Repository', exact: true }).selectOption({ label: 'Release checkout' });
    await page.getByRole('button', { name: 'Save target' }).click();
    const card = page.getByRole('heading', { name: 'Target repository' }).locator('..');
    await expect(card).toContainText('Saved target: Release checkout');
    await expect(card).toContainText('Target branch: main');
    await expect(card).toContainText('Checks: 1');
    await expect(card).toContainText('unit · required');
    await expect(card).toContainText('PR publication unavailable (no Bitbucket connection)');
    await page.screenshot({ path: testInfo.outputPath('repository-selection.png'), fullPage: true });
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Target repository' }).locator('..')).toContainText('Saved target: Release checkout');
  } finally { await server.close(); rmSync(root, { recursive: true, force: true }); }
});
