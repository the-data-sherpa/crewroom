import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {once} from 'node:events';
import {readFileSync, writeFileSync} from 'node:fs';
import {join} from 'node:path';
import type {TestContext} from 'node:test';
import {spawnFixtureProcess} from './feature-owned-process';

/** Runs a real GPUI test process against a caller-owned disposable service. */
export async function nativeUi(t: TestContext, root: string, input: Record<string,unknown>, filter = 'native_external_operator_control') {
  const executable = process.env.CREWROOM_NATIVE_TEST_BINARY;
  assert.ok(executable, 'Run scripts/test-native-resume.sh to build the native driver');
  const name = `native-${input.action}-${randomUUID()}`;
  const plan = join(root, `${name}.json`), resultPath = join(root, `${name}-result.json`);
  writeFileSync(plan, JSON.stringify({...input, resultPath}), {mode:0o600});
  const child = spawnFixtureProcess('native-ui', executable,
    [filter, ...(filter.includes('::') ? ['--exact'] : []), '--ignored', '--test-threads=1', '--nocapture'],
    {...process.env, CREWROOM_NATIVE_JOURNEY_PLAN:plan});
  let output = ''; child.stdout.on('data', value => output += value); child.stderr.on('data', value => output += value);
  // Startup plus multiple bounded UI waits may exceed one minute on Linux.
  // Keep the driver's deadline outside those waits so failures retain diagnostics.
  const timer = setTimeout(() => child.kill('SIGKILL'), 120000);
  let code: number | null;
  try { [code] = await once(child, 'exit') as [number|null]; } finally {
    clearTimeout(timer);
    writeFileSync(join(root, `${name}.log`), output, {mode:0o600});
  }
  assert.equal(code, 0, output);
  if (process.env.CREWROOM_FEATURE_RESOURCE_ROOT) {
    assert.match(output, /test result: ok\. 1 passed; 0 failed; 0 ignored;/,
      'A feature action must execute exactly one native test');
  }
  const result = JSON.parse(readFileSync(resultPath, 'utf8')); assert.equal(result.error, '');
  t.diagnostic(`Native ${input.action}: ${output.trim()}`); return result;
}
