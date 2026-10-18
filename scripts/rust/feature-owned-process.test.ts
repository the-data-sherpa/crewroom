import assert from 'node:assert/strict';
import {once} from 'node:events';
import {mkdtempSync, readFileSync, readdirSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import test from 'node:test';
import {spawnFixtureProcess} from './feature-owned-process';
import {inspectOwned, stopOwned} from './process-adapter';

test('owned native fixture launches exclude caller credentials from process and retained spec', async t => {
  const root = mkdtempSync(join(tmpdir(), 'cr-feature-env-'));
  const previous = process.env.CREWROOM_FEATURE_RESOURCE_ROOT;
  process.env.CREWROOM_FEATURE_RESOURCE_ROOT = root;
  t.after(async () => {
    if (previous === undefined) delete process.env.CREWROOM_FEATURE_RESOURCE_ROOT;
    else process.env.CREWROOM_FEATURE_RESOURCE_ROOT = previous;
    for (const name of readdirSync(root)) {
      const owned = join(root, name);
      if ((await inspectOwned(owned)).state !== 'stopped') await stopOwned(owned);
      assert.equal((await inspectOwned(owned)).state, 'stopped');
    }
    rmSync(root, {recursive:true, force:true});
  });
  for (const role of ['daemon', 'native-ui'] as const) {
    const child = spawnFixtureProcess(role, process.execPath,
      ['-e', 'process.stdout.write(JSON.stringify(process.env))'], {
        PATH:process.env.PATH, HOME:root, LANG:'C', CREWROOM_TEST_RUN:'fixture-id',
        CREWROOM_RUNTIME_CONFIG:join(root, 'runtime.json'),
        CREWROOM_NATIVE_JOURNEY_PLAN:join(root, 'plan.json'),
        CREWROOM_JIRA_TOKEN:'synthetic-must-not-retain',
        CREWROOM_BITBUCKET_TOKEN:'synthetic-must-not-retain',
        UNRELATED_SENTINEL:'synthetic-must-not-retain',
      });
    let output = '', stderr = '';
    child.stdout.on('data', value => output += value);
    child.stderr.on('data', value => stderr += value);
    const [code] = await once(child, 'exit');
    assert.equal(code, 0, stderr);
    const owned = join(root, readdirSync(root).find(name => name.startsWith(role + '-'))!);
    const launch = JSON.parse(readFileSync(join(owned, 'launch.json'), 'utf8'));
    for (const env of [JSON.parse(output), launch.env]) {
      assert.equal(env.HOME, root);
      assert.equal(env.CREWROOM_TEST_RUN, 'fixture-id');
      for (const key of ['CREWROOM_JIRA_TOKEN', 'CREWROOM_BITBUCKET_TOKEN', 'UNRELATED_SENTINEL']) assert.equal(env[key], undefined);
      assert.equal(env[role === 'daemon' ? 'CREWROOM_RUNTIME_CONFIG' : 'CREWROOM_NATIVE_JOURNEY_PLAN'],
        join(root, role === 'daemon' ? 'runtime.json' : 'plan.json'));
      assert.equal(env[role === 'daemon' ? 'CREWROOM_NATIVE_JOURNEY_PLAN' : 'CREWROOM_RUNTIME_CONFIG'], undefined);
    }
    assert.equal((await inspectOwned(owned)).state, 'stopped');
  }
});
