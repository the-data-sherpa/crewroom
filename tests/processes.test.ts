import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import type { CheckedProcess, ControllerAuthority } from '../server/runtime/processes';
import { acquireController, describeCheckedOwned, inspectCheckedOwned, launchCheckedOwned, prepareCheckedOwned, sealCheckedOwned, launchOwned, inspectOwned, prepareOwned, stopOwned } from '../server/runtime/processes';

function freezeCheckedDispatch(descriptor: CheckedProcess, marker: string) {
  const script = [
    'import fcntl,os,runpy,signal,sys',
    'from pathlib import Path',
    'helper,job,token,digest,epoch,controller,marker=sys.argv[1:]',
    'original=fcntl.flock',
    'def intercept(fd,mode):',
    '    result=original(fd,mode)',
    '    if mode==fcntl.LOCK_SH and os.fstat(fd).st_ino==os.stat(controller+"/dispatch.lock").st_ino:',
    '        Path(marker+".tmp").write_text("dispatch-held")',
    '        os.replace(marker+".tmp",marker)',
    '        os.kill(os.getpid(),signal.SIGSTOP)',
    '    return result',
    'fcntl.flock=intercept',
    'sys.argv=[helper,"checked-exec",job,token,digest,epoch,controller]',
    'runpy.run_path(helper,run_name="__main__")',
  ].join('\n');
  return spawn('python3', ['-c', script, resolve('server/runtime/process-lease.py'), descriptor.root, descriptor.token,
    descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot, marker], { detached: true, stdio: 'ignore' });
}

test('ownership inspection yields to other work while checking a live process', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-process-responsive-'));
  const child = launchOwned(join(root, 'job'), '/bin/sleep', ['30'], { cwd: root, env: { PATH: process.env.PATH } });
  t.after(async () => { await stopOwned(join(root, 'job')); child.stdin.end(); rmSync(root, { recursive: true, force: true }); });
  // Real process identity and lock acquisition occur in a separate OS process.
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline && (await inspectOwned(join(root, 'job'))).state !== 'live') {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  let timerFired = false;
  setImmediate(() => { timerFired = true; });
  assert.equal((await inspectOwned(join(root, 'job'))).state, 'live');
  assert.equal(timerFired, true, 'Inspection must not block the event loop during process probes.');
});

test('LIF-04: interruption stops a real tool and its child before releasing ownership', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-process-'));
  const output = join(root, 'progress');
  const child = launchOwned(join(root, 'job'), '/bin/sh', ['-c', 'while true; do echo working >> "$1"; sleep 0.05; done', 'worker', output], { cwd: root, env: { PATH: process.env.PATH } });
  t.after(async () => { await stopOwned(join(root, 'job')); child.stdin.end(); rmSync(root, { recursive: true, force: true }); });
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    try { if (readFileSync(output, 'utf8').length > 0) break; } catch {}
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.equal((await inspectOwned(join(root, 'job'))).state, 'live');
  const result = await stopOwned(join(root, 'job'));
  assert.equal(result.state, 'stopped', JSON.stringify(result));
  const progress = readFileSync(output, 'utf8');
  await new Promise(resolve => setTimeout(resolve, 150));
  assert.equal(readFileSync(output, 'utf8'), progress);
  assert.equal((await inspectOwned(join(root, 'job'))).state, 'stopped');
});

test('LIF-05: a surviving inherited lock blocks replacement after its leader exits', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-orphan-'));
  const output = join(root, 'child');
  const child = launchOwned(join(root, 'job'), '/bin/sh', ['-c', 'sleep 30 & echo $! > "$1"; exit', 'orphan', output], { cwd: root, env: { PATH: process.env.PATH } });
  await new Promise(resolve => child.once('exit', resolve));
  const pid = Number(readFileSync(output, 'utf8'));
  t.after(() => { try { process.kill(pid, 'SIGKILL'); } catch {} rmSync(root, { recursive: true, force: true }); });
  const result = await stopOwned(join(root, 'job'));
  assert.equal(result.state, 'uncertain');
  assert.ok(result.members?.includes(pid));
  assert.doesNotThrow(() => process.kill(pid, 0));
  process.kill(pid, 'SIGKILL');
  await new Promise(resolve => setTimeout(resolve, 80));
  assert.equal((await inspectOwned(join(root, 'job'))).state, 'stopped');
});

