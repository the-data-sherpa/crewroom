import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { PublicationGit } from '../server/publication-git';

test('DEL-03/07: real Git publication compares remote refs and cannot overwrite a competing branch', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-publish-git-')); const checkout = join(root, 'checkout'), remote = join(root, 'remote.git'); mkdirSync(checkout);
  const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  git(checkout, 'init', '-b', 'main'); git(checkout, 'config', 'user.name', 'Fixture'); git(checkout, 'config', 'user.email', 'fixture@example.invalid');
  git(root, 'init', '--bare', '-b', 'main', remote); writeFileSync(join(checkout, 'answer'), 'original'); git(checkout, 'add', '.'); git(checkout, 'commit', '-m', 'initial');
  const first = git(checkout, 'rev-parse', 'HEAD'); writeFileSync(join(checkout, 'answer'), 'change'); git(checkout, 'commit', '-am', 'change'); const head = git(checkout, 'rev-parse', 'HEAD');
  const transport = new PublicationGit(join(root, 'jobs'), { fixturePath: remote });
  t.after(async () => { await transport.close(); rmSync(root, { recursive: true, force: true }); });
  assert.equal(await transport.ref('fix/crewroom/test'), undefined);
  await transport.push({ directory: checkout, head, branch: 'fix/crewroom/test', expectedHead: null });
  assert.equal(await transport.ref('fix/crewroom/test'), head);
  assert.equal(git(checkout, 'rev-parse', 'HEAD'), head); assert.equal(git(checkout, 'branch', '--show-current'), 'main');
  await assert.rejects(transport.push({ directory: checkout, head: first, branch: 'fix/crewroom/test', expectedHead: null }));
  assert.equal(await transport.ref('fix/crewroom/test'), head, 'A stale expected ref cannot overwrite a competing update.');
  await assert.rejects(transport.push({ directory: checkout, head, branch: 'main', expectedHead: null }), /target|protected/);
  await transport.close(); const reopened = new PublicationGit(join(root, 'jobs'), { fixturePath: remote });
  try { assert.equal(await reopened.ref('fix/crewroom/test'), head); } finally { await reopened.close(); }
});
