import { existsSync, readFileSync, writeFileSync, readdirSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import { startServer } from '../server/app';
import assert from 'node:assert/strict';
import { createRuntimeObserver } from './runtime-observer';
import { execFileSync } from 'node:child_process';
import type { Message, Run, Task } from '../shared/types';
import { Workbench } from '../server/workbench';
import type { PublicationRecord } from '../shared/publication';
import { deliveryProviderFixture } from './delivery-provider-fixture';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const provider = config.scenario === 'queue' ? await deliveryProviderFixture(config.root, config.repository) : undefined;
if (provider) provider.fault.bitbucket = 'available';
const options = { port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime,
  ...(provider ? provider.options : config.jiraApiUrl ? { jira: { source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '1', assigneeAccountId: 'operator', name: 'Fixture' }, apiUrl: config.jiraApiUrl, credentials: async () => 'fixture-token' } } : {}) };
let app = await startServer(options);
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); assert.ok(response.ok, JSON.stringify(result)); return result;
};
const observedRuntime = createRuntimeObserver(request);
async function settled(runId: string, text: string, timeout = 12000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const state = await observedRuntime(runId);
    if (JSON.stringify(state.events).includes(text) && state.attempts.at(-1).state === 'idle') return state;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error('Agent did not settle at the expected checkpoint.');
}
async function refinementScenario() {
  const repo = await request('/repositories', { name: 'Fixture', path: config.repository, targetBranch: 'main' });
  const codingTask = await request('/tasks', { title: 'Direct coding', description: 'No refinement', repositoryId: repo.id });
  await request(`/tasks/${codingTask.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Direct run', criteria: ['Coding work starts directly'] });
  const coding = await request(`/tasks/${codingTask.id}/runs`, { revision: 2, requestId: crypto.randomUUID() });
  await request(`/runs/${coding.id}/repository`, { mode: 'new' });
  await request(`/runs/${coding.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Coordinate direct coding work.' }] });
  await settled(coding.id, 'CODING_ACTIVE');
  const attempts = [];
  for (const purpose of ['explore', 'grill'] as const) {
    const task = await request('/tasks', { title: `${purpose} separately`, description: 'Develop requirements without coding' });
    const requestId = crypto.randomUUID();
    const run = await request(`/tasks/${task.id}/refinement-runs`, { revision: 1, purpose, requestId });
    assert.equal((await request(`/tasks/${task.id}/refinement-runs`, { revision: 1, purpose, requestId })).id, run.id);
    await settled(run.id, `${purpose.toUpperCase()}_PROPOSED`);
    const observed = await request(`/runs/${run.id}/runtime`);
    assert.equal(observed.assignments.length, 1); assert.equal(observed.assignments[0].coordinator, true);
    assert.equal(observed.attempts.length, 1); assert.ok(observed.attempts[0].ompPid && observed.attempts[0].hostPid);
    assert.equal((await request('/workspace')).runs.filter((item: Run) => item.taskId === task.id).length, 1);
    assert.equal((await fetch(`${app.origin}/api/runs/${run.id}/repository`)).status, 404, 'Refinement has no repository worktree.');
    const proposal = (await request('/workspace')).refinementProposals.find((item: { runId: string }) => item.runId === run.id);
    assert.equal(proposal?.status, 'pending');
    if (purpose === 'explore') {
      const note = await request(`/runs/${run.id}/steering`, { requestId: 'explore-conversation', body: 'Include the zero case in your proposed criteria.' });
      writeFileSync(`${config.root}/refinement-steering.json`, JSON.stringify(note));
      await settled(run.id, 'REFINEMENT_STEERING_RECONCILED');
      const delivered = (await request('/workspace')).messages.find((item: Message) => item.id === note.id);
      assert.equal(delivered.delivery, 'applied');
      assert.match(delivered.dispositionReason, /zero case/);
    }
    const decided = await request(`/refinement-proposals/${proposal.id}/decision`, purpose === 'explore'
      ? { decision: 'accept', requestId: crypto.randomUUID(), brief: 'Operator-edited Explore result' }
      : { decision: 'discard', requestId: crypto.randomUUID(), reason: 'The challenge needs more evidence.' });
    assert.equal(decided.status, purpose === 'explore' ? 'accepted' : 'discarded');
    const closed = await request(`/runs/${run.id}/close`, {});
    assert.equal(closed.status, 'closed');
    assert.ok((await request(`/runs/${run.id}/runtime`)).attempts.every((item: { ownership?: string }) => item.ownership === 'stopped'));
    attempts.push(observed.attempts[0].id);
  }
  assert.equal((await request('/workspace')).runs.find((item: Run) => item.id === coding.id).status, 'running');
  const ended = await request(`/runs/${coding.id}/cancel`, {}); assert.equal(ended.status, 'cancelled');
  assert.ok((await request(`/runs/${coding.id}/runtime`)).attempts.every((item: { ownership?: string }) => item.ownership === 'stopped'));
  console.log(JSON.stringify({ passed: true, scenario: 'refinement', coordinatorAttempts: attempts, codingRun: coding.id }));
}
async function researchScenario() {
  const git = (...args: string[]) => execFileSync('git', ['-C', config.repository, ...args], { encoding: 'utf8' }).trim();
  const head = git('rev-parse', 'HEAD'); const before = git('status', '--porcelain');
  const repo = await request('/repositories', { name: 'Pinned queue fixture', path: config.repository, targetBranch: 'main' });
  const task = (await request('/import/jira', { siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '100', key: 'SIM-110', fields: { summary: 'Compare queue designs', description: 'Fictional FIFO and priority designs', status: { id: '1', name: 'To Do' } } }] }))[0];
  await request(`/tasks/${task.id}/repository`, { revision: 1, repositoryId: repo.id });
  const accepted = await request(`/tasks/${task.id}/refinement`, { revision: 2, sourceRevision: 1, brief: 'Compare FIFO and priority using checked-in source, document risk and tradeoff.', criteria: ['Compare both queue options', 'Define decision criteria and limitations'] });
  const run = await request(`/tasks/${task.id}/research-runs`, { revision: accepted.revision, requestId: crypto.randomUUID() });
  assert.equal(run.researchSource.commit, head); assert.equal(run.purpose, 'research');
  assert.equal((await fetch(`${app.origin}/api/runs/${run.id}/repository`)).status, 404);
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Select a non-coding team. Finish only with verified artifacts.' }] });
  async function awaitAgent(name: string, text: string) {
    const deadline = Date.now() + 25000;
    while (Date.now() < deadline) {
      const state = await observedRuntime(run.id);
      const assigned = state.assignments.find((item: { name: string }) => item.name === name);
      const attempt = state.attempts.find((item: { id: string }) => item.id === assigned?.currentAttemptId);
      if (attempt?.state === 'idle' && state.events.some((item: { attemptId: string; event: unknown }) => item.attemptId === attempt.id && JSON.stringify(item.event).includes(text))) return state;
      await delay(50);
    }
    throw new Error(`${name} did not settle after ${text}.`);
  }
  await awaitAgent('Coordinator', 'TEAM_READY');
  await awaitAgent('Architect', 'BAD_HANDOFF'); await awaitAgent('TechWriter', 'DRAFT_READY');
  const first = await request(`/runs/${run.id}/research`);
  assert.equal(first.artifacts.length, 2);
  const rejected = first.artifacts.find((item: { state: string }) => item.state === 'rejected');
  assert.ok(rejected); assert.match(rejected.reasons.join(' '), /missing.md/);
  assert.equal(first.assessment.ready, false); assert.deepEqual(first.assessment.covered, [1]);
  const coordinator = (await request(`/runs/${run.id}/runtime`)).assignments.find((item: { coordinator: boolean }) => item.coordinator);
  await request(`/runs/${run.id}/steering`, { requestId: crypto.randomUUID(), body: 'Try to complete this research outcome. Do not invent citations.' });
  const premature = await awaitAgent('Coordinator', 'INCOMPLETE_HANDOFF');
  assert.equal(premature.events.some((item: { event: { type: string; toolName?: string; isError?: boolean; result?: unknown } }) => item.event.type === 'tool_execution_end' && item.event.isError && item.event.toolName === 'request_research_completion' && JSON.stringify(item.event.result).includes('Criterion 0')), true);
  assert.equal((await request(`/runs/${run.id}/research`)).assessment.ready, false);
  await request(`/runs/${run.id}/steering`, { requestId: crypto.randomUUID(), body: 'Request a corrected source citation from Architect.' });
  await awaitAgent('Coordinator', 'CORRECTION_REQUESTED'); await awaitAgent('Architect', 'CORRECTED_HANDOFF');
  const corrected = await request(`/runs/${run.id}/research`);
  assert.equal(corrected.assessment.ready, true);
  const superseded = corrected.artifacts.find((item: { id: string }) => item.id === rejected.id);
  assert.ok(superseded?.supersededBy);
  const replacement = corrected.artifacts.find((item: { id: string }) => item.id === superseded.supersededBy);
  assert.ok(replacement); assert.equal(replacement.state, 'verified');
  const read = await request(`/runs/${run.id}/result/artifacts/${replacement.body.id}`);
  assert.match(read.content, /FIFO is predictable/);
  await request(`/runs/${run.id}/steering`, { requestId: crypto.randomUUID(), body: 'Complete the verified outcome now.' });
  const deadline = Date.now() + 25000;
  while (Date.now() < deadline && (await request('/workspace')).runs.find((item: Run) => item.id === run.id)?.status !== 'completed') await delay(100);
  const final = (await request('/workspace')).runs.find((item: Run) => item.id === run.id);
  assert.equal(final.status, 'completed');
  const runtime = await request(`/runs/${run.id}/runtime`);
  assert.equal(runtime.assignments.length, 3); assert.equal(runtime.attempts.filter((item: { ownership: string }) => item.ownership !== 'stopped').length, 0);
  const evidence = await request(`/runs/${run.id}/result`);
  assert.equal(evidence.research.assessment.completed, true);
  assert.deepEqual([evidence.candidates, evidence.checks, evidence.reviews], [[], [], []]);
  await request('/jira/reconcile', {}); await request('/jira/reconcile', {});
  const jira = (await request('/workspace')).jiraUpdates.filter((item: { runId: string }) => item.runId === run.id);
  assert.equal(jira.length, 1); assert.equal(jira[0].kind, 'research_outcome'); assert.equal(jira[0].state, 'confirmed'); assert.equal(jira[0].attemptId, coordinator.currentAttemptId);
  assert.equal(git('rev-parse', 'HEAD'), head); assert.equal(git('status', '--porcelain'), before);
  console.log(JSON.stringify({ passed: true, scenario: 'research', runId: run.id, artifactHashes: corrected.artifacts.map((item: { body: { sha256: string } }) => item.body.sha256), requests: corrected.artifacts.length, jiraUpdate: jira[0].id, attemptedCompletionRejected: true, stoppedAgents: runtime.attempts.length }));
}

