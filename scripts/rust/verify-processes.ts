/** Reuse the retained real-process suite against Rust, with explicit fixture-only adaptations. */
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const source = readFileSync('tests/processes.test.ts', 'utf8');
assert.equal((source.match(/^test\(/gm) ?? []).length, 19);
const directory = resolve(process.env.CREWROOM_NATIVE_PROCESS_RESULTS ?? '.crewroom/test-results/native-processes');
mkdirSync(directory, { recursive: true });
const adapter = resolve('scripts/rust/process-adapter.ts');
let native = source.replaceAll("'../server/runtime/processes'", JSON.stringify(adapter)).replaceAll('"./server/runtime/processes.ts"', JSON.stringify(adapter));
const start = native.indexOf('function freezeCheckedDispatch('); const end = native.indexOf("test('ownership inspection", start);
assert.ok(start > 0 && end > start);
// A fixture process holds the real dispatch inode through SIGSTOP and exec. On resume,
// the Rust helper performs its own epoch/lease check while takeover remains fenced.
native = native.slice(0, start) + `function freezeCheckedDispatch(descriptor: CheckedProcess, marker: string) {
  return spawn('python3', ['-c', [
    'import fcntl,os,signal,sys',
    'from pathlib import Path',
    'helper,job,token,digest,epoch,controller,marker=sys.argv[1:]',
    'fd=os.open(controller+"/dispatch.lock",os.O_RDONLY|os.O_NOFOLLOW)',
    'fcntl.flock(fd,fcntl.LOCK_SH)',
    'os.set_inheritable(fd,True)',
    'Path(marker).write_text("dispatch-held")',
    'os.kill(os.getpid(),signal.SIGSTOP)',
    'os.execv(helper,[helper,"checked-exec",job,token,digest,epoch,controller])',
  ].join('\\n'), resolve('target/debug/crewroom-process'), descriptor.root, descriptor.token,
    descriptor.specSha256, descriptor.epoch, descriptor.controllerRoot, marker], { detached: true, stdio: 'ignore' });
}\n\n` + native.slice(end);
native = native.replaceAll("resolve('server/runtime/process-lease.py'), 'checked-exec'", "resolve('target/debug/crewroom-process'), 'checked-exec'")
  .replaceAll("spawn('python3', [resolve('target/debug/crewroom-process'), 'checked-exec'", "spawn(resolve('target/debug/crewroom-process'), ['checked-exec'")
  .replaceAll('os.execv(sys.executable, [sys.executable, *sys.argv[1:]])', 'os.execv(sys.argv[1],sys.argv[1:])')
  .replaceAll('os.execv(sys.executable,[sys.executable,*sys.argv[1:]])', 'os.execv(sys.argv[1],sys.argv[1:])')
  .replace('/Process lease already held\\./', '/Process lease already held|already launched/');
const file = resolve(directory, 'processes.test.ts'); writeFileSync(file, native);
const result = spawnSync(process.execPath, ['--import', 'tsx', '--test', file, resolve('scripts/rust/process-extra.test.ts')], { encoding: 'utf8', timeout: 240000, maxBuffer: 4 * 1024 * 1024 });
process.stdout.write(result.stdout ?? ''); process.stderr.write(result.stderr ?? '');
const receipt = { status: result.status === 0 ? 'passed' : 'failed', cases: 25, sourceSha256: createHash('sha256').update(source).digest('hex'), adaptedSha256: createHash('sha256').update(native).digest('hex'), helper: 'target/debug/crewroom-process', adaptations: [
  'Calls use a test-only CLI adapter to Rust; original process assertions are retained.',
  'Python controls delayed/frozen fixture processes only; checked execution is Rust. The concurrent shared probe remains Python to verify lock interoperability.',
  'Frozen dispatch holds a real inherited shared lock across SIGSTOP and exec, then invokes Rust epoch/lease validation.',
  'Duplicate rejection accepts either a held lease or an already-written owner, while requiring one execution and unchanged winning logs.',
], claims: 'Actual local processes, inherited locks, signals, bounded logs, disposable Git and controller death. No OMP, model, Mail, Docker, native workflow admission or live provider proof.' };
writeFileSync(resolve(directory, 'receipt.json'), JSON.stringify(receipt, null, 2) + '\n');
console.log(JSON.stringify(receipt, null, 2));
assert.equal(result.status, 0, result.error?.message ?? 'Native process contracts failed');
