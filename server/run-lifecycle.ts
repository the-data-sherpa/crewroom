import type { Run } from '../shared/types';

export class LifecycleConflict extends Error {}

export type RunTransition =
  | { type: 'account' | 'start' | 'beginPause' | 'finishPause'; reason?: Run['pauseReason']; unresolved?: boolean; waiting?: boolean }
  | { type: 'desire'; desiredState: NonNullable<Run['desiredState']> }
  | { type: 'continue'; controlRevision: number }
  | { type: 'reviewRegressed' | 'reviewRestored' }
  | { type: 'attention'; reason: string }
  | { type: 'setLimit'; activeLimitMs: number }
  | { type: 'agentStart' }
  | { type: 'sourceChanged'; reason: string; active: boolean }
  | { type: 'close' | 'completeResearch' };

/** The sole authority for changing run execution state. Persistence and process ownership stay with the caller. */
export function transitionRun(previous: Run, action: RunTransition, now: number): Run {
  const run = { ...previous };
  if (previous.status === 'closed' || previous.status === 'completed') {
    if (action.type === 'account') return run;
    throw new LifecycleConflict('This run is already complete.');
  }
  const charge = () => {
    run.activeMs ??= 0;
    run.activeLimitMs ??= 60 * 60 * 1000;
    if (run.activeSince !== undefined) {
      const at = Math.max(now, run.activeSince);
      run.activeMs += at - run.activeSince;
      run.activeSince = at;
    }
    if (run.status === 'running' && run.activeMs >= run.activeLimitMs) {
      run.status = 'pausing'; run.pauseReason = 'budget';
    }
  };
  const resumeClock = () => {
    if (!run.activeWait) return;
    run.activeWait = undefined;
    if (['running', 'pausing', 'attention_required'].includes(run.status)) run.activeSince = now;
  };
  switch (action.type) {
    case 'completeResearch':
      if (run.purpose !== 'research' || run.status !== 'paused' || run.desiredState !== 'running') throw new LifecycleConflict('Stop and verify the research team before completing.');
      run.status = 'completed'; run.desiredState = 'cancelled'; run.activeSince = undefined;
      break;
    case 'close':
      if (run.purpose !== 'explore' && run.purpose !== 'grill') throw new LifecycleConflict('Only a refinement run can be closed.');
      if (run.status !== 'paused' && run.status !== 'cancelled' && run.status !== 'awaiting_runner') throw new LifecycleConflict('Stop the coordinator before closing refinement.');
      run.status = 'closed'; run.desiredState = 'cancelled'; run.activeSince = undefined;
      break;
    case 'account':
      if (!action.waiting) resumeClock();
      charge();
      if (action.waiting && run.status === 'running' && !run.activeWait) {
        run.activeWait = { kind: 'hosted-ci', since: now }; run.activeSince = undefined;
      }
      break;
    case 'desire':
      if (run.desiredState === 'cancelled' && action.desiredState !== 'cancelled') throw new LifecycleConflict('This run is cancelled. Its retained work requires a new explicit continuation.');
      if (run.desiredState !== action.desiredState) {
        run.desiredState = action.desiredState;
        run.controlRevision = (run.controlRevision ?? 0) + 1;
      }
      break;
    case 'reviewRegressed':
      if (run.status === 'paused' && run.desiredState === 'paused' && run.pauseReason === 'review') run.pauseReason = 'validation';
      break;
    case 'reviewRestored':
      if (run.status === 'paused' && run.desiredState === 'paused' && run.pauseReason === 'validation') run.pauseReason = 'review';
      break;
    case 'continue':
      if (run.status !== 'paused' || run.desiredState !== 'running' || run.controlRevision !== action.controlRevision) throw new LifecycleConflict('A newer control request superseded continuation.');
      run.status = 'awaiting_runner'; run.attentionReason = undefined;
      break;
    case 'attention':
      resumeClock();
      charge(); run.status = 'attention_required'; run.attentionReason = action.reason;
      break;
    case 'start':
      resumeClock();
      charge();
      if (run.activeMs! >= run.activeLimitMs!) throw new LifecycleConflict('The active execution budget is exhausted. Extend it explicitly before resuming.');
      if (run.desiredState === 'cancelled' || !['awaiting_runner', 'paused', 'running'].includes(run.status)) throw new LifecycleConflict('Run is not available for execution.');
      run.startedAt ??= new Date(now).toISOString();
      run.activeSince ??= now; run.status = 'running'; run.pauseReason = undefined;
      if ((run.purpose ?? 'coding') === 'coding' && run.schedule) run.launchCompletedAt ??= new Date(now).toISOString();
      break;
    case 'setLimit':
      charge(); run.activeLimitMs = action.activeLimitMs; charge();
      break;
    case 'beginPause':
      resumeClock();
      charge(); run.pauseReason = action.reason ?? (run.status === 'pausing' ? run.pauseReason : 'operator');
      run.status = 'pausing';
      break;
    case 'finishPause':
      if (action.unresolved) resumeClock();
      charge();
      run.status = action.unresolved ? 'attention_required' : run.desiredState === 'cancelled' ? 'cancelled' : 'paused';
      if (!action.unresolved) { run.activeSince = undefined; run.activeWait = undefined; }
      run.attentionReason = action.unresolved ? 'Process ownership is unresolved. Replacement work is blocked.' : undefined;
      break;
    case 'agentStart':
      resumeClock();
      if (run.status === 'awaiting_runner') { run.startedAt ??= new Date(now).toISOString(); run.status = 'running'; run.activeSince ??= now; }
      if ((run.purpose ?? 'coding') === 'coding' && run.schedule) run.launchCompletedAt ??= new Date(now).toISOString();
      break;
    case 'sourceChanged':
      if (run.desiredState !== 'cancelled') {
        run.sourceConflict = action.reason; run.attentionReason = action.reason;
        run.desiredState = 'paused'; run.pauseReason = 'source'; run.controlRevision = (run.controlRevision ?? 0) + 1;
        run.status = !action.active && previous.position !== undefined ? 'awaiting_runner' : action.active ? 'pausing' : 'attention_required';
      }
      break;
  }
  if (run.activeWait && run.status !== 'running') resumeClock();
  return run;
}
