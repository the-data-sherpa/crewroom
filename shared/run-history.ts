import type { Run } from './types';

/** Workflow explanations are independent of Jira status and live process observation. */
export function runCauses(run: Run, queueHold?: string): string[] {
  const reasons = [run.attentionReason, run.launchHold && `${run.launchHold.source === 'preflight' ? 'Launch' : 'Preparation'} held: ${run.launchHold.reason}`,
    run.feedbackResumeHold?.reason, run.sourceConflict, queueHold];
  if (run.readinessHold) reasons.push('Readiness is no longer confirmed; this run is held for reconciliation.');
  const pauses: Record<NonNullable<Run['pauseReason']>, string> = {
    operator: 'Paused by operator request.', budget: 'Active-time limit reached.', recovery: 'Paused for recovery.',
    source: 'Source requirements changed.', requirements: 'Paused to adopt requirements.', review: 'Paused at the human review gate.',
    validation: 'Validation no longer supports review readiness.', merged: 'Pull request merge recorded.',
  };
  if (run.pauseReason && run.desiredState !== 'cancelled') reasons.push(pauses[run.pauseReason]);
  const known = [...new Set(reasons.filter((reason): reason is string => !!reason))];
  return known.length ? known : ['paused', 'pausing', 'attention_required'].includes(run.status) ? ['Cause unknown: no reason was recorded.'] : [];
}

export function runNextAction(run: Run): string {
  if (['closed', 'completed', 'cancelled'].includes(run.status)) return 'Inspect retained history and evidence.';
  if (run.desiredState === 'cancelled') return 'Wait for confirmed stop or resolve the recorded ownership hold. This run cannot resume.';
  if (run.launchHold) return run.launchHold.source === 'preflight' ? 'Correct the launch prerequisites; the queue rechecks them automatically.' : 'Correct the preparation hold, then use Save launch details to retry.';
  if (run.feedbackResumeHold?.terminal) return 'Inspect the PR status and recorded feedback hold.';
  if (run.pauseReason === 'budget' || (run.activeMs ?? 0) >= (run.activeLimitMs ?? 3600000)) return 'Set a higher total active-time limit before requesting Resume.';
  if (run.resumeRequest) return 'Wait for the queued Resume; inspect the coding slot and process ownership if it remains held.';
  if (run.status === 'attention_required') return 'Inspect process and recovery details. Resolve ownership or delivery holds before requesting Resume.';
  if (run.status === 'paused') return 'Inspect the retained result; use Resume when ready and the runner is available.';
  if (run.status === 'pausing') return 'Wait for stop confirmation; inspect process ownership if it remains unresolved.';
  if (run.position !== undefined) return 'Inspect the queue hold and launch details; eligible coding plans start automatically.';
  return 'Inspect the coordinator conversation and saved evidence.';
}

export function runOutcome(run: Run): string {
  if (run.researchOutcome) return `Research outcome for revision ${run.researchOutcome.revision}`;
  if (run.slotRelease?.reason === 'merged' || run.pauseReason === 'merged') return 'Merge recorded';
  if (run.status === 'cancelled') return 'Cancelled; work retained';
  if (run.status === 'closed') return 'Refinement closed';
  if (run.status === 'completed') return 'Completed';
  return runCauses(run)[0] ?? 'No final outcome recorded';
}