async function schedulerScenario() {
  const repo = await request('/repositories', { name: 'Queue target', path: config.repository, targetBranch: 'main' });
  const create = async (title: string, team: { name: string; roleId: string; coordinator: boolean; input: string }[]) => {
    const task = await request('/tasks', { title, description: '', repositoryId: repo.id });
    await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: title, criteria: ['Produce one result'] });
    return request(`/tasks/${task.id}/runs`, { revision: 2, requestId: crypto.randomUUID(), launch: { team, repository: { mode: 'new' } } });
  };
  const heldTask = await request('/tasks', { title: 'Needs launch details', description: '', repositoryId: repo.id });
  await request(`/tasks/${heldTask.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Hold this plan', criteria: ['Remain queued'] });
  const held = await request(`/tasks/${heldTask.id}/runs`, { revision: 2, requestId: crypto.randomUUID() });
  const first = await create('First coding ticket', [
    { name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Coordinate the first ticket.' },
    { name: 'Writer', roleId: 'implementer', coordinator: false, input: 'Implement the accepted brief.' },
    { name: 'Reviewer', roleId: 'reviewer', coordinator: false, input: 'Review independently.' },
    { name: 'Reporter', roleId: 'triage', coordinator: false, input: 'Report progress.' },
  ]);
  await settled(first.id, 'FIRST_EXECUTING', 45000);
  const agents = await request(`/runs/${first.id}/runtime`);
  assert.equal(agents.assignments.length, 4);
  assert.equal(agents.attempts.filter((attempt: { hostPid?: number; ompPid?: number }) => attempt.hostPid && attempt.ompPid).length, 4);
  const second = await create('Next coding ticket', [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Coordinate the next ticket.' }]);
  const queue = (await request('/workspace')).queue;
  assert.equal(queue.slot.runId, first.id); assert.equal(queue.plans[0].id, held.id);
  assert.equal(queue.plans[0].holdReason, 'Needs launch details');
  assert.equal(queue.plans[1].id, second.id);
  assert.match(queue.plans[1].holdReason, /Waiting for .* ready for review/);
  const store = new Workbench(`${config.root}/workflow.sqlite`);
  const work = store.repositoryWork(first.id)!;
  const publication: PublicationRecord = { id: crypto.randomUUID(), runId: first.id, attemptId: agents.attempts[0].id,
    candidateId: crypto.randomUUID(), repositoryPath: config.repository, repository: { uuid: `{${crypto.randomUUID()}}`, workspace: 'fixture', slug: 'repository' },
    branch: work.branch, target: 'main', revision: work.checkpoints.at(-1)!.revision, title: 'First', description: 'Review fixture',
    policy: { statusKeys: [], pipelineSteps: [], optionalSteps: [] }, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 17, providerState: 'OPEN' }, ready: false, blockers: ['PR created'] };
  store.savePublication(publication);
  await request(`/runs/${first.id}/budget`, { activeLimitMs: 3600000 });
  assert.equal((await request('/workspace')).queue.slot.runId, first.id);
  store.savePublication({ ...publication, blockers: ['CI pending'] });
  await request(`/runs/${first.id}/budget`, { activeLimitMs: 3600000 });
  assert.equal((await request('/workspace')).queue.slot.runId, first.id);
  store.savePublication({ ...publication, ready: true, blockers: [] });
  store.close();
  await request(`/runs/${first.id}/budget`, { activeLimitMs: 3600000 });
  const next = await settled(second.id, 'SECOND_EXECUTING', 45000);
  const workspace = await request('/workspace');
  const prior = workspace.runs.find((run: Run) => run.id === first.id);
  assert.equal(prior.status, 'paused'); assert.equal(prior.pauseReason, 'review'); assert.equal(prior.slotRelease.candidateId, publication.candidateId);
  assert.equal(next.assignments.length, 1);
  assert.equal(workspace.queue.slot.runId, second.id);
  assert.equal(workspace.queue.plans[0].id, held.id);
  assert.equal(workspace.queue.plans[0].holdReason, 'Needs launch details');
  assert.equal((await request(`/runs/${first.id}/runtime`)).attempts.every((attempt: { ownership: string }) => attempt.ownership === 'stopped'), true);
  const refused = await fetch(`${app.origin}/api/runs/${first.id}/resume`, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: '{}' });
  assert.equal(refused.status, 409); assert.match(JSON.stringify(await refused.json()), new RegExp(second.taskSnapshot.key));
  await request(`/runs/${second.id}/cancel`, {});
  console.log(JSON.stringify({ passed: true, scenario: 'scheduler', first: first.id, next: second.id, firstAgents: agents.attempts.length, oldStoppedBeforeNext: true, refusal: 409 }));
}

async function queueScenario() {
  assert.ok(provider);
  provider.fault.jira = 'available';
  const repository = await request('/repositories', { name: 'Queue target', path: config.repository, targetBranch: 'main' });
  const issue = (id: string) => ({ id, key: `SIM-${id}`, fields: { summary: `Ticket ${id}`, description: 'Shared fixture requirements', status: { id: '1', name: 'To Do' } } });
  provider.setIssues(['201', '202', '203', '204'].map(issue));
  await request('/jira/sync', {});
  const tickets = (await request('/workspace')).tasks as Task[];
  const [a, b, c, backlog] = ['201', '202', '203', '204'].map(id => tickets.find(item => item.sourceId === id)!);
  for (const ticket of [a, b, c]) {
    const selected = await request(`/tasks/${ticket.id}/repository`, { revision: ticket.revision, repositoryId: repository.id });
    await request(`/tasks/${ticket.id}/refinement`, { revision: selected.revision, sourceRevision: 1, brief: `Deliver ${ticket.key}`,
      criteria: ['Produce one reviewed result'] });
  }
  const saved = await request(`/tasks/${c.id}/dependencies`, { dependencyRevision: (await request('/workspace')).tasks.find((item: Task) => item.id === c.id).dependencyRevision ?? 0,
    dependsOn: [a.id], requestId: crypto.randomUUID() });
  assert.deepEqual(saved.dependsOn, [a.id]);
  const queueBare = async (ticket: Task) => request(`/tasks/${ticket.id}/runs`, { revision: (await request('/workspace')).tasks.find((item: Task) => item.id === ticket.id).revision, requestId: crypto.randomUUID() });
  const first = await queueBare(a), dependent = await queueBare(c), independent = await queueBare(b);
  const original = (await request('/workspace')).queue;
  assert.deepEqual(original.plans.map((item: Run) => item.id), [first.id, dependent.id, independent.id]);
  await request('/queue/order', { requestId: crypto.randomUUID(), expectedRevision: original.revision, runIds: [first.id, independent.id, dependent.id] });
  provider.setIssues(['203', '202', '201', '204'].map(issue));
  await request('/jira/sync', {});
  const reordered = await request('/workspace');
  assert.deepEqual(reordered.tasks.slice(0, 3).map((item: Task) => item.key), [c.key, b.key, a.key]);
  assert.deepEqual(reordered.queue.plans.map((item: Run) => item.id), [first.id, independent.id, dependent.id]);
  const launch = (text: string) => ({ team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: text }], repository: { mode: 'new' } });
  await request(`/runs/${first.id}/launch`, { requestId: crypto.randomUUID(), launch: launch('Run A') });
  const aRuntime = await settled(first.id, 'QUEUE_A_EXECUTING', 45000);
  assert.equal(aRuntime.assignments.length, 1); assert.ok(aRuntime.attempts[0].hostPid && aRuntime.attempts[0].ompPid);
  await request(`/runs/${independent.id}/launch`, { requestId: crypto.randomUUID(), launch: launch('Run B') });
  await request(`/runs/${dependent.id}/launch`, { requestId: crypto.randomUUID(), launch: launch('Run C') });
  let workspace = await request('/workspace');
  assert.equal(workspace.queue.slot.runId, first.id);
  assert.match(workspace.queue.plans.find((item: Run) => item.id === dependent.id).holdReason, /coding run has no PR yet/);
  assert.equal((await request(`/runs/${dependent.id}/runtime`)).attempts.length, 0);
  const store = new Workbench(`${config.root}/workflow.sqlite`);
  const work = store.repositoryWork(first.id)!;
  const record: PublicationRecord = { id: crypto.randomUUID(), runId: first.id, attemptId: aRuntime.attempts[0].id,
    candidateId: crypto.randomUUID(), repositoryPath: config.repository, repository: provider.options.publication[0].repository,
    branch: work.branch, target: 'main', revision: work.checkpoints.at(-1)!.revision, title: 'Ticket A', description: 'Fixture PR',
    policy: provider.options.publication[0].policy, createdAt: new Date().toISOString(),
    push: { state: 'confirmed', expectedHead: null }, pr: { state: 'confirmed', id: 9, providerState: 'OPEN' }, ready: false, blockers: ['PR created'] };
  record.pr.describedSha256 = provider.seedPR(record);
  store.savePublication(record);
  await request(`/runs/${first.id}/publication/reconcile`, {});
  workspace = await request('/workspace'); assert.equal(workspace.queue.slot.runId, first.id);
  assert.match(workspace.queue.plans.find((item: Run) => item.id === dependent.id).holdReason, /PR open/);
  assert.equal((await request(`/runs/${independent.id}/runtime`)).attempts.length, 0);
  store.savePublication({ ...record, blockers: ['CI pending'] });
  await request(`/runs/${first.id}/publication/reconcile`, {});
  workspace = await request('/workspace'); assert.equal(workspace.queue.slot.runId, first.id);
  store.savePublication({ ...record, ready: true, blockers: [] });
  store.close();
  const beforeRelease = await request('/workspace');
  await request('/queue/order', { requestId: crypto.randomUUID(), expectedRevision: beforeRelease.queue.revision,
    runIds: beforeRelease.queue.plans.map((item: Run) => item.id) });
  const bRuntime = await settled(independent.id, 'QUEUE_B_EXECUTING', 45000);
  workspace = await request('/workspace');
  assert.equal(workspace.queue.slot.runId, independent.id);
  assert.equal(workspace.runs.find((item: Run) => item.id === first.id).pauseReason, 'review');
  assert.equal((await request(`/runs/${first.id}/runtime`)).attempts.every((item: { ownership: string }) => item.ownership === 'stopped'), true);
  assert.equal((await request(`/runs/${dependent.id}/runtime`)).attempts.length, 0);
  provider.setPRState('DECLINED');
  await request(`/runs/${first.id}/publication/reconcile`, {});
  workspace = await request('/workspace');
  assert.match(workspace.queue.plans.find((item: Run) => item.id === dependent.id).holdReason, /closed without merge/);
  provider.setPRState('MERGED', provider.gitRef('main'));
  await request(`/runs/${first.id}/publication/reconcile`, {});
  workspace = await request('/workspace');
  assert.ok(workspace.dependencyStates[a.id].state === 'merged');
  assert.equal(workspace.queue.slot.runId, independent.id);
  assert.equal((await request(`/runs/${dependent.id}/runtime`)).attempts.length, 0);
  await request(`/runs/${independent.id}/cancel`, {});
  const cRuntime = await settled(dependent.id, 'QUEUE_C_EXECUTING', 45000);
  assert.equal((await request(`/runs/${independent.id}/runtime`)).attempts.every((item: { ownership: string }) => item.ownership === 'stopped'), true);
  workspace = await request('/workspace'); assert.equal(workspace.queue.slot.runId, dependent.id);
  assert.equal(workspace.runs.some((item: Run) => item.taskId === backlog.id), false);
  assert.equal(provider.requests.some(path => /merge/i.test(path)), false);
  await request(`/runs/${dependent.id}/cancel`, {});
  console.log(JSON.stringify({ passed: true, scenario: 'queue', runs: [first.id, independent.id, dependent.id],
    order: [a.key, b.key, c.key], jiraRank: [c.key, b.key, a.key], agentProcesses: [aRuntime, bRuntime, cRuntime].map(state => state.attempts[0].ompPid),
    providerMergeObserved: true, syntheticRecordSteps: ['A PR created', 'A CI pending', 'A readiness'],
    providerObservedSteps: ['A PR open', 'A PR closed-unmerged', 'A PR merged'], noMergeApi: true, backlogUnqueued: true }));
}

try {
  if (config.scenario === 'queue') await queueScenario();
  else if (config.scenario === 'scheduler') await schedulerScenario();
  else if (config.scenario === 'research') await researchScenario();
  else if (config.scenario === 'refinement') await refinementScenario();
  else {

  const repo = await request('/repositories', { name: 'Fixture', path: config.repository, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Resume saved work', description: '', repositoryId: repo.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Continue from a checkpoint', criteria: ['Keep session and pending steering'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  if (config.scenario === 'cancel-continuation') await request(`/runs/${run.id}/repository`, { mode: 'new' });
  await request(`/runs/${run.id}/start`, { team: [{ name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Create the first checkpoint.' }] });
  if (config.scenario === 'cancel-continuation') {
    await settled(run.id, 'CANCEL_CHECKPOINT');
    const before = await request(`/runs/${run.id}/repository`);
    assert.equal((await request(`/runs/${run.id}/cancel`, {})).status, 'cancelled');
    const stopped = await request(`/runs/${run.id}/runtime`);
    assert.ok(stopped.attempts.every((attempt: any) => attempt.ownership === 'stopped'));
    await app.close(); app = await startServer(options);
    const input = { requestId: crypto.randomUUID(), revision: 2, roleId: 'triage' };
    const next = await request(`/runs/${run.id}/continue-later`, input);
    assert.equal((await request(`/runs/${run.id}/continue-later`, input)).id, next.id);
    await settled(next.id, 'LINKED_SUCCESSOR_STARTED');
    const owned = await request(`/runs/${next.id}/repository`);
    assert.equal(owned.path, before.path); assert.equal(owned.branch, before.branch);
    const workspace = await request('/workspace');
    assert.equal(workspace.runs.find((item: Run) => item.id === run.id).status, 'cancelled');
    assert.equal(workspace.runs.find((item: Run) => item.id === run.id).continuedBy, next.id);
    const active = await request(`/runs/${next.id}/runtime`);
    assert.equal(active.assignments.length, 1);
    assert.ok(active.attempts.every((attempt: any) => !stopped.attempts.some((old: any) => old.id === attempt.id || old.sessionId === attempt.sessionId)));
    assert.equal((await request(`/runs/${next.id}/cancel`, {})).status, 'cancelled');
    console.log(JSON.stringify({ passed: true, scenario: config.scenario, predecessor: run.id, successor: next.id, sameWorktree: true, stoppedBeforeSuccessor: true }));
  } else if (config.scenario === 'budget-resume') {
    const initial = await settled(run.id, 'CHECKPOINT_COMPLETE');
    assert.equal(initial.attempts.length, 1);
    await request(`/runs/${run.id}/pause`, {});
    const steering = await request(`/runs/${run.id}/steering`, { requestId: 'retained-budget-note', body: 'Document the fixture.' });
    await app.close();
    // The saved model response makes replay exceed the configured context budget.
    app = await startServer(options);
    await request(`/runs/${run.id}/resume`, {});
    const deadline = Date.now() + 12000;
    let held;
    do { held = await observedRuntime(run.id); await delay(50); }
    while (!held.events.some((item: any) => item.event.type === 'model_policy_rejected' && item.event.code === 'input_budget') && Date.now() < deadline);
    assert.ok(held.events.some((item: any) => item.event.type === 'model_policy_rejected' && item.event.code === 'input_budget'));
    while (held.attempts.some((item: any) => item.ownership !== 'stopped') && Date.now() < deadline) { await delay(50); held = await observedRuntime(run.id); }
    assert.equal(held.attempts.length, 2); assert.equal(held.attempts[1].recoveryMode, 'session');
    assert.ok(held.attempts.every((item: any) => item.ownership === 'stopped'));
    await app.close(); app = await startServer(options);
    await request(`/runs/${run.id}/resume`, {});
    const continued = await settled(run.id, 'CONTINUED_FROM_CHECKPOINT');
    assert.equal(continued.attempts.length, 3);
    assert.equal(continued.attempts[2].recoveryMode, 'replacement');
    assert.notEqual(continued.attempts[2].sessionId, initial.attempts[0].sessionId);
    assert.equal(continued.assignments[0].automaticCycles ?? 0, 0, 'Explicit Resume consumes no automatic cycle.');
    assert.ok(continued.events.some((item: any) => JSON.stringify(item.event).includes('CHECKPOINT_COMPLETE')), 'Original history is retained for inspection.');
    assert.ok(readdirSync(initial.attempts[0].sessionDirectory).length > 0, 'The rejected session is retained.');
    const workspace = await request('/workspace');
    assert.equal(workspace.tasks.find((item: Task) => item.id === task.id).brief, 'Continue from a checkpoint');
    assert.ok(workspace.messages.some((item: Message) => item.id === steering.id && item.body === 'Document the fixture.'));
    await request(`/runs/${run.id}/cancel`, {});
    console.log(JSON.stringify({ passed: true, scenario: config.scenario, heldAttempt: held.attempts[1].id,
      freshAttempt: continued.attempts[2].id, retainedHistory: true, retainedSteering: steering.id }));
  } else if (config.scenario === 'adoption') {
    await settled(run.id, 'ADOPTION_CHECKPOINT');
    const current = (await request('/workspace')).tasks.find((item: Task) => item.id === task.id);
    const accepted = await request(`/tasks/${task.id}/refinement`, { revision: current.revision, sourceRevision: 1,
      brief: 'Keep the checkpoint and broaden the requirements', criteria: ['Keep session and pending steering', 'ADOPTED_SOURCE_CRITERION'] });
    assert.equal((await request('/workspace')).runs[0].taskRevision, 2, 'An accepted edit does not rewrite the running plan.');
    const adopted = await request(`/runs/${run.id}/requirements/adopt`, { requestId: crypto.randomUUID(), revision: accepted.revision, reason: 'Changed after coordinator checkpoint.' });
    assert.equal(adopted.status, 'paused'); assert.equal(adopted.taskRevision, accepted.revision);
    const message = (await request('/workspace')).messages.find((item: Message) => item.kind === 'requirements_adoption');
    assert.equal(message.delivery, 'pending');
    writeFileSync(`${config.root}/adoption.json`, JSON.stringify(message));
    const readyBy = Date.now() + 5000;
    while (!existsSync(`${config.root}/adoption-ready`) && Date.now() < readyBy) await delay(20);
    assert.ok(existsSync(`${config.root}/adoption-ready`), 'The provider prepared the matching steering receipt.');
    await app.close(); app = await startServer(options);
    assert.equal((await request('/workspace')).messages.find((item: Message) => item.id === message.id).delivery, 'pending');
    await request(`/runs/${run.id}/resume`, {});
    const observed = await settled(run.id, 'ADOPTED_REQUIREMENTS_SEEN');
    const delivered = (await request('/workspace')).messages.find((item: Message) => item.id === message.id);
    assert.equal(delivered.delivery, 'applied'); assert.match(delivered.dispositionReason, /Re-briefing/);
    assert.equal(observed.attempts.at(-1).generation, 2);
    console.log(JSON.stringify({ passed: true, scenario: config.scenario, adoptedRevision: adopted.taskRevision, message: delivered }));
  } else if (config.scenario === 'steering') {
    const deadline = Date.now() + 12000;
    while (!existsSync(`${config.root}/held-0`) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 30));
    assert.ok(existsSync(`${config.root}/held-0`));
    const note = await request(`/runs/${run.id}/steering`, { requestId: 'busy-note', body: 'Document the fixture.' });
    let delivered;
    do { delivered = (await request('/workspace')).messages.find((message: any) => message.id === note.id); await new Promise(resolve => setTimeout(resolve, 50)); }
    while (['pending', 'dispatched'].includes(delivered.delivery) && Date.now() < deadline);
    assert.equal(delivered.delivery, 'received', 'Busy steering must reach the runner without claiming application.');
    writeFileSync(`${config.root}/steering.json`, JSON.stringify(note));
    const result = await settled(run.id, 'STEERING_RECONCILED');
    delivered = (await request('/workspace')).messages.find((message: any) => message.id === note.id);
    assert.equal(delivered.delivery, 'applied'); assert.match(delivered.dispositionReason, /documentation/);
    assert.deepEqual(delivered.transitions.map((entry: { state: string }) => entry.state), ['dispatched', 'received', 'applied']);
    assert.ok(delivered.transitions.every((entry: { at?: string }) => entry.at));
    assert.ok(result.events.some((item: any) => item.event.type === 'prompt_result' && item.event.id === delivered.inputId));
    console.log(JSON.stringify({ passed: true, scenario: config.scenario, steering: delivered }));
  } else if (config.scenario !== 'session') {
    const count = config.scenario === 'cycles' ? 3 : 1;
    for (let cycle = 0; cycle < count; cycle++) {
      const deadline = Date.now() + 15000;
      while (!existsSync(`${config.root}/held-${cycle}`) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 50));
      assert.ok(existsSync(`${config.root}/held-${cycle}`), 'The replacement did not reach the provider.');
      const active = await request(`/runs/${run.id}/runtime`);
      process.kill(active.attempts.at(-1).ompPid, 'SIGKILL');
    }
    if (count === 1) {
      const result = await settled(run.id, 'CONTINUED_FROM_CHECKPOINT');
      assert.equal(result.attempts.length, 2);
      assert.equal(result.attempts[1].recoveryMode, 'replacement');
      assert.notEqual(result.attempts[0].sessionId, result.attempts[1].sessionId);
      assert.equal(result.assignments[0].automaticCycles, 1);
      assert.equal(result.attempts[0].ownership, 'stopped');
    } else {
      const deadline = Date.now() + 12000;
      let state;
      do { state = (await request('/workspace')).runs[0]; await new Promise(resolve => setTimeout(resolve, 50)); }
      while (state.status !== 'attention_required' && Date.now() < deadline);
      assert.equal(state.status, 'attention_required'); assert.match(state.attentionReason, /two automatic/i);
      const result = await request(`/runs/${run.id}/runtime`);
      assert.equal(result.attempts.length, 3); assert.equal(result.assignments[0].automaticCycles, 2);
      assert.ok(result.attempts.every((attempt: any) => attempt.ownership === 'stopped'));
    }
    const cancelled = await request(`/runs/${run.id}/cancel`, {});
    assert.equal(cancelled.status, 'cancelled');
    console.log(JSON.stringify({ passed: true, scenario: config.scenario }));
  } else {
  const initial = await settled(run.id, 'CHECKPOINT_COMPLETE');
  await request(`/runs/${run.id}/pause`, {});
  const steering = await request(`/runs/${run.id}/steering`, { requestId: 'paused-note', body: 'Document the fixture.' });
  const paused = (await request('/workspace')).runs[0];
  await new Promise(resolve => setTimeout(resolve, 150));
  assert.equal((await request('/workspace')).messages[0].delivery, 'pending');
  await app.close(); app = await startServer(options);
  assert.equal((await request('/workspace')).runs[0].activeMs, paused.activeMs);
  await request(`/runs/${run.id}/resume`, {});
  const continued = await settled(run.id, 'CONTINUED_FROM_CHECKPOINT');
  assert.equal(continued.attempts.length, 2);
  assert.equal(continued.attempts[0].ownership, 'stopped');
  assert.equal(continued.attempts[1].generation, 2);
  assert.equal(continued.attempts[1].sessionId, initial.attempts[0].sessionId);
  assert.notEqual(continued.attempts[1].ompPid, initial.attempts[0].ompPid);
  assert.equal(continued.attempts[1].recoveryMode, 'session');
  const delivered = (await request('/workspace')).messages.find((message: any) => message.id === steering.id);
  assert.equal(delivered.delivery, 'received');
  assert.deepEqual(delivered.transitions.map((entry: { state: string }) => entry.state), ['dispatched', 'received']);
  assert.ok(delivered.transitions.every((entry: { at?: string }) => entry.at));
  assert.equal(delivered.attemptId, continued.attempts[1].id);
  await request(`/runs/${run.id}/pause`, {});
  const resuming = fetch(app.origin + `/api/runs/${run.id}/resume`, { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: '{}' });
  const waitingBy = Date.now() + 5000;
  while ((await request('/workspace')).runs[0].status !== 'awaiting_runner' && Date.now() < waitingBy) await new Promise(resolve => setTimeout(resolve, 10));
  const cancelled = await request(`/runs/${run.id}/cancel`, {});
  assert.equal(cancelled.status, 'cancelled');
  assert.equal((await resuming).status, 409, 'A newer cancel must supersede an in-progress resume.');
  assert.equal((await request('/workspace')).runs[0].status, 'cancelled');
  const afterCancel = await request(`/runs/${run.id}/runtime`);
  assert.ok(afterCancel.attempts.every((attempt: any) => attempt.state === 'admitted' || attempt.ownership === 'stopped'));
  console.log(JSON.stringify({ passed: true, attempts: continued.attempts, steering: delivered, cancelSupersededResume: true }));
  }
  }
} finally { await app.close(); await provider?.close(); }
