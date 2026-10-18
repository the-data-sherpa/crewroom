import { attemptModel } from './runtime/types';
import { createHash } from 'node:crypto';
import type { Workbench } from './workbench';
import type { TokenUsage } from './runtime/provider-usage';

/** Per-attempt usage. Tokens come only from provider usage receipts; OMP cost remains a runner estimate. */
export interface AttemptUsage {
  attemptId: string; assignmentId: string; name: string; generation: number; model: string;
  /** actual: every provider request has an actual usage report; partial: some do; unavailable: none do or none were recorded. */
  state: 'actual' | 'partial' | 'unavailable';
  awaitingStart: boolean;
  requestCounts?: 'available' | 'partial' | 'unavailable';
  costBasis?: 'cumulative-list-price-estimate';
  providerRequests: number; actualResponses: number; tokens: TokenUsage | null; unavailableReasons: string[];
  responses: number; costResponses: number; costUsd: number | null;
}

function addTokens(sum: TokenUsage | null, usage: TokenUsage): TokenUsage {
  return { input: (sum?.input ?? 0) + usage.input, output: (sum?.output ?? 0) + usage.output, cacheRead: (sum?.cacheRead ?? 0) + usage.cacheRead,
    cacheWrite: (sum?.cacheWrite ?? 0) + usage.cacheWrite, reasoning: (sum?.reasoning ?? 0) + usage.reasoning, total: (sum?.total ?? 0) + usage.total };
}

export function readRunUsage(store: Workbench, runId: string) {
  const runtime = store.runtime(runId); const seen = new Set<string>(); const receipts = new Map<string, number>();
  const rows = runtime.attempts.map((attempt): AttemptUsage => {
    const assignment = runtime.assignments.find(item => item.id === attempt.assignmentId)!;
    return { attemptId: attempt.id, assignmentId: assignment.id, name: assignment.name, generation: attempt.generation, model: attemptModel(assignment, attempt).resolved.selector,
      awaitingStart: attempt.state === 'admitted', state: 'unavailable', providerRequests: 0, actualResponses: 0, tokens: null, unavailableReasons: [], responses: 0, costResponses: 0, costUsd: null };
  });
  for (const item of store.agentEventsOfType(runId, ['provider_request', 'provider_usage', 'message_end'])) {
    const row = rows.find(row => row.attemptId === item.attemptId); if (!row) continue;
    if (item.event.type === 'provider_request') { row.providerRequests++; continue; }
    if (item.event.type === 'provider_usage') {
      receipts.set(row.attemptId, (receipts.get(row.attemptId) ?? 0) + 1);
      if (item.event.state === 'actual') {
        const usage: TokenUsage = item.event.usage; row.actualResponses++;
        const sum = row.tokens ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, total: 0 };
        row.tokens = { input: sum.input + usage.input, output: sum.output + usage.output, cacheRead: sum.cacheRead + usage.cacheRead,
          cacheWrite: sum.cacheWrite + usage.cacheWrite, reasoning: sum.reasoning + usage.reasoning, total: sum.total + usage.total };
      } else if (row.unavailableReasons.length < 3 && !row.unavailableReasons.includes(item.event.reason)) row.unavailableReasons.push(item.event.reason);
      continue;
    }
    const message = item.event.message;
    if (message?.role !== 'assistant') continue;
    const attempt = runtime.attempts.find(attempt => attempt.id === item.attemptId)!;
    const identity = message.timestamp === undefined ? `${attempt.id}:${item.sequence}` : `${attempt.sessionId}:${createHash('sha256').update(JSON.stringify(message)).digest('hex')}`;
    if (seen.has(identity)) continue; seen.add(identity);
    row.responses++;
    const assignment = runtime.assignments.find(assignment => assignment.id === attempt.assignmentId)!;
    if (message.provider !== attemptModel(assignment, attempt).resolved.provider || message.model !== attemptModel(assignment, attempt).resolved.id) continue;
    const cost = message.usage?.cost?.total;
    if (typeof cost === 'number' && Number.isFinite(cost) && cost >= 0) { row.costUsd = (row.costUsd ?? 0) + cost; row.costResponses++; }
  }
  for (const row of rows) {
    // A host that stops mid-request journals the request but never its receipt: that request stays unavailable.
    const missing = row.providerRequests - (receipts.get(row.attemptId) ?? 0);
    if (missing > 0) row.unavailableReasons.push(runtime.attempts.find(attempt => attempt.id === row.attemptId)?.ownership === 'stopped'
      ? `${missing} provider request(s) ended without a usage receipt.` : `${missing} provider request(s) have no usage receipt yet.`);
    if (!row.providerRequests) row.unavailableReasons.push('No provider request was recorded for this attempt.');
    row.state = row.providerRequests && row.actualResponses === row.providerRequests ? 'actual' : row.actualResponses ? 'partial' : 'unavailable';
  }
  const started = rows.filter(row => !row.awaitingStart);
  const totals = started.reduce((sum, row) => ({ providerRequests: sum.providerRequests + row.providerRequests, actualResponses: sum.actualResponses + row.actualResponses,
    tokens: row.tokens ? addTokens(sum.tokens, row.tokens) : sum.tokens, costUsd: row.costUsd === null ? sum.costUsd : (sum.costUsd ?? 0) + row.costUsd,
    responses: sum.responses + row.responses, costResponses: sum.costResponses + row.costResponses }),
    { providerRequests: 0, actualResponses: 0, tokens: null as TokenUsage | null, costUsd: null as number | null, responses: 0, costResponses: 0 });
  const state: AttemptUsage['state'] = started.length && started.every(row => row.state === 'actual') ? 'actual' : totals.actualResponses ? 'partial' : 'unavailable';
  const total = { ...totals, requestCounts: 'available' as const, state, incompleteAttempts: started.filter(row => row.state !== 'actual').map(row => ({ attemptId: row.attemptId, name: row.name, generation: row.generation })) };
  return { total, source: 'Provider response usage recorded by each agent host gateway', billedUsd: null, rows,
    note: 'Tokens are the provider\'s own usage reports, per attempt. A response without a report is unavailable, never zero. Cost is OMP\'s runner-pricing estimate, not provider billing.' };
}

export type RunUsage = ReturnType<typeof readRunUsage>;