test('LIF-05: a stale PID identity is not signalled or treated as stopped', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-stale-'));
  const job = join(root, 'job');
  const child = launchOwned(job, '/bin/sleep', ['30'], { cwd: root, env: { PATH: process.env.PATH } });
  let original = '';
  for (let i = 0; i < 100; i++) { try { original = readFileSync(join(job, 'owner.json'), 'utf8'); break; } catch {} await new Promise(resolve => setTimeout(resolve, 20)); }
  t.after(async () => { writeFileSync(join(job, 'owner.json'), original); await stopOwned(job); child.stdin.end(); rmSync(root, { recursive: true, force: true }); });
  writeFileSync(join(job, 'owner.json'), JSON.stringify({ ...JSON.parse(original), started: 'stale process' }));
  assert.equal((await stopOwned(job)).state, 'uncertain');
  assert.doesNotThrow(() => process.kill(child.pid!, 0));
});

test('FOR-60: an owner receipt is never visible before it is complete', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-owner-receipt-')); const job = join(root, 'job'); const marker = join(root, 'writing');
  const [, helper] = prepareOwned(job, '/bin/sleep', ['30'], { cwd: root, env: { PATH: process.env.PATH } });
  // The real helper stops itself as it starts writing the receipt (the chmod right after creating the file),
  // which is the window an inspector watching for owner.json used to land in under load.
  const script = [
    'import os,runpy,signal,sys',
    'from pathlib import Path',
    'helper,job,marker=sys.argv[1:]',
    'original=os.chmod',
    'def intercept(path,mode,*args,**kwargs):',
    '    if Path(path).name.startswith("owner.json"):',
    '        Path(marker+".tmp").write_text(Path(path).name)',
    '        os.replace(marker+".tmp",marker)',
    '        os.kill(os.getpid(),signal.SIGSTOP)',
    '    return original(path,mode,*args,**kwargs)',
    'os.chmod=intercept',
    'sys.argv=[helper,"exec",job]',
    'runpy.run_path(helper,run_name="__main__")',
  ].join('\n');
  const frozen = spawn('python3', ['-c', script, helper, job, marker], { detached: true, stdio: 'ignore' });
  t.after(async () => {
    try { process.kill(frozen.pid!, 'SIGCONT'); } catch {}
    await stopOwned(job);
    if (frozen.exitCode === null && frozen.signalCode === null) try { process.kill(-frozen.pid!, 'SIGKILL'); } catch {}
    rmSync(root, { recursive: true, force: true });
  });
  const deadline = Date.now() + 10000;
  while (!existsSync(marker) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(readFileSync(marker, 'utf8'), 'owner.json.partial', 'The receipt is written under its partial name.');
  assert.equal(existsSync(join(job, 'owner.json')), false, 'A receipt still being written is not visible as owner.json.');
  process.kill(frozen.pid!, 'SIGCONT');
  let state = await inspectOwned(job);
  while (state.state !== 'live' && Date.now() < deadline) { await new Promise(resolve => setTimeout(resolve, 20)); state = await inspectOwned(job); }
  assert.equal(state.state, 'live', JSON.stringify(state));
  assert.equal(JSON.parse(readFileSync(join(job, 'owner.json'), 'utf8')).pid, frozen.pid);
  assert.equal(existsSync(join(job, 'owner.json.partial')), false);
  assert.equal((await stopOwned(job)).state, 'stopped');
});

test('FOR-60: a checked helper killed while writing its receipt leaves a lease no duplicate can execute', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-partial-'))); const job = join(root, 'job'); const output = join(root, 'count');
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo once >> "$1"', 'job', output], { cwd: root, env: { PATH: process.env.PATH } }, authority);
  // What a winner SIGKILLed between creating and publishing its receipt leaves behind; its lock died with it.
  writeFileSync(join(job, 'owner.json.partial'), '{"pi', { mode: 0o600 });
  const late = spawn('python3', [resolve('server/runtime/process-lease.py'), 'checked-exec', job, descriptor.token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot], { detached: true, stdio: ['ignore', 'ignore', 'pipe'] });
  let diagnostics = '';
  late.stderr.on('data', chunk => { diagnostics += chunk; });
  assert.notEqual((await once(late, 'close'))[0], 0);
  assert.match(diagnostics, /Process lease already held\./);
  assert.doesNotMatch(diagnostics, /FileExistsError|Traceback/);
  assert.equal(existsSync(output), false, 'The command never ran.');
  assert.equal(existsSync(join(job, 'owner.json')), false);
});

