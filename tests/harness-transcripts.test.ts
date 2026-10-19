import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

test('FOR-72: committed harness transcripts pass the redaction gate and re-recordings diff readably', () => {
  const result = spawnSync('python3', ['-m', 'unittest', 'discover', '-s', 'scripts', '-p', 'test_harness_transcripts.py'], { encoding: 'utf8', timeout: 120000 });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /Ran 7 tests/);
});
