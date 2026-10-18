import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import type { deliveryProviderFixture } from './delivery-provider-fixture';
import { createRuntimeObserver } from './runtime-observer';

export async function verifyDelivery(run: { id: string; taskId: string }, provider: Awaited<ReturnType<typeof deliveryProviderFixture>>, request: (path: string, body?: unknown) => Promise<any>, restart: () => Promise<void>, origin: string) {
  const observedRuntime = createRuntimeObserver(request);
  const reconcile = () => request(`/runs/${run.id}/publication/reconcile`, {});
  const result = () => request(`/runs/${run.id}/result`);
  const jiraState = async (kind: string) => (await request('/workspace')).jiraUpdates.find((item: any) => item.kind === kind)?.state;
  async function until(read: () => Promise<any>, ready: (value: any) => boolean) {
    const deadline = Date.now() + 30000; let value;
    do { value = await read(); if (ready(value)) return value; await new Promise(resolve => setTimeout(resolve, 100)); } while (Date.now() < deadline);
    throw new Error('Delivery state did not arrive: ' + JSON.stringify(value));
  }
  await reconcile(); assert.match((await result()).publication.record.error, /403/); assert.equal(provider.receipts().prPosts, 0);
  provider.fault.bitbucket = 'outage'; await reconcile(); assert.match((await result()).publication.record.error, /503/); assert.equal(provider.receipts().prPosts, 0);
  provider.fault.bitbucket = 'available'; await reconcile();
  assert.equal((await result()).publication.record.pr.state, 'uncertain'); assert.equal(provider.receipts().prPosts, 1);
  await restart(); await reconcile();
  assert.equal((await result()).publication.record.pr.state, 'confirmed'); assert.equal((await result()).publication.record.ready, false); assert.equal(provider.receipts().prPosts, 1);
  await request(`/runs/${run.id}/resume`, {});
  await until(() => request('/workspace'), state => state.jiraUpdates.some((item: any) => item.kind === 'pr_link'));
  await request('/jira/reconcile', {}); assert.equal(await jiraState('in_progress'), 'pending'); assert.equal(await jiraState('pr_link'), 'pending');
  provider.fault.jira = 'available'; provider.fault.hideComments = true;
  await request('/jira/reconcile', {}); assert.equal(await jiraState('in_progress'), 'uncertain'); assert.equal(await jiraState('pr_link'), 'pending');
  await request('/jira/reconcile', {}); assert.equal(await jiraState('in_progress'), 'confirmed'); assert.equal(await jiraState('pr_link'), 'uncertain');
  await request('/jira/reconcile', {}); assert.equal(provider.receipts().commentPosts, 1);
  // Restart only after the Reporter's reply is recorded: a tool call still queued when the
  // supervisor closes is refused, and this scenario tests Jira reconciliation, not that race.
  await until(() => request(`/runs/${run.id}/runtime`), value => value.messages.some((item: any) => item.from === value.assignments.find((assignment: any) => assignment.name === 'Reporter')?.id));
  await restart(); provider.fault.hideComments = false; await request('/jira/reconcile', {});
  assert.equal(await jiraState('pr_link'), 'confirmed'); assert.equal(provider.receipts().commentPosts, 1); assert.equal(provider.receipts().transitionPosts, 1);
  provider.fault.pipeline = 'failed'; await reconcile(); assert.equal((await result()).publication.record.ready, false);
  provider.fault.pipeline = 'successful'; await reconcile(); assert.equal((await result()).publication.record.ready, true);
  await request(`/runs/${run.id}/resume`, {});
  const runtime = await until(() => observedRuntime(run.id), value => JSON.stringify(value.events).includes('DELIVERY_READY') && value.deliveryNotices.every((item: any) => item.disposition === 'received') && value.attempts.filter((item: any) => item.ownership === 'held').every((item: any) => item.state === 'idle'));
  assert.equal(new Set(runtime.assignments.map((item: any) => runtime.attempts.find((attempt: any) => attempt.id === item.currentAttemptId).ompPid)).size, 4);
  assert.equal(new Set(runtime.assignments.map((item: any) => runtime.attempts.find((attempt: any) => attempt.id === item.currentAttemptId).sessionId)).size, 4);
  assert.equal(new Set(runtime.assignments.map((item: any) => item.model.resolved.selector)).size, 3);
  assert.ok(runtime.messages.length >= 6); assert.ok(runtime.messages.every((item: any) => item.disposition === 'received'));
  for (const toolName of ['publish_change', 'jira_report']) {
    const completions = runtime.events.filter((item: any) => item.event.type === 'tool_execution_end' && item.event.toolName === toolName);
    assert.equal(completions.length, toolName === 'jira_report' ? 2 : 1); assert.ok(completions.every((item: any) => item.event.isError === false));
  }
  const current = await result(); const pr = provider.receipts().pr;
  assert.ok(pr.description.includes(current.assessment.candidate.revision.head));
  const finalCheck = current.checks.findLast((item: any) => item.candidateId === current.assessment.candidate.id);
  assert.ok(pr.description.includes(finalCheck.output.sha256));
  for (const check of current.checks.filter((item: any) => item.candidateId !== current.assessment.candidate.id && item.exitCode !== 0)) assert.equal(pr.description.includes(check.output.sha256), false);
  assert.equal(provider.gitRef(pr.source.branch.name), current.assessment.candidate.revision.head);
  assert.equal(JSON.stringify(runtime).includes('private-bitbucket-fixture-token'), false);
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage(); await page.goto(origin + '/#' + run.taskId);
    await page.getByRole('tab', { name: 'Change & evidence', exact: true }).click();
    await page.getByRole('region', { name: 'PR delivery', exact: true }).getByText('Ready for human review', { exact: true }).waitFor();
    await page.getByRole('region', { name: 'Jira delivery', exact: true }).getByText('pr link · confirmed', { exact: true }).waitFor();
    await page.getByText('Controller delivery notices', { exact: true }).click();
    await page.getByText('publication · received', { exact: false }).first().waitFor();
  } finally { await browser.close(); }
  return { passed: true, independentProcesses: 4, distinctSessions: 4, modelSelectors: 3, mailReceipts: runtime.messages.length, publishToolCalls: 1, jiraToolCalls: 2, prPosts: provider.receipts().prPosts, jiraCommentPosts: provider.receipts().commentPosts, jiraTransitions: provider.receipts().transitionPosts,
    restarts: 2, permissionAndOutageHeld: true, lostRepliesReconciled: true, controllerNotices: runtime.deliveryNotices.length, browserReadyAndJiraConfirmed: true, revision: current.assessment.candidate.revision.head };
}
