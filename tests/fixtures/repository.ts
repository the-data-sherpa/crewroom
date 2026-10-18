import { execFileSync } from 'node:child_process';
import { lstatSync, mkdirSync, realpathSync } from 'node:fs';
import { join } from 'node:path';
import { Workbench } from '../../server/workbench';
import { repositorySchema, type ValidatedRepository } from '../../server/repository-configuration';

/** A root checkout with a resolvable main ref, independent of the test's workflow database. */
export function seedRepository(path: string) {
  mkdirSync(path, { recursive: true });
  execFileSync('git', ['-C', path, 'init', '-b', 'main'], { stdio: 'ignore' });
  execFileSync('git', ['-C', path, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--allow-empty', '-m', 'fixture'], { stdio: 'ignore' });
}

/** Store-only fixtures still validate actual Git state; application requests use async preflight. */
export function saveRepositoryFixture(store: Workbench, input: unknown) {
  const replay = store.repositoryReplay(input);
  if (replay) return replay;
  const value = repositorySchema.parse(input);
  let path: string;
  try { path = realpathSync(value.path); }
  catch { throw new Error('Repository path must exist and be a Git root checkout.'); }
  const git = (...args: string[]) => execFileSync('git', ['-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false', '-c', 'submodule.recurse=false', ...args], {
    cwd: path, encoding: 'utf8', timeout: 10000, maxBuffer: 1024 * 1024,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, LANG: 'C', GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' },
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trimEnd();
  try {
    const metadata = lstatSync(join(path, '.git'));
    if (metadata.isSymbolicLink() || (!metadata.isDirectory() && !metadata.isFile())) throw new Error('Unsafe metadata');
    if (metadata.isFile() && store.isOwnedRepositoryWorktree(path)) throw new Error('A Crewroom-owned linked worktree cannot be configured as a repository.');
    if (git('rev-parse', '--show-toplevel') !== path) throw new Error('Not a root checkout');
  } catch (error) {
    if (error instanceof Error && error.message.startsWith('A Crewroom-owned')) throw error;
    throw new Error('Repository path must be the top level of an existing Git root checkout.');
  }
  try {
    git('check-ref-format', `refs/heads/${value.targetBranch}`);
    try { git('rev-parse', '--verify', '--end-of-options', `refs/heads/${value.targetBranch}^{commit}`); }
    catch { git('rev-parse', '--verify', '--end-of-options', `refs/remotes/${value.targetBranch}^{commit}`); }
  } catch { throw new Error('Target branch must resolve to a commit in this checkout (local branch or remote-tracking branch).'); }
  return store.saveRepository({ ...value, path } as ValidatedRepository);
}
