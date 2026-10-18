import { join } from 'node:path';
import { Workbench } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';

/** Local workflow records, no model or provider delivery. */
export function steeringFixture(root: string) {
  let now = Date.parse('2026-10-01T15:00:00Z');
  const path = join(root, 'workflow.sqlite');
  const clock = () => now;
  const store = new Workbench(path, clock);
  const task = store.createTask({ title: 'Steering feedback', description: 'Keep every delivery state truthful.' });
  const run = store.queueRefinement(task.id, { revision: task.revision, purpose: 'explore' });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Reconcile steering.' });
  store.claimAttempt(attempt.id); store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  store.recordAttemptIdentity(attempt.id, { hostPid: 101, ompPid: 102, sessionId: 'fixture-session', hostUrl: 'http://127.0.0.1:59999', pane: '%1' });
  return { store, path, clock, run, attempt, tick: () => { now += 1000; return new Date(now).toISOString(); } };
}