test('R3: concurrent ownership probes of a stopped process agree that it stopped', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-probe-')); const job = join(root, 'job');
  const child = launchOwned(job, '/bin/sh', ['-c', 'exit 0'], { cwd: root, env: { PATH: process.env.PATH } });
  await once(child, 'exit');
  // A real lease probe, frozen after answering while it keeps whatever lock it took, stands in for an overlapping observer.
  const frozen = spawn('python3', ['-c', 'import os, runpy, sys, time; sys.exit = lambda code=0: (sys.stdout.flush(), time.sleep(30)); sys.argv = ["process-lease.py", "probe", sys.argv[1]]; runpy.run_path(os.environ["LEASE"], run_name="__main__")', job],
    { env: { PATH: process.env.PATH, LEASE: resolve('server/runtime/process-lease.py') }, stdio: ['ignore', 'pipe', 'inherit'] });
  t.after(() => { frozen.kill('SIGKILL'); rmSync(root, { recursive: true, force: true }); });
  assert.equal(String((await once(frozen.stdout, 'data'))[0]).trim(), 'free');
  const result = await inspectOwned(job);
  assert.equal(result.state, 'stopped', JSON.stringify(result));
});

test('a sealed prepared launch cannot execute when its delayed helper resumes', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-delay-')));
  const job = join(root, 'job'); const marker = join(root, 'executed');
  const authority = await acquireController(join(root, 'controller'));
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo executed > "$1"', 'job', marker], { cwd: root, env: { PATH: process.env.PATH } }, authority);
  const delayed = spawn('python3', ['-c', 'import os,signal,sys; os.kill(os.getpid(), signal.SIGSTOP); os.execv(sys.executable, [sys.executable, *sys.argv[1:]])', resolve('server/runtime/process-lease.py'), 'checked-exec', job, descriptor.token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot], { detached: true, stdio: 'ignore' });
  t.after(async () => { try { process.kill(delayed.pid!, 'SIGKILL'); } catch {} try { process.kill(delayed.pid!, 'SIGCONT'); } catch {} await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline && !execFileSync('/bin/ps', ['-o', 'stat=', '-p', String(delayed.pid)], { encoding: 'utf8' }).includes('T')) await new Promise(resolve => setTimeout(resolve, 20));
  assert.ok(execFileSync('/bin/ps', ['-o', 'stat=', '-p', String(delayed.pid)], { encoding: 'utf8' }).includes('T'), 'Helper stopped before lease acquisition.');
  assert.equal((await sealCheckedOwned(descriptor)).state, 'stopped');
  delayed.kill('SIGCONT'); await once(delayed, 'exit');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
  assert.equal(readFileSync(join(job, 'seal.json'), 'utf8').includes(descriptor.specSha256), true);
  assert.equal(existsSync(marker), false);
});

test('an admitted manifest-less launch seals before a delayed helper can execute and is never rewritten', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-unprepared-')));
  const job = join(root, 'job'); const marker = join(root, 'executed');
  const authority = await acquireController(join(root, 'controller'));
  const token = randomUUID(); const args = ['-c', 'echo executed > "$1"', 'job', marker];
  const options = { cwd: root, env: { PATH: process.env.PATH } };
  const descriptor = describeCheckedOwned(job, token, '/bin/sh', args, options, authority);
  assert.equal(existsSync(job), false);
  const delayed = spawn('python3', ['-c', 'import os,signal,sys; print("waiting",flush=True); os.kill(os.getpid(),signal.SIGSTOP); os.execv(sys.executable,[sys.executable,*sys.argv[1:]])',
    resolve('server/runtime/process-lease.py'), 'checked-exec', job, token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot],
    { detached: true, stdio: ['ignore', 'pipe', 'ignore'] });
  t.after(async () => { try { delayed.kill('SIGKILL'); delayed.kill('SIGCONT'); } catch {} await authority.close(); rmSync(root, { recursive: true, force: true }); });
  assert.match(String((await once(delayed.stdout, 'data'))[0]), /waiting/);
  assert.equal((await sealCheckedOwned(descriptor)).state, 'stopped');
  assert.equal(existsSync(join(job, 'launch.json')), false);
  assert.throws(() => prepareCheckedOwned(job, token, '/bin/sh', args, options, authority));
  delayed.kill('SIGCONT'); assert.notEqual((await once(delayed, 'exit'))[0], 0);
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
  assert.equal(existsSync(marker), false);
});

