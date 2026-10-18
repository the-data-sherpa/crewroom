import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

test('LIF-01: starting an attempt claims it once and observations remain readable after reopen', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-runtime-'));
  seedRepository(root);
  let store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Observe', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Observe messages', criteria: ['Retain activity'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Start.' });
  store.claimAttempt(attempt.id);
  assert.throws(() => store.claimAttempt(attempt.id), /already claimed/);
  store.recordAttemptIdentity(attempt.id, { hostPid: 101, ompPid: 102, sessionId: 'session-1', hostUrl: 'http://127.0.0.1:4333', pane: '%0' });
  store.recordAgentEvent(attempt.id, 1, { type: 'message_start', message: { role: 'assistant' } });
  store.recordAgentEvent(attempt.id, 1, { type: 'message_start', message: { role: 'assistant' } });
  store.close(); store = new Workbench(join(root, 'db'));
  assert.equal(store.runtime(run.id).attempts[0].ompPid, 102);
  assert.equal(store.agentEvents(run.id).length, 1);
  assert.throws(() => store.claimAttempt(attempt.id), /already claimed/);
});

test('LIM-01/03: active time survives restart, excludes confirmed pause, and cannot reset on resume', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-budget-'));
  seedRepository(root);
  let now = 1000;
  let store = new Workbench(join(root, 'db'), () => now);
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Bound execution', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Preserve the budget', criteria: ['No reset on restart'] });
  const run = store.queue(task.id, { revision: 2, activeLimitMs: 1000 });
  store.startExecution(run.id);
  now = 1500; assert.equal(store.accountTime(run.id).activeMs, 500);
  store.beginPause(run.id);
  now = 1800; store.finishPause(run.id);
  assert.equal(store.getRun(run.id).activeMs, 800);
  now = 100000; store.close(); store = new Workbench(join(root, 'db'), () => now);
  assert.equal(store.accountTime(run.id).activeMs, 800);
  store.startExecution(run.id);
  now = 100300;
  const expired = store.accountTime(run.id);
  assert.equal(expired.activeMs, 1100);
  assert.equal(expired.status, 'pausing');
  assert.equal(expired.pauseReason, 'budget');
  store.finishPause(run.id);
  assert.throws(() => store.startExecution(run.id), /budget/i);
  store.setActiveLimit(run.id, { activeLimitMs: 2000 });
  store.startExecution(run.id);
  assert.equal(store.getRun(run.id).activeMs, 1100);
});

