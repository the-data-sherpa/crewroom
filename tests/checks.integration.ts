import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import {homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { Workbench } from '../server/workbench';
import { saveRepositoryFixture } from './fixtures/repository';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { Checks } from '../server/checks';
import { ContainerCommand } from '../server/runtime/container-command';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

test('EVD-02: real container checks create controller-verified evidence for the exact candidate', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-evidence-')); const checkout = join(root, 'operator'); mkdirSync(checkout);
  const git = (...args: string[]) => execFileSync('git', ['-C', checkout, ...args], { encoding: 'utf8' }).trim();
  git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'sum.py'), 'def sum_values(xs): return sum(xs)\n'); git('add', '.'); git('commit', '-m', 'fixture');
  const store = new Workbench(join(root, 'db')); t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = store.createTask({ title: 'Sum empty inputs', description: '', repositoryId: repo.id }); store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Support zero', criteria: ['Empty input returns zero'] }); const run = store.queue(task.id, { revision: 2 });
  const work = new RepositoryWork(store, join(root, 'owned')); const owned = (await work.prepare(run.id, { mode: 'new' }));
  const role = { ...defaultRoles[1], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const writer = store.admitAssignment(run.id, { name: 'Writer', role, model, coordinator: false, input: 'Implement' });
  const reviewer = store.admitAssignment(run.id, { name: 'Reviewer', role: { ...role, id: 'reviewer' }, model, coordinator: false, input: 'Review independently' });
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role: { ...role, id: 'coordinator' }, model, coordinator: true, input: 'Coordinate' });
  for (const [i, item] of [writer, reviewer, coordinator].entries()) { store.claimAttempt(item.attempt.id); store.recordAttemptIdentity(item.attempt.id, { hostPid: i + 1, ompPid: i + 3, sessionId: `fixture-${i}`, hostUrl: `http://127.0.0.1:${5000 + i}`, pane: `%${i}` }); }
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const evidence = new Evidence(store, work, join(root, 'artifacts'));
  const engine = { socket: `${homedir()}/.docker/run/docker.sock` };
  const checks = new Checks(store, work, evidence, join(root, 'hosts'), engine);
  const required = { id: 'unit', image: 'sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285', command: '/usr/local/bin/python3', args: ['-B', '-c', 'from sum import sum_values; from pathlib import Path; import subprocess; assert int(Path("/sys/fs/cgroup/memory.max").read_text()) == 4294967296; child=Path("/tmp/check-child"); child.write_text("#!/bin/sh\\nexit 0\\n"); child.chmod(0o700); assert subprocess.run([str(child)],env={}).returncode == 0; assert sum_values([]) == 0; print("empty input passed")'], timeoutMs: 5000, memoryMiB: 4096, tmpExecutable: true, required: true };
  for (const memoryMiB of [0, 8193, 128.5]) assert.throws(() => evidence.configure(run.id, [{ ...required, memoryMiB }]));
  evidence.configure(run.id, [required]); store.recordOwnership(writer.attempt.id, true);
  const candidate = (await evidence.registerCandidate(run.id, [writer.attempt.id]));
  evidence.assignReview(coordinator.attempt.id, 'assign-review', { assignee: 'Reviewer', candidateId: candidate.id, instruction: 'Validate and review the candidate.' });
  const job = new ContainerCommand(checks.jobRoot(reviewer.attempt.id, 'actual-check'), engine);
  // Cleanup runs before SQLite/root cleanup registered above (Node runs after hooks in registration order).
  try {
    await assert.rejects(async () => (await checks.prepare(reviewer.attempt.id, 'forged', { checkId: 'unit', command: '/bin/true' })), /Unrecognized/i);
    await assert.rejects(async () => (await checks.prepare(reviewer.attempt.id, 'forged-memory', { checkId: 'unit', memoryMiB: 8192 })), /Unrecognized/i);
    await assert.rejects(async () => (await checks.prepare(reviewer.attempt.id, 'forged-tmp', { checkId: 'unit', tmpExecutable: true })), /Unrecognized/i);
    const receipt = await checks.execute(reviewer.attempt.id, 'actual-check', { checkId: 'unit' });
    assert.equal(receipt.candidateId, candidate.id); assert.equal(receipt.exitCode, 0);
    assert.equal(receipt.started, true); assert.equal(receipt.completed, true);
    assert.equal(receipt.environment.memoryMiB, 4096);
    assert.equal(receipt.environment.tmpExecutable, true);
    assert.throws(() => evidence.recordCheck(run.id, { ...receipt, environment: { ...receipt.environment, memoryMiB: 1024 } }), /environment/);
    assert.throws(() => evidence.recordCheck(run.id, { ...receipt, environment: { ...receipt.environment, tmpExecutable: false } }), /environment/);
    assert.match(evidence.readArtifact(receipt.output), /empty input passed/);
    assert.deepEqual(await checks.finish(reviewer.attempt.id, 'actual-check'), receipt);
    assert.deepEqual(await checks.execute(reviewer.attempt.id, 'actual-check', { checkId: 'unit' }), receipt, 'Retry reuses the stopped command rather than creating another.');
    store.updateCodingRecord(run.id, record => ({ ...record!, checkOperations: record!.checkOperations!.map(item => ({ ...item, state: 'prepared' as const, evidenceId: undefined })) }));
    assert.equal((await checks.finish(reviewer.attempt.id, 'actual-check')).id, receipt.id, 'Lost receipt adoption preserves one evidence identity.');
    assert.equal(store.codingRecord(run.id)!.checks.length, 1);
    assert.equal((await evidence.assess(run.id)).ready, false, 'An actual successful check still needs independent review.');
    evidence.review(reviewer.attempt.id, { candidateId: candidate.id, verdict: 'pass', summary: 'Confirmed empty input behavior.', findings: [], coverage: [{ criterion: 0, assessment: 'Actual unit check and source inspection agree.', checkIds: ['unit'] }] });
    assert.equal((await evidence.assess(run.id)).ready, true);
    const copy = store.codingRecord(run.id)!.checkOperations![0].workspace;
    writeFileSync(join(copy, 'sum.py'), 'check-local change');
    assert.equal((await evidence.assess(run.id)).ready, true, 'A later change to disposable check files does not alter the certified source.');
    const altered = (await checks.prepare(reviewer.attempt.id, 'altered-check', { checkId: 'unit' }));
    const alteredJob = new ContainerCommand(checks.jobRoot(reviewer.attempt.id, 'altered-check'), engine);
    try {
      await alteredJob.start(altered.spec); await alteredJob.wait();
      writeFileSync(join(altered.spec.workspace, 'sum.py'), 'source changed during check');
      const changed = await checks.finish(reviewer.attempt.id, 'altered-check');
      assert.equal(changed.sourceIntact, false);
      assert.equal((await evidence.assess(run.id)).ready, false, 'A zero exit code cannot certify a changed check snapshot.');
    } finally { await alteredJob.stop(); await alteredJob.remove(); }
    const interrupted = (await checks.prepare(reviewer.attempt.id, 'lost-check-reply', { checkId: 'unit' }));
    const interruptedJob = new ContainerCommand(checks.jobRoot(reviewer.attempt.id, 'lost-check-reply'), engine);
    try {
      await interruptedJob.start(interrupted.spec); await interruptedJob.wait();
      const before = (await interruptedJob.inspect()).id;
      store.recordOwnership(reviewer.attempt.id, true);
      await checks.reconcileStopped(run.id);
      const recovered = store.codingRecord(run.id)!.checkOperations!.find(item => item.id === interrupted.operationId)!;
      assert.equal(recovered.state, 'recorded');
      assert.equal(store.codingRecord(run.id)!.checks.find(item => item.id === recovered.evidenceId)!.exitCode, 0);
      await checks.reconcileStopped(run.id);
      assert.equal(store.codingRecord(run.id)!.checks.filter(item => item.id === recovered.evidenceId).length, 1);
      assert.equal((await interruptedJob.inspect()).id, before, 'Receipt recovery does not execute another check.');
    } finally { await interruptedJob.stop(); await interruptedJob.remove(); }
  } finally { await job.stop(); await job.remove(); }
});