test('a missing manifest after a real checked execution cannot be classified as never launched', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-lost-manifest-')));
  const job = join(root, 'job'); const marker = join(root, 'executed');
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo executed > "$1"', 'job', marker], { cwd: root, env: { PATH: process.env.PATH } }, authority);
  const child = launchCheckedOwned(descriptor, authority); child.stderr?.resume();
  assert.equal((await once(child, 'close'))[0], 0);
  assert.equal(readFileSync(marker, 'utf8'), 'executed\n');
  rmSync(join(job, 'launch.json'));
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'uncertain');
  assert.equal((await sealCheckedOwned(descriptor)).state, 'uncertain');
});

test('checked preparation binds the exact spec and duplicate helpers execute at most once', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-once-'))); const job = join(root, 'job'); const output = join(root, 'count');
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await stopOwned(job); await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const token = randomUUID();
  const args = ['-c', 'echo winner-output; echo winner-error >&2; echo once >> "$1"', 'job', output];
  const options = { cwd: root, env: { PATH: process.env.PATH } };
  const descriptor = prepareCheckedOwned(job, token, '/bin/sh', args, options, authority);
  assert.deepEqual(prepareCheckedOwned(job, token, '/bin/sh', args, options, authority), descriptor);
  assert.throws(() => prepareCheckedOwned(job, randomUUID(), '/bin/sh', args, options, authority));
  assert.throws(() => prepareCheckedOwned(job, token, '/bin/sh', ['-c', 'echo wrong'], options, authority));
  const reserved = join(root, 'reserved');
  mkdirSync(reserved);
  assert.throws(() => prepareCheckedOwned(reserved, randomUUID(), '/bin/sh', args, options, authority));
  assert.throws(() => prepareCheckedOwned(job, token, '/bin/sh', args, { cwd: root, env: { PATH: '/wrong' } }, authority));
  assert.throws(() => launchCheckedOwned({ ...descriptor, specSha256: '0'.repeat(64) }, authority));
  assert.throws(() => launchCheckedOwned({ ...descriptor, token: randomUUID() }, authority));
  const manifest = readFileSync(join(job, 'launch.json'), 'utf8');
  writeFileSync(join(job, 'launch.json'), manifest.replace('/bin/sh', '/bin/false'));
  assert.throws(() => launchCheckedOwned(descriptor, authority));
  assert.equal((await sealCheckedOwned(descriptor)).state, 'uncertain');
  writeFileSync(join(job, 'launch.json'), manifest);
  const substituted = spawn('python3', [resolve('server/runtime/process-lease.py'), 'checked-exec', job, randomUUID(), descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot], { detached: true, stdio: 'ignore' });
  assert.notEqual((await once(substituted, 'exit'))[0], 0);
  assert.equal(existsSync(output), false);
  const first = launchCheckedOwned(descriptor, authority); const second = launchCheckedOwned(descriptor, authority);
  const diagnostics = ['', ''];
  first.stderr?.on('data', chunk => { diagnostics[0] += chunk; });
  second.stderr?.on('data', chunk => { diagnostics[1] += chunk; });
  const exits = await Promise.all([once(first, 'exit'), once(second, 'exit')]);
  assert.equal(exits.filter(([code]) => code === 0).length, 1);
  const loser = exits.findIndex(([code]) => code !== 0);
  assert.match(diagnostics[loser], /Process lease already held\./);
  assert.ok(Buffer.byteLength(diagnostics[loser]) <= 512);
  assert.doesNotMatch(diagnostics[loser], /Traceback/);
  assert.equal(diagnostics[1 - loser], '');
  assert.equal(readFileSync(output, 'utf8'), 'once\n');
  assert.equal(readFileSync(join(job, 'stdout.log'), 'utf8'), 'winner-output\n');
  assert.equal(readFileSync(join(job, 'stderr.log'), 'utf8'), 'winner-error\n');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
  assert.throws(() => launchCheckedOwned(descriptor, authority));
  // A duplicate helper delayed until the winner has exited finds the lock free; it must
  // still refuse with the lease message instead of failing on the existing owner record.
  const late = spawn('python3', [resolve('server/runtime/process-lease.py'), 'checked-exec', job, descriptor.token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot], { detached: true, stdio: ['ignore', 'ignore', 'pipe'] });
  let lateDiagnostics = '';
  late.stderr.on('data', chunk => { lateDiagnostics += chunk; });
  assert.notEqual((await once(late, 'close'))[0], 0);
  assert.match(lateDiagnostics, /Process lease already held\./);
  assert.doesNotMatch(lateDiagnostics, /FileExistsError|Traceback/);
  assert.equal(readFileSync(output, 'utf8'), 'once\n');
  assert.equal(readFileSync(join(job, 'stdout.log'), 'utf8'), 'winner-output\n');
});

