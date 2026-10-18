import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { ContainerCommand } from '../server/runtime/container-command';
import { pythonCheckImage, requireDockerEngine } from '../scripts/rust/docker-engine';
const engine = await requireDockerEngine();
const image = pythonCheckImage();

test('LIF-04/05: stop contains a detached child with closed descriptors and an empty environment', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-contained-'));
  const workspace = join(root, 'workspace'); mkdirSync(workspace, { mode: 0o700 });
  const job = new ContainerCommand(join(root, 'job'), engine);
  t.after(async () => { await job.stop(); await job.remove(); rmSync(root, { recursive: true, force: true }); });
  const writer = 'import time; from pathlib import Path; p=Path("/workspace/ticks");\nwhile True:\n with p.open("a") as f: f.write("tick\\n"); f.flush()\n time.sleep(0.03)';
  await job.start({ image, workspace, command: '/usr/local/bin/python3',
    args: ['-c', 'import subprocess,time,sys; subprocess.Popen([sys.executable,"-c",sys.argv[1]],start_new_session=True,close_fds=True,env={}); time.sleep(60)', writer] });
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) { try { if (readFileSync(join(workspace, 'ticks'), 'utf8')) break; } catch {} await new Promise(resolve => setTimeout(resolve, 30)); }
  assert.equal((await job.inspect()).state, 'running');
  assert.equal((await job.stop()).state, 'stopped');
  const ticks = readFileSync(join(workspace, 'ticks'), 'utf8');
  await new Promise(resolve => setTimeout(resolve, 200));
  assert.equal(readFileSync(join(workspace, 'ticks'), 'utf8'), ticks);
  // A new controller uses the durable identity, without an attached client or parent process.
  assert.equal((await new ContainerCommand(join(root, 'job'), engine).inspect()).state, 'stopped');
});

test('LIF-05: unavailable containment control stays uncertain and never admits the command twice', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-engine-loss-'));
  const workspace = join(root, 'workspace'); mkdirSync(workspace);
  const jobRoot = join(root, 'job');
  const job = new ContainerCommand(jobRoot, engine);
  t.after(async () => { await job.stop(); await job.remove(); rmSync(root, { recursive: true, force: true }); });
  const spec = { image, workspace, command: '/bin/sleep', args: ['60'] };
  await job.start(spec);
  const disconnected = new ContainerCommand(jobRoot, { socket: join(root, 'missing.sock') });
  assert.equal((await disconnected.stop()).state, 'uncertain');
  assert.equal((await job.inspect()).state, 'running');
  await assert.rejects(() => disconnected.start(spec), /durable intent/);
  assert.equal((await job.stop()).state, 'stopped');
});

test('EVD-02: command receipts prove execution, output, pinned environment and timeout', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-receipt-')); const workspace = join(root, 'workspace'); mkdirSync(workspace);
  const jobs: ContainerCommand[] = [];
  t.after(async () => { for (const job of jobs) { await job.stop(); await job.remove(); } rmSync(root, { recursive: true, force: true }); });
  const restrictedScratch = 'import subprocess; from pathlib import Path; p=Path("/tmp/not-admitted"); p.write_text("#!/bin/sh\\nexit 0\\n"); p.chmod(0o700)\ntry: subprocess.run([str(p)],check=True)\nexcept PermissionError: print("actual check output")\nelse: raise AssertionError("Temporary executables require explicit admission")';
  for (const [index, timeoutMs, script] of [[0, 5000, restrictedScratch], [1, 300, 'import time; print("check started", flush=True); time.sleep(30)']] as const) {
    const job = new ContainerCommand(join(root, String(index)), engine); jobs.push(job);
    await job.start({ image, workspace, command: '/usr/local/bin/python3', args: ['-c', script], timeoutMs });
    await job.wait();
    const receipt = await job.receipt();
    assert.equal(receipt.started, true);
    assert.match(receipt.platform, /^linux\//);
    // The command runs as the workspace owner, so CapDrop ALL does not deny writes to a private workspace on Linux.
    assert.equal(receipt.user, `${process.geteuid!()}:${process.getegid!()}`);
    assert.equal(receipt.timedOut, index === 1);
    assert.equal(receipt.completed, index === 0);
    if (index === 0) assert.equal(receipt.exitCode, 0);
    else assert.notEqual(receipt.exitCode, 0);
    assert.match(receipt.output, /actual check output|check started/);
    await job.remove();
    await job.stop();
    assert.deepEqual(await new ContainerCommand(join(root, String(index)), engine).receipt(), receipt, 'Removal retains the exact execution receipt and final logs.');
    assert.equal((await new ContainerCommand(join(root, String(index)), engine).receipt()).completed, index === 0, 'Later cleanup cannot erase a completed command receipt.');
  }
});

test('R2: an ad-hoc command reads the worktree through a read-only mount and cannot change it', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-readonly-')); const workspace = join(root, 'workspace'); mkdirSync(workspace);
  writeFileSync(join(workspace, 'source.txt'), 'original\n');
  const job = new ContainerCommand(join(root, 'job'), engine);
  t.after(async () => { await job.stop(); await job.remove(); rmSync(root, { recursive: true, force: true }); });
  await job.start({ image, workspace, command: '/bin/sh', readOnlyWorkspace: true,
    args: ['-c', 'cat /workspace/source.txt; echo changed > /workspace/source.txt && echo WRITE-ALLOWED; echo scratch > /tmp/x && cat /tmp/x'] });
  const state = await job.wait();
  const output = await job.logs();
  assert.equal(state.exitCode, 0);
  assert.match(output, /original/); assert.match(output, /scratch/);
  assert.doesNotMatch(output, /WRITE-ALLOWED/);
  assert.equal(readFileSync(join(workspace, 'source.txt'), 'utf8'), 'original\n');
});
