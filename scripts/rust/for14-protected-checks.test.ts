/** Real networkless Docker/Chromium checks. These deterministic fixtures use no
 * model and must never be reported as live instruction-comparison coverage. */
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test, type TestContext } from 'node:test';
import { digest, materializeCase, readCasePack, type ComparisonCase } from './for14-case-pack';

const image = process.env.FOR14_EVALUATION_IMAGE;
const retention = process.env.FOR14_CHECK_RETENTION;
function fixture(t: TestContext, item: ComparisonCase, prior = false) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-for14-contained-'));
  materializeCase(item, root, prior);
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}
function run(item: ComparisonCase, checkId: string, root: string) {
  assert.match(image!, /^sha256:[a-f0-9]{64}$/);
  const name = 'crewroom-for14-check-' + randomUUID();
  const definition = item.requiredChecks.find(check => check.id === checkId)!;
  const args = ['run', '--name', name, '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
    '--user', `${process.getuid!()}:${process.getgid!()}`, '--env', 'HOME=/tmp', '--memory', '512m', '--pids-limit', '256',
    '--tmpfs', '/tmp:rw,exec,size=256m,mode=1777', '--mount', `type=bind,src=${root},dst=/workspace,readonly`,
    '--workdir', '/workspace', image!, '/usr/local/bin/node', '/opt/crewroom-checks/' + definition.script];
  const startedAt = new Date().toISOString();
  let result: ReturnType<typeof spawnSync> | undefined;
  let ownershipStopped = false;
  try {
    result = spawnSync('docker', args, { encoding: 'utf8', timeout: 45000, maxBuffer: 4 * 1024 * 1024 });
    const inspected = JSON.parse(execFileSync('docker', ['inspect', name], { encoding: 'utf8' }))[0];
    ownershipStopped = inspected.State.Running === false;
    assert.equal(inspected.HostConfig.NetworkMode, 'none');
    assert.equal(inspected.HostConfig.ReadonlyRootfs, true);
    assert.deepEqual(inspected.HostConfig.CapDrop, ['ALL']);
    assert.equal(inspected.Mounts.find((mount: { Destination: string }) => mount.Destination === '/workspace').RW, false);
    return { ...result, stdout: String(result.stdout), stderr: String(result.stderr) };
  } finally {
    const removed = spawnSync('docker', ['rm', '-f', name], { encoding: 'utf8', timeout: 10000 });
    const receipt = { version: 1, mode: 'deterministic-protected-check', caseId: item.id, checkId, image,
      definitionHash: definition.definitionHash, startedAt, endedAt: new Date().toISOString(),
      exitCode: result?.status, signal: result?.signal, output: String(result?.stdout ?? '') + String(result?.stderr ?? ''),
      cleanup: { ownershipStopped, removed: removed.status === 0 } };
    if (retention) {
      mkdirSync(resolve(retention), { recursive: true, mode: 0o700 });
      writeFileSync(join(resolve(retention), name + '.json'), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
    }
    assert.equal(ownershipStopped, true, 'Check ownership must stop without fallback cleanup');
    assert.equal(removed.status, 0, 'Disposable check container must be removed');
  }
}

test('protected image records a genuine previous-source pass and current-source failure', { skip: !image, timeout: 60000 }, t => {
  const item = readCasePack().cases[1];
  const prior = fixture(t, item, true), current = fixture(t, item);
  assert.notEqual(item.priorSourceHash, item.sourceHash);
  assert.equal(run(item, 'unit', prior).status, 0);
  assert.equal(run(item, 'unit', current).status, 1);
});

test('actual browser rejects untouched page and accepts final typing, Escape, focus and 390px layout', { skip: !image, timeout: 90000 }, t => {
  const item = readCasePack().cases[3], root = fixture(t, item);
  assert.equal(run(item, 'unit', root).status, 0);
  assert.equal(run(item, 'browser', root).status, 1);
  const path = join(root, 'index.html');
  writeFileSync(path, readFileSync(path, 'utf8')
    .replace('min-width:600px', 'min-width:0;width:100%')
    .replace('id="empty" hidden', 'id="empty" role="status" hidden')
    .replace("search.addEventListener('change', update);", "search.addEventListener('input', update);\nsearch.addEventListener('keydown', event => { if (event.key === 'Escape') { search.value = ''; update(); } });"));
  const passed = run(item, 'browser', root);
  assert.equal(passed.status, 0, passed.stderr);
  const body = JSON.parse(passed.stdout);
  assert.equal(body.surface, 'browser');
  assert.deepEqual(body.observations.map((observation: { width: number }) => observation.width), [1280, 390]);
  for (const observation of body.observations) {
    assert.equal(observation.typing, true);
    assert.equal(observation.focus, true);
    assert.equal(observation.noOverflow, true);
    const bytes = Buffer.from(observation.screenshot.base64, 'base64');
    assert.equal(bytes.length, observation.screenshot.bytes);
    assert.equal(digest(bytes), observation.screenshot.sha256);
    assert.equal(bytes.readUInt32BE(16), observation.width);
  }
});

test('workspace modules cannot alter the immutable executable check definition', { skip: !image, timeout: 60000 }, t => {
  const item = readCasePack().cases[0], root = fixture(t, item);
  writeFileSync(join(root, 'delivery.cjs'), "require('node:fs').writeFileSync('/opt/crewroom-checks/delivery-acceptance.cjs', 'process.exit(0)'); exports.deliveryLabel = () => 'wrong';\n");
  const result = run(item, 'acceptance', root);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /EROFS|EACCES|EPERM/);
});
