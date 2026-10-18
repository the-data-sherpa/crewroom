// Differential oracle: the accepted TypeScript lifecycle supplies expected results.
import { writeFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';
import { transitionRun } from '../../server/run-lifecycle';
import type { Run } from '../../shared/types';
const cases: unknown[] = [];
const actions: Parameters<typeof transitionRun>[1][] = [
  { type: 'account' }, { type: 'account', waiting: true }, { type: 'start' },
  { type: 'beginPause' }, { type: 'beginPause', reason: 'review' },
  { type: 'finishPause', unresolved: false }, { type: 'finishPause', unresolved: true },
  ...(['running', 'paused', 'cancelled'] as const).map(desiredState => ({ type: 'desire' as const, desiredState })),
  { type: 'continue', controlRevision: 2 }, { type: 'continue', controlRevision: 3 },
  { type: 'reviewRegressed' }, { type: 'reviewRestored' }, { type: 'attention', reason: 'Lost ownership' },
  { type: 'setLimit', activeLimitMs: 3000 }, { type: 'agentStart' },
  { type: 'sourceChanged', reason: 'New requirements', active: true },
  { type: 'sourceChanged', reason: 'New requirements', active: false }, { type: 'close' }, { type: 'completeResearch' },
];
for (const status of ['awaiting_runner', 'running', 'pausing', 'paused', 'attention_required', 'cancelled', 'closed', 'completed'] as const)
for (const desiredState of ['running', 'paused', 'cancelled'] as const)
for (const purpose of ['coding', 'explore', 'research'] as const)
for (const clock of [{ activeMs: 0 }, { activeMs: 900, activeSince: 100 }, { activeMs: 400, activeWait: { kind: 'hosted-ci' as const, since: 100 } }])
for (const action of actions) {
  const previous: Run = { id: 'run', taskId: 'ticket', status, desiredState, purpose, createdAt: '2026-09-28', taskRevision: 1,
    taskSnapshot: {} as Run['taskSnapshot'], roles: [], stages: [], position: 0, controlRevision: 2,
    pauseReason: 'review', schedule: { id: 'decision', at: '2026-09-28' }, activeLimitMs: 1000, ...clock };
  try { cases.push({ previous, action, now: 500, expected: transitionRun(previous, action, 500) }); }
  catch (error) { cases.push({ previous, action, now: 500, error: (error as Error).message }); }
}
writeFileSync('crates/crewroom-core/tests/lifecycle-corpus.json.gz', gzipSync(JSON.stringify(cases), { level: 9 }));
console.log(`${cases.length} lifecycle transitions captured.`);