test('a prior controller cannot launch a checked intent after takeover', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-epoch-'))); const job = join(root, 'job'); const marker = join(root, 'executed');
  const old = await acquireController(join(root, 'controller'));
  let current: ControllerAuthority | undefined;
  t.after(async () => { await current?.close(); await old.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo stale > "$1"', 'job', marker], { cwd: root, env: { PATH: process.env.PATH } }, old);
  await assert.rejects(acquireController(join(root, 'controller')));
  await old.close(); await old.lost;
  assert.throws(() => old.assertCurrent());
  const second = await acquireController(join(root, 'controller'));
  current = second;
  assert.notEqual(second.epoch, old.epoch);
  assert.throws(() => launchCheckedOwned(descriptor, old));
  assert.throws(() => launchCheckedOwned(descriptor, second));
  const stale = spawn('python3', [resolve('server/runtime/process-lease.py'), 'checked-exec', job, descriptor.token, descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot], { detached: true, stdio: 'ignore' });
  const [code] = await once(stale, 'exit');
  assert.notEqual(code, 0);
  assert.equal(existsSync(marker), false);
  assert.equal((await sealCheckedOwned(descriptor)).state, 'stopped');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
  await second.close();
  current = await acquireController(join(root, 'controller'));
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
});

test('checked execution retains authenticated leader, descendant and private output evidence', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-live-'))); const job = join(root, 'job');
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await stopOwned(job); await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo retained; echo failure >&2; sleep 30'], { cwd: root, env: { PATH: process.env.PATH } }, authority);
  const child = launchCheckedOwned(descriptor, authority);
  let ownership = await inspectCheckedOwned(descriptor);
  const deadline = Date.now() + 5000;
  while (ownership.state !== 'live' && Date.now() < deadline) { await new Promise(resolve => setTimeout(resolve, 20)); ownership = await inspectCheckedOwned(descriptor); }
  assert.equal(ownership.state, 'live', JSON.stringify(ownership));
  assert.equal(ownership.owner?.pid, child.pid);
  assert.equal(ownership.owner?.token, descriptor.token);
  assert.ok(ownership.members?.length);
  assert.equal((await sealCheckedOwned(descriptor)).state, 'live');
  assert.equal((await stopOwned(job)).state, 'stopped');
  assert.equal((await sealCheckedOwned(descriptor)).state, 'stopped');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
  assert.equal(readFileSync(join(job, 'stdout.log'), 'utf8'), 'retained\n');
  assert.equal(readFileSync(join(job, 'stderr.log'), 'utf8'), 'failure\n');
});

test('a checked descendant keeps the lease live after its command exits', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-orphan-'))); const job = join(root, 'job'); const pidFile = join(root, 'pid');
  const authority = await acquireController(join(root, 'controller'));
  let pid = 0;
  t.after(async () => { if (pid) { try { process.kill(pid, 'SIGKILL'); } catch {} } await stopOwned(job); await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'sleep 30 & echo $! > "$1"; exit', 'job', pidFile], { cwd: root, env: { PATH: process.env.PATH } }, authority);
  const child = launchCheckedOwned(descriptor, authority);
  const exited = once(child, 'exit');
  const deadline = Date.now() + 5000;
  while (!existsSync(pidFile) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  pid = Number(readFileSync(pidFile, 'utf8'));
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'live');
  assert.equal((await sealCheckedOwned(descriptor)).state, 'live');
  assert.equal((await stopOwned(job)).state, 'stopped');
  await exited;
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
});

