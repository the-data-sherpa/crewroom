import { publicationFixture } from './publication';

/** Controlled host events and evidence receipts; no live provider or agent execution. */
export async function historyFixture(root: string, options: { fallback?: boolean } = {}) {
  const fixture = await publicationFixture(root); const { store, run } = fixture;
  const runtime = store.runtime(run.id); const reviewer = runtime.assignments.find(item => item.name === 'Reviewer')!;
  const oldAttemptId = reviewer.currentAttemptId;
  const retirement = store.proposeTeam(fixture.coordinator.attempt.id, 'retire-reporter', [], [{ name: 'Reporter', reason: 'Fixture reporting complete' }]);
  store.recordAgentEvent(oldAttemptId, 1, { type: 'provider_request' });
  store.recordAgentEvent(oldAttemptId, 2, { type: 'provider_usage', state: 'actual', usage: { input: 12, output: 8, cacheRead: 0, cacheWrite: 0, reasoning: 0, total: 20 } });
  store.recordAgentEvent(oldAttemptId, 3, { type: 'prompt_result', status: 'error', error: { message: 'Fixture turn interrupted', ...(options.fallback ? { provider: reviewer.model.resolved.provider, model: reviewer.model.resolved.id, retryable: true } : {}) } });
  store.recordAgentEvent(oldAttemptId, 4, { type: 'process_exit', code: null, signal: 'SIGTERM' });
  store.recordOwnership(oldAttemptId, false, 'Fixture process identity was uncertain');
  for (const attempt of runtime.attempts) store.recordOwnership(attempt.id, true);
  store.beginPause(run.id); store.finishPause(run.id);
  store.applyTeamProposal(retirement.id);
  const replacement = store.replaceAttempt(reviewer.id, { automatic: !!options.fallback, ...(options.fallback ? { fallback: { eventSequence: 3,
    model: { ...reviewer.model, resolved: { ...reviewer.model.resolved, id: 'alternate', selector: 'fixture/alternate' }, fallbackFrom: reviewer.model.resolved.selector } } } : {}) });
  store.claimAttempt(replacement.id);
  store.recordAttemptIdentity(replacement.id, { hostPid: 123, ompPid: 124, sessionId: 'replacement', hostUrl: 'http://127.0.0.1:59999', pane: '%9' });
  store.recordAgentEvent(replacement.id, 1, { type: 'provider_request' });
  store.recordOwnership(replacement.id, true);
  store.setDesiredState(run.id, 'cancelled'); store.beginPause(run.id); store.finishPause(run.id);
  const next = store.queue(run.taskId, { revision: store.getTask(run.taskId).revision });
  const otherTask = store.createTask({ title: 'Other ticket', description: '' });
  const foreign = store.queueRefinement(otherTask.id, { revision: otherTask.revision, purpose: 'explore' });
  return { ...fixture, oldAttemptId, replacement, next, foreign };
}
