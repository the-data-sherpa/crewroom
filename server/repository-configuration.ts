import { execFile } from 'node:child_process';
import { lstat, realpath } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { promisify } from 'node:util';
import { z } from 'zod';
import { checkSchema } from './check-policy';

const exec = promisify(execFile);
export class RepositoryValidationError extends Error {}
export const repositorySchema = z.object({
  name: z.string().trim().min(1).max(120), path: z.string().refine(isAbsolute, 'Use an absolute repository path.'),
  branchPrefix: z.string().max(100).regex(/^[A-Za-z0-9][A-Za-z0-9_-]*(?:\/[A-Za-z0-9][A-Za-z0-9_-]*)*\/$/, 'Use a branch prefix ending in /, with letters, numbers, hyphens or underscores.').optional(),
  targetBranch: z.string().trim().min(1).max(200), checks: z.array(checkSchema).max(100).refine(items => new Set(items.map(item => item.id)).size === items.length, 'Check IDs must be distinct.').optional(),
  requestId: z.string().uuid().optional(),
}).strict();
export const repositoryUpdateSchema = repositorySchema.omit({ requestId: true }).extend({
  revision: z.number().int().positive(), requestId: z.string().uuid(),
});

/** Only this Git preflight produces a production repository configuration for insertion. */
declare const validated: unique symbol;
export type ValidatedRepository = z.infer<typeof repositorySchema> & { readonly [validated]: true };

export async function validateRepository(input: unknown, isOwnedWorktree: (path: string) => boolean): Promise<ValidatedRepository> {
  const value = repositorySchema.parse(input);
  let path: string;
  try { path = await realpath(value.path); }
  catch { throw new RepositoryValidationError('Repository path must exist and be a Git root checkout.'); }
  const git = async (...args: string[]) => (await exec('git', ['-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false', '-c', 'submodule.recurse=false', ...args], {
    cwd: path, encoding: 'utf8', timeout: 10000, maxBuffer: 1024 * 1024,
    env: { PATH: process.env.PATH, HOME: process.env.HOME, LANG: 'C', GIT_TERMINAL_PROMPT: '0', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' },
  })).stdout.trimEnd();
  try {
    const metadata = await lstat(join(path, '.git'));
    if (metadata.isSymbolicLink() || (!metadata.isDirectory() && !metadata.isFile())) throw new Error('Unsafe metadata');
    if (metadata.isFile() && isOwnedWorktree(path)) throw new RepositoryValidationError('A Crewroom-owned linked worktree cannot be configured as a repository.');
    if (await git('rev-parse', '--show-toplevel') !== path) throw new Error('Not a root checkout');
  } catch (error) {
    if (error instanceof RepositoryValidationError) throw error;
    throw new RepositoryValidationError('Repository path must be the top level of an existing Git root checkout.');
  }
  try {
    await git('check-ref-format', `refs/heads/${value.targetBranch}`);
    try { await git('rev-parse', '--verify', '--end-of-options', `refs/heads/${value.targetBranch}^{commit}`); }
    catch { await git('rev-parse', '--verify', '--end-of-options', `refs/remotes/${value.targetBranch}^{commit}`); }
  } catch { throw new RepositoryValidationError('Target branch must resolve to a commit in this checkout (local branch or remote-tracking branch).'); }
  return { ...value, path } as ValidatedRepository;
}