test('LIF-05/06: replacement requires stopped ownership, preserves history, and allows only two automatic cycles', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-replacement-'));
  seedRepository(root);
  let store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Continue work', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Preserve completed work', criteria: ['Bound replacement'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const original = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Keep this durable instruction.' });
  store.claimAttempt(original.attempt.id);
  assert.throws(() => store.replaceAttempt(original.assignment.id, { automatic: true }), /ownership/i);
  store.recordOwnership(original.attempt.id, true); store.beginPause(run.id); store.finishPause(run.id);
  const note = store.steer(run.id, { requestId: 'uncertain-note', body: 'Preserve delivery uncertainty.' });
  store.dispatchSteering(original.attempt.id, [note.id], 'lost-input');
  assert.throws(() => store.replaceAttempt(original.assignment.id, { automatic: true }), /uncertain/i);
  store.receiveSteering(original.attempt.id, 'lost-input');
  const second = store.replaceAttempt(original.assignment.id, { automatic: true });
  assert.equal(second.generation, 2); assert.equal(second.input, 'Keep this durable instruction.');
  assert.equal(second.previousAttemptId, original.attempt.id);
  store.claimAttempt(second.id); store.recordOwnership(second.id, true);
  store.close(); store = new Workbench(join(root, 'db'));
  const third = store.replaceAttempt(original.assignment.id, { automatic: true });
  store.claimAttempt(third.id); store.recordOwnership(third.id, true);
  assert.throws(() => store.replaceAttempt(original.assignment.id, { automatic: true }), /two automatic/i);
  assert.equal(store.runtime(run.id).attempts.length, 3);
  assert.equal(store.runtime(run.id).assignments[0].automaticCycles, 2);
  const manual = store.replaceAttempt(original.assignment.id, { automatic: false });
  assert.equal(manual.generation, 4);
  assert.equal(store.runtime(run.id).assignments[0].automaticCycles, 2);
  // U1: another automatic cycle requires an explicit, recorded, idempotent operator allowance.
  store.claimAttempt(manual.id); store.recordOwnership(manual.id, true);
  const requestId = '5b0c5f0e-8f0e-4f6a-9d8e-2f7a9f7f1a01';
  assert.throws(() => store.grantRepairCycle(run.id, original.assignment.id, { requestId, reason: '' }));
  store.grantRepairCycle(run.id, original.assignment.id, { requestId, reason: 'Browser fixture was at fault; allow one more repair.' });
  store.grantRepairCycle(run.id, original.assignment.id, { requestId, reason: 'Duplicate submit' });
  assert.equal(store.runtime(run.id).assignments[0].repairAllowances?.length, 1);
  store.close(); store = new Workbench(join(root, 'db'));
  const granted = store.replaceAttempt(original.assignment.id, { automatic: true });
  assert.equal(store.runtime(run.id).assignments[0].automaticCycles, 3);
  store.claimAttempt(granted.id); store.recordOwnership(granted.id, true);
  assert.throws(() => store.replaceAttempt(original.assignment.id, { automatic: true }), /All 3 automatic/);
});

test('LIF-04/LIM-02: cancellation is terminal and policy failure survives later idle events', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-control-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Stop work', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Retain cancelled work', criteria: ['No automatic restart'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Retain this input.' });
  store.claimAttempt(attempt.id);
  store.recordAgentEvent(attempt.id, 1, { type: 'model_policy_rejected', reason: 'Changed effort' });
  store.recordAgentEvent(attempt.id, 2, { type: 'session_settled' });
  assert.equal(store.getAttempt(attempt.id).state, 'failed');
  assert.equal(store.getRun(run.id).status, 'attention_required');
  store.setDesiredState(run.id, 'cancelled'); store.beginPause(run.id);
  store.recordOwnership(attempt.id, true); store.finishPause(run.id);
  assert.equal(store.getRun(run.id).status, 'cancelled');
  assert.throws(() => store.setDesiredState(run.id, 'running'), /cancelled/i);
  assert.throws(() => store.startExecution(run.id), /execution/i);
  assert.throws(() => store.steer(run.id, { requestId: 'late', body: 'Do more work' }), /cancelled/i);
  assert.equal(store.runtime(run.id).attempts[0].input, 'Retain this input.');
});

