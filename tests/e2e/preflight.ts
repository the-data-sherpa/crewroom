import { spawnSync } from 'node:child_process';

type Check = { status: number | null; stderr: string; error?: Error };
/** FOR-59: stop once, naming the preparation step, instead of failing fixture-backed specs one by one. */
export function fixtureReadiness(run: () => Check = () => spawnSync('python3', ['scripts/prepare_worktree.py', '--check'], { encoding: 'utf8' })) {
  const result = run();
  if (result.error) return `python3 is required to check browser fixtures: ${result.error.message}`;
  return result.status === 0 ? undefined : `Browser fixtures are not prepared for this checkout.\n${result.stderr.trim()}`;
}

export default function preflight() {
  const problem = fixtureReadiness();
  if (problem) throw new Error(problem);
}
