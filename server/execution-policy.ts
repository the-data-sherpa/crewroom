import type { Run, Task } from '../shared/types';
import type { PublicationRecord } from '../shared/publication';
/**
 * First-release execution policy. Process ownership is checked per run; this policy
 * decides which runs may execute at the same time.
 */
export interface ExecutionPolicy { maxExecutingCodingRuns: number }

export const firstReleaseExecution: ExecutionPolicy = { maxExecutingCodingRuns: 1 };

/** A run that holds, reserves or may still hold agent processes. */
export interface ExecutingRun { runId: string; coding: boolean }

export function executionConflict(candidate: ExecutingRun, executing: ExecutingRun[], policy = firstReleaseExecution): string | undefined {
  if (!candidate.coding) return;
  const others = new Set(executing.filter(item => item.coding && item.runId !== candidate.runId).map(item => item.runId));
  if (others.size >= policy.maxExecutingCodingRuns) return 'Another coding ticket is executing. Crewroom executes one coding ticket at a time; wait until its agents stop.';
}

/** A dependency is released only by a provider-observed merge of a coding run. */
export function dependencyDelivery(task: Task, runs: Run[], publications: PublicationRecord[]): { state: 'merged' | 'open' | 'closed' | 'unknown' | 'none'; reason?: string; merge?: string } {
  const coding = runs.filter(run => run.taskId === task.id && (run.purpose ?? 'coding') === 'coding');
  if (!coding.length) return { state: 'none', reason: `${task.key}: no coding run or PR yet` };
  const records = coding.map(run => publications.find(record => record.runId === run.id)).filter((record): record is PublicationRecord => !!record);
  const merged = records.find(record => !!record.merged);
  if (merged) return { state: 'merged', merge: merged.merged!.commit };
  const latest = records.at(-1);
  if (!latest) return { state: 'none', reason: `${task.key}: coding run has no PR yet; a local branch or worktree cannot release it` };
  if (latest.pr.providerState === 'DECLINED' || latest.pr.providerState === 'SUPERSEDED')
    return { state: 'closed', reason: `${task.key}: PR closed without merge` };
  if (latest.pr.providerState === 'OPEN')
    return { state: 'open', reason: `${task.key}: PR open${latest.ready ? ' and ready for review' : ' or CI pending'}, but not merged` };
  if (latest.pr.state === 'pending' && ['pending', 'confirmed'].includes(latest.push.state))
    return { state: 'none', reason: `${task.key}: no delivered PR yet${latest.push.state === 'pending' ? '; only a local branch or worktree' : ''}` };
  return { state: 'unknown', reason: `${task.key}: provider PR or merge state unknown or unavailable` };
}

export function dependencyHold(_run: Run, task: Task, publications: PublicationRecord[], tasks: Task[], runs: Run[]): string | undefined {
  for (const id of task.dependsOn ?? []) {
    const dependency = tasks.find(item => item.id === id);
    if (!dependency) return `Dependency ${id} is unavailable`;
    const delivery = dependencyDelivery(dependency, runs, publications);
    if (delivery.reason) return delivery.reason;
  }
}