test('checked Git reset restores a large tracked file and large index without limiting repository writes', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-git-')));
  const job = join(root, 'job'); const repo = join(root, 'repo'); mkdirSync(repo);
  const git = (...args: string[]) => execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8' }).trim();
  git('init', '-q', '-b', 'main');
  git('config', 'user.name', 'Process Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  const tracked = join(repo, 'large.bin');
  const original = Buffer.from('original-byte-pattern-'.repeat(22000));
  const expected = createHash('sha256').update(original).digest('hex');
  writeFileSync(tracked, original);
  const entries = join(repo, 'entries'); mkdirSync(entries);
  for (let i = 0; i < 2400; i++) writeFileSync(join(entries, `entry-${String(i).padStart(4, '0')}`), `first-${i}\n`);
  git('add', '-A'); git('commit', '-qm', 'first');
  const first = git('rev-parse', 'HEAD');
  assert.ok(statSync(join(repo, '.git', 'index')).size > 128 * 1024, 'First revision has a real oversized index.');
  writeFileSync(tracked, Buffer.from('changed-byte-pattern-'.repeat(22000)));
  git('add', '-A'); git('commit', '-qm', 'second');
  const second = git('rev-parse', 'HEAD');
  assert.notEqual(first, second);
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await stopOwned(job); await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), execFileSync('/usr/bin/which', ['git'], { encoding: 'utf8' }).trim(),
    ['-C', repo, 'reset', '--keep', '--no-recurse-submodules', first],
    { cwd: repo, env: { PATH: process.env.PATH, HOME: root, GIT_CONFIG_NOSYSTEM: '1' } }, authority);
  const command = launchCheckedOwned(descriptor, authority);
  const [code, signal] = await once(command, 'exit');
  assert.equal(code, 0, `Git reset failed with signal ${signal}; ${readFileSync(join(job, 'stderr.log'), 'utf8')}`);
  assert.equal(git('rev-parse', 'HEAD'), first);
  const restored = readFileSync(tracked);
  assert.deepEqual(restored, original);
  assert.equal(createHash('sha256').update(restored).digest('hex'), expected);
  assert.ok(statSync(join(repo, '.git', 'index')).size > 128 * 1024, 'Restored index remains oversized.');
  assert.equal(git('status', '--porcelain'), '');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
});

test('checked capture drains alternating long streams without capping repository artifacts', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-output-')));
  const repo = join(root, 'repo'); mkdirSync(repo); execFileSync('git', ['init', '-q', repo]);
  const job = join(root, 'job'); const artifact = join(repo, 'artifact.bin'); const marker = join(repo, 'completed');
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await stopOwned(job); await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const script = 'import os,sys; from pathlib import Path; artifact,marker=sys.argv[1:]; '
    + 'Path(artifact).write_bytes(b"A"*400000); '
    + '[(os.write(1,b"O"*5003),os.write(2,b"E"*7001)) for _ in range(50)]; '
    + 'os.write(1,b"stdout-final\\n"); os.write(2,b"stderr-final\\n"); Path(marker).write_text("complete")';
  const python = execFileSync('/usr/bin/which', ['python3'], { encoding: 'utf8' }).trim();
  const descriptor = prepareCheckedOwned(job, randomUUID(), python, ['-c', script, artifact, marker],
    { cwd: repo, env: { PATH: process.env.PATH } }, authority);
  const child = launchCheckedOwned(descriptor, authority);
  const [code, signal] = await once(child, 'exit');
  assert.equal(code, 0, `Checked child failed: ${signal}`);
  assert.deepEqual(readFileSync(artifact), Buffer.alloc(400000, 'A'));
  assert.equal(readFileSync(marker, 'utf8'), 'complete');
  const cap = 128 * 1024;
  assert.deepEqual(readFileSync(join(job, 'stdout.log')), Buffer.alloc(cap, 'O'));
  assert.deepEqual(readFileSync(join(job, 'stderr.log')), Buffer.alloc(cap, 'E'));
  assert.deepEqual(JSON.parse(readFileSync(join(job, 'capture.json'), 'utf8')), {
    stdout: { retainedBytes: cap, truncated: true }, stderr: { retainedBytes: cap, truncated: true },
  });
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'stopped');
});

test('controller takeover does not hold dispatch until a checked command completes', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-takeover-')));
  const job = join(root, 'job'); const ready = join(root, 'ready');
  const first = await acquireController(join(root, 'controller'));
  let second: ControllerAuthority | undefined;
  t.after(async () => { await stopOwned(job); await second?.close(); await first.close(); rmSync(root, { recursive: true, force: true }); });
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo ready > "$1"; sleep 5', 'job', ready],
    { cwd: root, env: { PATH: process.env.PATH } }, first);
  const child = launchCheckedOwned(descriptor, first);
  const exited = once(child, 'exit');
  const deadline = Date.now() + 5000;
  while (!existsSync(ready) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(readFileSync(ready, 'utf8'), 'ready\n');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'live');
  await first.close();
  const started = Date.now();
  second = await acquireController(join(root, 'controller'));
  assert.ok(Date.now() - started < 2000, 'Acquiring the next controller must not wait for the checked command.');
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'live');
  assert.throws(() => launchCheckedOwned(descriptor, second!));
  assert.equal((await stopOwned(job)).state, 'stopped');
  await exited;
});