test('LIF-01: an invalid tmux socket path is rejected before claiming any process', async t => {
  const { RuntimeSupervisor } = await import('../server/runtime/supervisor');
  const root = mkdtempSync(join(tmpdir(), 'crewroom-socket-'));
  const store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  assert.throws(() => new RuntimeSupervisor(store, { root, socket: '/tmp/' + 'x'.repeat(120), ompBinary: '/unused', hostPorts: [4001, 4002, 4003, 4004],
    mail: { url: 'http://127.0.0.1:4005/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog: [], policy: {}, models: { providers: {} } }, 'http://127.0.0.1:4000'), /socket path/i);
  assert.equal(store.snapshot().runs.length, 0);
});

test('MSG-02/LIF-06: confirmed undispatched input moves to the new recipient attempt without inventing receipt', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-redelivery-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Retained input', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Recover undispatched input', criteria: ['Correct recipient generation'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const first = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Continue.' });
  const message = store.prepareMessage(first.attempt.id, 'self-note', { to: 'Coordinator', body: 'Reconcile this result.' });
  store.recordMessageStored(message.id, 1); store.acceptMessage(message.id); store.recordDispatch(message.id);
  assert.throws(() => store.restoreUndispatchedInput(first.attempt.id, message.id), /ownership/i);
  store.recordOwnership(first.attempt.id, true); store.beginPause(run.id); store.finishPause(run.id);
  store.restoreUndispatchedInput(first.attempt.id, message.id);
  assert.equal(store.getPeerMessage(message.id).disposition, 'accepted');
  const second = store.replaceAttempt(first.assignment.id, { automatic: false });
  store.recordDispatch(message.id);
  assert.equal(store.getPeerMessage(message.id).recipientAttemptId, second.id);
  assert.equal(store.getPeerMessage(message.id).disposition, 'dispatched');
});

test('R7/QUE-01: process ownership is per run; only one coding run may hold agents, with one writer per worktree', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-ownership-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const revision = { base: 'a'.repeat(40), head: 'a'.repeat(40), tree: 'b'.repeat(40), worktreeHash: 'c', dirty: false, requirementsHash: 'd', instructionsHash: 'e' };
  const checkpoint = (reason: 'prepared' | 'writer_started') => ({ id: randomUUID(), at: new Date().toISOString(), reason, revision });
  const admit = (title: string, coding: boolean) => {
    const task = store.createTask({ title, description: '', repositoryId: repo.id });
    store.refine(task.id, { revision: 1, sourceRevision: 1, brief: title, criteria: ['Own its processes'] });
    const run = store.queue(task.id, { revision: 2 });
    if (coding) {
      store.reserveRepositoryWork({ runId: run.id, repository: repo, commonDir: join(root, 'git'), path: join(root, run.id), branch: `fix/${run.id}`, targetRef: 'refs/heads/main',
        mode: 'new', base: revision.base, startHead: revision.head, requirementsHash: 'd', state: 'preparing', instructions: [], checkpoints: [] });
      store.confirmRepositoryWork(run.id, 'gitdir: fixture', [], checkpoint('prepared'));
    }
    store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate.' });
    const { attempt } = store.admitAssignment(run.id, { name: 'Implementer', role, model, coordinator: false, input: 'Implement.' });
    return { run, attempts: store.runtime(run.id).attempts.map(item => item.id), writer: attempt.id };
  };
  const execute = (target: { run: { id: string }; attempts: string[] }) => {
    for (const [index, id] of target.attempts.entries()) {
      store.claimAttempt(id);
      store.recordAttemptIdentity(id, { hostPid: 1000 + index, ompPid: 2000 + index, sessionId: `session-${id}`, hostUrl: 'http://127.0.0.1:1/', pane: `%${index}` });
    }
    store.setDesiredState(target.run.id, 'running'); store.startExecution(target.run.id);
  };
  const coding = admit('Coding', true), research = admit('Research', false), second = admit('Second coding', true);
  execute(coding);
  execute(research);
  assert.deepEqual(new Set(store.executingRuns().map(item => item.runId)), new Set([coding.run.id, research.run.id]));
  assert.throws(() => store.claimAttempt(second.attempts[0]), /one coding ticket/);
  assert.equal(store.getAttempt(second.attempts[0]).state, 'admitted');
  store.claimRepositoryWriter(coding.writer, checkpoint('writer_started'));
  store.setDesiredState(coding.run.id, 'paused'); store.beginPause(coding.run.id);
  for (const id of coding.attempts) store.recordOwnership(id, true);
  assert.equal(store.finishPause(coding.run.id).status, 'paused');
  assert.equal(store.repositoryWork(coding.run.id)?.writer?.attemptId, coding.writer, 'A paused run retains its worktree writer.');
  execute(second);
  assert.equal(store.claimRepositoryWriter(second.writer, checkpoint('writer_started')).writer?.attemptId, second.writer, 'Another worktree accepts its own writer.');
});
