import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/** A fresh Git root for process fixtures that need repository selection but no coding worktree. */
export function fixtureRepository(root: string) {
  const path = join(root, 'fixture-checkout'); mkdirSync(path);
  const git = (...args: string[]) => execFileSync('git', ['-C', path, ...args], { stdio: 'pipe' });
  git('init', '-b', 'main');
  git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(path, 'README.md'), 'Disposable process fixture.\n');
  git('add', 'README.md'); git('commit', '-m', 'Fixture');
  return path;
}
