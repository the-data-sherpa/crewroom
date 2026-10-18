import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { RepositoryWork } from '../server/repository-work';
import { Evidence } from '../server/evidence';
import { Research } from '../server/research';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

test('INT-07: a repository-free accepted research brief completes with honest source-free limitations', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-source-free-'));
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const source = new RepositoryWork(store, join(root, 'worktrees'));
  const research = new Research(store, source, new Evidence(store, source, join(root, 'artifacts')));
  const task = store.createTask({ title: 'Assess customer queue priorities', description: '' });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Document criteria without a repository', criteria: ['Define customer tradeoffs'] });
  const run = store.queueResearch(task.id, { revision: accepted.revision, requestId: crypto.randomUUID() });
  assert.equal(run.researchSource, undefined);
  await assert.rejects(source.readResearch(run.id, 'queue.md'), /no pinned repository/);
  const role = { ...defaultRoles.find(item => item.id === 'coordinator')!, model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Assess customer tradeoffs.' });
  store.claimAttempt(attempt.id);
  store.recordAttemptIdentity(attempt.id, { hostPid: 101, ompPid: 102, sessionId: 'research-no-repo', hostUrl: 'http://127.0.0.1:9', pane: '%1' });
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  const artifact = await research.submit(attempt.id, 'draft', { title: 'Tradeoffs', kind: 'research', body: '# Customer tradeoffs\nDifferent priorities need user interviews.',
    citations: [], limitations: 'No checked-in source or production data was supplied.', coverage: [{ criterionId: 0, section: 'Customer tradeoffs' }] });
  assert.equal(artifact.state, 'verified'); assert.equal(artifact.pinnedCommit, undefined);
  const request = research.requestCompletion(attempt.id, 'complete', {});
  store.recordOwnership(attempt.id, true); store.beginPause(run.id); store.finishPause(run.id);
  store.completeResearch(run.id, request.requestId, request.revision);
  assert.equal(research.read(run.id).assessment.completed, true);
  assert.equal(store.repositoryWork(run.id), undefined);
  assert.equal(store.codingRecord(run.id), undefined);
});
