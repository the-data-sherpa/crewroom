import { attemptModel } from './runtime/types';
import type { Workbench } from './workbench';
import type { EvidenceArtifact } from '../shared/evidence';
import { runNextAction } from '../shared/run-history';
import { readRunUsage } from './run-usage';

export function recordedCause(event: Record<string, any>): string | undefined {
  switch (event.type) {
    case 'process_exit': return `Runner exited: ${event.code != null ? `code ${event.code}` : 'exit code unknown'}${event.signal ? ` · signal ${event.signal}` : ''}.`;
    case 'prompt_result': return event.status === 'error' ? `Turn failed: ${typeof event.error === 'string' ? event.error : event.error?.message ?? 'cause unknown; no error detail recorded'}` : undefined;
    case 'model_policy_rejected': return event.code === 'input_budget' ? `Provider input budget rejected: ${event.cause ?? 'cause unavailable'}. ${event.recovery === 'fresh_session' ? 'Explicit Resume starts a fresh model session.' : 'Correct the configuration before resuming.'}` : `Model policy rejected a provider request. Approved: ${event.requestedModel ?? 'unknown model'} · ${event.requestedEffort ?? 'unknown effort'}.`;
    case 'ownership_unresolved': return `Ownership unresolved: ${event.reason ?? 'cause unknown; no reason recorded'}`;
    default: return undefined;
  }
}
export interface AttemptEvidence {
  id: string; label: string; at?: string; revision: string; artifact?: EvidenceArtifact; detail?: string;
}

/** A read-only projection of durable records, independent of Git readiness and host availability. */
export function readRunHistory(store: Workbench, runId: string) {
  const run = store.getRun(runId); const runtime = store.runtime(runId); const coding = store.codingRecord(runId);
  const research = store.researchArtifacts(runId); const proposals = store.refinementProposals(runId); const usage = readRunUsage(store, runId);
  const events = store.agentEventsOfType(runId, ['process_exit', 'prompt_result', 'model_policy_rejected', 'ownership_unresolved']);
  const revision = (candidateId: string) => {
    const candidate = coding?.candidates.find(item => item.id === candidateId);
    return candidate ? `Head ${candidate.revision.head} · base ${candidate.revision.base} · tree ${candidate.revision.tree}` : 'Revision unknown: candidate unavailable';
  };
  return { usage, attempts: runtime.attempts.map(attempt => {
    const assignment = runtime.assignments.find(item => item.id === attempt.assignmentId)!;
    const current = assignment.currentAttemptId === attempt.id && !assignment.retiredAt;
    const causes = [...new Set(events.filter(item => item.attemptId === attempt.id).map(item => recordedCause(item.event)).filter((item): item is string => !!item))];
    for (const note of attempt.ownershipNotes ?? []) if (!causes.includes(note.reason)) causes.push(note.reason);
    if (attempt.ownershipReason && !causes.includes(attempt.ownershipReason)) causes.push(attempt.ownershipReason);
    if (assignment.retiredAt && assignment.currentAttemptId === attempt.id) causes.push(`Assignment retired: ${assignment.retirementReason ?? 'reason unknown; not recorded'}`);
    if (attempt.fallback) causes.push(`Configured fallback from ${attempt.fallback.fromModel}: ${attempt.fallback.reason}`);
    const needsCause = ['failed', 'exited', 'attention_required'].includes(attempt.state);
    if (!causes.length && needsCause) causes.push('Cause unknown: no reason was recorded for this attempt.');
    const evidence: AttemptEvidence[] = [
      ...(coding?.candidates.filter(item => item.implementationAttempts.includes(attempt.id)).map(item => ({ id: item.id, label: 'Implementation candidate', at: item.createdAt, revision: revision(item.id) })) ?? []),
      ...(coding?.checks.filter(item => item.attemptId === attempt.id).map(item => ({ id: item.id,
        label: `Check ${item.checkId} · ${!item.started || !item.completed ? 'incomplete' : item.timedOut ? 'timed out' : !item.sourceIntact ? 'source changed' : `exit ${item.exitCode}`}`,
        at: item.completedAt, revision: revision(item.candidateId), artifact: item.output })) ?? []),
      ...(coding?.reviews.filter(item => item.attemptId === attempt.id).map(item => ({ id: item.id, label: `Review · ${item.verdict}`, at: item.createdAt, revision: revision(item.candidateId), artifact: item.artifact, detail: item.summary })) ?? []),
      ...research.filter(item => item.attemptId === attempt.id).map(item => ({ id: item.id, label: `${item.title} · ${item.state}${item.supersededBy ? ' · superseded' : ''}`, at: item.createdAt,
        revision: `Requirements revision ${item.requirementRevision} · source ${item.pinnedCommit ?? 'not assigned'}`, artifact: item.body })),
      ...proposals.filter(item => item.attemptId === attempt.id).map(item => ({ id: item.id, label: `Refinement proposal · ${item.status}`, at: item.createdAt,
        revision: `Requirements revision ${item.baseRevision} · source revision ${item.sourceRevision}`, detail: `${item.brief}\nCriteria: ${item.criteria.join('; ')}\nDecisions: ${item.decisions.join('; ')}\nUnresolved questions: ${item.unresolvedQuestions.join('; ')}` })),
    ];
    return { model: attemptModel(assignment, attempt), retiredAt: assignment.retiredAt, retirementReason: assignment.retirementReason, state: attempt.state, ownership: attempt.ownership, attemptId: attempt.id, name: assignment.name, generation: attempt.generation, current, causes, evidence,
      nextAction: assignment.retiredAt ? 'Inspect this retired assignment and its retained evidence. It cannot resume.' : current ? runNextAction(run) : 'Inspect this retained attempt and its evidence. Use the current attempt for further work.',
      createdAt: attempt.createdAt ?? null, startedAt: attempt.startedAt ?? null, stoppedAt: attempt.stoppedAt ?? null };
  }) };
}
export type RunHistory = ReturnType<typeof readRunHistory>;