test('checked output stays bounded after its controller process dies', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-crash-')));
  const job = join(root, 'job'); const gate = join(root, 'gate'); const ready = join(root, 'ready');
  const artifact = join(root, 'artifact'); const done = join(root, 'done');
  let takeover: ControllerAuthority | undefined;
  t.after(async () => { await stopOwned(job); await takeover?.close(); rmSync(root, { recursive: true, force: true }); });
  const script = [
    'import { randomUUID } from "node:crypto";',
    'import { existsSync } from "node:fs";',
    'import { acquireController, launchCheckedOwned, prepareCheckedOwned } from "./server/runtime/processes.ts";',
    'const [root, job, gate, ready, artifact, done, python] = process.argv.slice(1);',
    'const authority = await acquireController(`${root}/controller`);',
    'const command = `import os,sys,time; from pathlib import Path; gate,ready,artifact,done=sys.argv[1:]; Path(ready).write_text("ready");\\nwhile not Path(gate).exists(): time.sleep(.02)\\nPath(artifact).write_bytes(b"A"*400000)\\nfor _ in range(50): os.write(1,b"O"*5003); os.write(2,b"E"*7001)\\nPath(done).write_text("complete")`;',
    'const descriptor = prepareCheckedOwned(job, randomUUID(), python, ["-c", command, gate, ready, artifact, done], { cwd: root, env: { PATH: process.env.PATH } }, authority);',
    'launchCheckedOwned(descriptor, authority);',
    'const deadline = Date.now() + 5000;',
    'while (!existsSync(ready) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));',
    'if (!existsSync(ready)) throw new Error("Checked child never started.");',
    'process.stdout.write(JSON.stringify(descriptor), () => process.exit(0));',
  ].join('\n');
  const python = execFileSync('/usr/bin/which', ['python3'], { encoding: 'utf8' }).trim();
  const controller = spawn(process.execPath, ['--import', 'tsx', '--input-type=module', '-e', script, root, job, gate, ready, artifact, done, python],
    { cwd: resolve('.'), env: { ...process.env, PATH: `/opt/homebrew/opt/node/bin:${process.env.PATH}` }, stdio: ['ignore', 'pipe', 'pipe'] });
  let raw = ''; let diagnostic = '';
  controller.stdout.on('data', chunk => { raw += chunk; });
  controller.stderr.on('data', chunk => { diagnostic += chunk; });
  const [code] = await once(controller, 'exit');
  assert.equal(code, 0, diagnostic);
  const descriptor = JSON.parse(raw);
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'live');
  takeover = await acquireController(join(root, 'controller'));
  assert.equal((await inspectCheckedOwned(descriptor)).state, 'live');
  writeFileSync(gate, 'go');
  const deadline = Date.now() + 5000;
  while (!existsSync(done) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(readFileSync(done, 'utf8'), 'complete');
  assert.deepEqual(readFileSync(artifact), Buffer.alloc(400000, 'A'));
  let ownership = await inspectCheckedOwned(descriptor);
  while (ownership.state !== 'stopped' && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 20)); ownership = await inspectCheckedOwned(descriptor);
  }
  assert.equal(ownership.state, 'stopped', JSON.stringify(ownership));
  const cap = 128 * 1024;
  assert.deepEqual(readFileSync(join(job, 'stdout.log')), Buffer.alloc(cap, 'O'));
  assert.deepEqual(readFileSync(join(job, 'stderr.log')), Buffer.alloc(cap, 'E'));
  assert.deepEqual(JSON.parse(readFileSync(join(job, 'capture.json'), 'utf8')), {
    stdout: { retainedBytes: cap, truncated: true }, stderr: { retainedBytes: cap, truncated: true },
  });
});

