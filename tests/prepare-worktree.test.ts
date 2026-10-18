import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { fixtureReadiness } from './e2e/preflight';

test('FOR-59: the fresh-worktree preparation step links fixtures idempotently without tracked state', () => {
  const result = spawnSync('python3', ['-m', 'unittest', 'discover', '-s', 'scripts', '-p', 'test_prepare_worktree.py'], { encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /Ran 6 tests/);
});

test('FOR-59: missing browser fixtures stop the browser run once, naming the preparation step', () => {
  const missing = 'Missing Rust Mail test runtime at /x. Run `python3 scripts/prepare_worktree.py` (links an installed checkout\'s fixture)';
  const problem = fixtureReadiness(() => ({ status: 1, stderr: `${missing}\n` }));
  assert.equal(problem, `Browser fixtures are not prepared for this checkout.\n${missing}`);
  assert.equal(fixtureReadiness(() => ({ status: 0, stderr: '' })), undefined);
});
