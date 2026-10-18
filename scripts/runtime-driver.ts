import { fixtureRepository } from './fixture-repository';
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { startServer } from '../server/app';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createRuntimeObserver } from './runtime-observer';
const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const app = await startServer({ port: config.serverPort, dataDir: config.root, serveUi: false, rolePath: config.rolePath, runtimeConfig: config.runtime });
const request = async (path: string, body?: unknown) => {
  const response = await fetch(app.origin + '/api' + path, body === undefined ? {} : { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const result = await response.json(); if (!response.ok) throw new Error(JSON.stringify(result)); return result;
};
const observedRuntime = createRuntimeObserver(request);
try {
  let checkout = config.repository ? config.root : fixtureRepository(config.root);
  if (config.repository) {
    checkout = join(config.root, 'operator'); mkdirSync(checkout);
    const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { stdio: 'pipe' });
    git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
    writeFileSync(join(checkout, 'answer.txt'), 'original'); writeFileSync(join(checkout, 'AGENTS.md'), 'Retain the original answer until explicitly assigned.');
    git('add', '.'); git('commit', '-m', 'fixture');
  }
  const repository = await request('/repositories', { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = await request('/tasks', { title: 'Four hops', description: '', repositoryId: repository.id });
  await request(`/tasks/${task.id}/refinement`, { revision: 1, sourceRevision: 1, brief: 'Exchange peer messages', criteria: ['Reply returns to coordinator'] });
  const run = await request(`/tasks/${task.id}/runs`, { revision: 2 });
  const owned = config.repository ? await request(`/runs/${run.id}/repository`, { mode: 'new' }) : undefined;
  await request(`/runs/${run.id}/start`, { team: [
    { name: 'Coordinator', roleId: 'triage', coordinator: true, input: 'Send a message through SpecialistOne and SpecialistTwo, then receive the reply.' },
    { name: 'SpecialistOne', roleId: 'implementer', coordinator: false, input: 'Relay findings through SpecialistTwo and back to Coordinator.' },
    { name: 'SpecialistTwo', roleId: 'reviewer', coordinator: false, input: 'Reply to SpecialistOne.' },
  ] });
  if (config.fault === 'specialist') {
    const deadline = Date.now() + 10000; let before;
    do {
      before = await observedRuntime(run.id);
      if (existsSync(`${config.root}/specialist-held`) && before.attempts[0].state === 'idle') break;
      await new Promise(resolve => setTimeout(resolve, 50));
    } while (Date.now() < deadline);
    assert.ok(existsSync(`${config.root}/specialist-held`));
    process.kill(before.attempts[1].ompPid, 'SIGKILL');
  }
  const deadline = Date.now() + 30000; let state: any;
  while (Date.now() < deadline) {
    state = await observedRuntime(run.id);
    if (JSON.stringify(state.events).includes('ROUNDTRIP_COMPLETE')) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  assert.ok(JSON.stringify(state.events).includes('ROUNDTRIP_COMPLETE'), JSON.stringify({ attempts: state.attempts, messages: state.messages, error: state.error }));
  assert.equal(state.assignments.length, 3);
  const currentAttempts = state.assignments.map((assignment: any) => state.attempts.find((attempt: any) => attempt.id === assignment.currentAttemptId));
  if (config.fault === 'specialist') {
    assert.equal(state.attempts.length, 6);
    assert.equal(state.assignments[1].automaticCycles, 1);
    assert.equal(state.assignments[0].automaticCycles, 0);
    assert.equal(state.attempts[4].recoveryMode, 'replacement');
    assert.ok(state.attempts.slice(0, 3).every((attempt: any) => attempt.ownership === 'stopped'));
  }
  assert.equal(new Set(currentAttempts.map((a: any) => a.ompPid)).size, 3);
  assert.equal(new Set(currentAttempts.map((a: any) => a.hostPid)).size, 3);
  assert.equal(new Set(currentAttempts.map((a: any) => a.sessionId)).size, 3);
  assert.equal(state.messages.length, 4);
  assert.ok(state.messages.every((m: any) => m.transportId && m.disposition === 'received'));
  assert.ok(state.messages.slice(1).every((m: any, i: number) => m.replyTo === state.messages[i].id));
  // R5: each scripted provider response reports 10 input and 5 output tokens through the host gateway.
  const usage = (await request(`/runs/${run.id}/result`)).usage.rows.filter((row: any) => currentAttempts.some((attempt: any) => attempt.id === row.attemptId));
  assert.equal(usage.length, 3);
  for (const row of usage) {
    assert.equal(row.state, 'actual', JSON.stringify(row));
    assert.ok(row.actualResponses > 0); assert.equal(row.tokens.total, 15 * row.actualResponses); assert.equal(row.tokens.input, 10 * row.actualResponses);
  }
  const workspace = await request('/workspace');
  assert.equal(workspace.connections.runner, true);
  assert.equal(workspace.runs[0].status, 'running');
  assert.equal(workspace.activity.length, 3);
  const panes = execFileSync('tmux', ['-S', config.runtime.socket, 'list-panes', '-a', '-F', '#{pane_id} #{pane_pid}'], { encoding: 'utf8' });
  for (const attempt of currentAttempts) {
    assert.ok(panes.includes(`${attempt.pane} ${attempt.hostPid}`));
    assert.equal(Number(execFileSync('ps', ['-o', 'ppid=', '-p', String(attempt.ompPid)], { encoding: 'utf8' }).trim()), attempt.hostPid);
    const capture = execFileSync('tmux', ['-S', config.runtime.socket, 'capture-pane', '-p', '-S', '-', '-t', attempt.pane], { encoding: 'utf8' });
    assert.ok(capture.includes('host ready'), 'The pane shows readable host activity.');
  }
  const forbidden = await fetch(app.origin + '/internal/agent-message', { method: 'POST', headers: { Origin: app.origin, 'Content-Type': 'application/json', Authorization: 'Bearer forged' }, body: JSON.stringify({ attemptId: state.attempts[0].id, toolCallId: 'forged', arguments: { to: 'SpecialistOne', body: 'Unauthorized' } }) });
  assert.equal(forbidden.status, 409);
  assert.ok(!JSON.stringify(workspace).includes(config.runtime.mail.token));
  let repositoryProof;
  if (owned) {
    const boundary = new Workbench(join(config.root, 'workflow.sqlite'));
    try {
      const work = new RepositoryWork(boundary, join(config.root, 'worktrees'));
      await work.claimWriter(currentAttempts[1].id);
      writeFileSync(join(owned.path, 'answer.txt'), 'retained interrupted edit');
      const host = JSON.parse(readFileSync(join(config.runtime.root, currentAttempts[1].id, 'host.json'), 'utf8'));
      assert.ok(host.args.some((value: string) => value.includes('Retain the original answer')));
      assert.deepEqual(host.tools, [], 'Unscoped fixture commands must not enter an owned coding workspace.');
      await request(`/runs/${run.id}/pause`, {});
      const retained = await request(`/runs/${run.id}/repository`);
      assert.equal(retained.writer, undefined);
      assert.equal(retained.checkpoints.at(-1).reason, 'interrupted');
      assert.equal(retained.checkpoints.at(-1).attemptId, currentAttempts[1].id);
      assert.equal(retained.revision.dirty, true);
      assert.equal(readFileSync(join(checkout, 'answer.txt'), 'utf8'), 'original');
      assert.equal(readFileSync(join(owned.path, 'answer.txt'), 'utf8'), 'retained interrupted edit');
      assert.ok((await request(`/runs/${run.id}/runtime`)).attempts.every((attempt: any) => attempt.ownership === 'stopped'));
      repositoryProof = { branch: owned.branch, checkpoint: retained.checkpoints.at(-1), writerReleasedAfterStop: true, operatorUnchanged: true };
    } finally { boundary.close(); }
  }
  console.log(JSON.stringify({ passed: true, repositoryProof, origin: app.origin, runId: run.id, attempts: state.attempts, messages: state.messages, socket: config.runtime.socket }));
} finally { await app.close(); }