test('checked helper reports the child exit code and signal to its live parent', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-exits-')));
  const authority = await acquireController(join(root, 'controller'));
  t.after(async () => { await authority.close(); rmSync(root, { recursive: true, force: true }); });
  const failed = prepareCheckedOwned(join(root, 'failed'), randomUUID(), '/bin/sh', ['-c', 'exit 23'],
    { cwd: root, env: { PATH: process.env.PATH } }, authority);
  assert.deepEqual(await once(launchCheckedOwned(failed, authority), 'exit'), [23, null]);
  const signalled = prepareCheckedOwned(join(root, 'signalled'), randomUUID(), '/bin/sh', ['-c', 'kill -TERM $$'],
    { cwd: root, env: { PATH: process.env.PATH } }, authority);
  assert.deepEqual(await once(launchCheckedOwned(signalled, authority), 'exit'), [null, 'SIGTERM']);
  assert.equal((await inspectCheckedOwned(failed)).state, 'stopped');
  assert.equal((await inspectCheckedOwned(signalled)).state, 'stopped');
});

test('controller acquisition retries until a checked dispatch guard releases', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-dispatch-short-')));
  const job = join(root, 'job'); const marker = join(root, 'dispatch-held'); const commandMarker = join(root, 'executed');
  const old = await acquireController(join(root, 'controller'));
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo wrong > "$1"', 'job', commandMarker],
    { cwd: root, env: { PATH: process.env.PATH } }, old);
  const frozen = freezeCheckedDispatch(descriptor, marker);
  const exited = once(frozen, 'exit');
  let next: ControllerAuthority | undefined;
  let acquiring: Promise<ControllerAuthority> | undefined;
  t.after(async () => {
    try { frozen.kill('SIGCONT'); frozen.kill('SIGKILL'); } catch {}
    await acquiring?.then(async authority => { await authority.close(); }, () => {});
    await old.close(); rmSync(root, { recursive: true, force: true });
  });
  const deadline = Date.now() + 5000;
  while (!existsSync(marker) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(readFileSync(marker, 'utf8'), 'dispatch-held');
  await old.close(); await old.lost;
  acquiring = acquireController(join(root, 'controller'));
  // A real stopped OS process holds dispatch.lock SH; the contender must not acquire it early.
  const pending = await Promise.race([acquiring.then(() => 'acquired'), new Promise<string>(resolve => setTimeout(() => resolve('pending'), 120))]);
  assert.equal(pending, 'pending');
  frozen.kill('SIGCONT');
  assert.notEqual((await exited)[0], 0, 'Stale helper cannot run when its controller lease is gone.');
  next = await acquiring;
  assert.notEqual(next.epoch, old.epoch);
  assert.equal(existsSync(commandMarker), false);
  assert.equal((await sealCheckedOwned(descriptor)).state, 'stopped');
});

test('a frozen checked dispatch with a free controller lease fails acquisition with a bounded inspection error', async t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'crewroom-checked-dispatch-frozen-')));
  const job = join(root, 'job'); const marker = join(root, 'dispatch-held'); const commandMarker = join(root, 'executed');
  const old = await acquireController(join(root, 'controller'));
  const descriptor = prepareCheckedOwned(job, randomUUID(), '/bin/sh', ['-c', 'echo wrong > "$1"', 'job', commandMarker],
    { cwd: root, env: { PATH: process.env.PATH } }, old);
  const frozen = freezeCheckedDispatch(descriptor, marker);
  const exited = once(frozen, 'exit');
  let next: ControllerAuthority | undefined;
  t.after(async () => { try { frozen.kill('SIGCONT'); frozen.kill('SIGKILL'); } catch {} await next?.close(); await old.close(); rmSync(root, { recursive: true, force: true }); });
  const deadline = Date.now() + 5000;
  while (!existsSync(marker) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
  assert.equal(readFileSync(marker, 'utf8'), 'dispatch-held');
  await assert.rejects(acquireController(join(root, 'controller')), /Another controller owns this workbench/);
  await old.close(); await old.lost;
  const acquiring = acquireController(join(root, 'controller'));
  let timer!: NodeJS.Timeout;
  const watchdog = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error('Controller acquisition never returned while dispatch was frozen.')), 3000);
  });
  try {
    await assert.rejects(Promise.race([acquiring, watchdog]), /dispatch.*lease.*inspect.*retry/i);
  } finally {
    clearTimeout(timer);
    frozen.kill('SIGCONT');
    assert.notEqual((await exited)[0], 0);
    await acquiring.then(async authority => { await authority.close(); }, () => {});
  }
  assert.equal(existsSync(commandMarker), false);
  next = await acquireController(join(root, 'controller'));
  assert.equal((await sealCheckedOwned(descriptor)).state, 'stopped');
});
