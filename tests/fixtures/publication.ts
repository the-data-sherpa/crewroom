import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { Workbench } from '../../server/workbench';
import { saveRepositoryFixture } from './repository';
import { RepositoryWork } from '../../server/repository-work';
import { Evidence } from '../../server/evidence';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';

/** A reviewer-facing PR summary, as the coordinator writes through publish_change. */
export const reviewerSummary = '## Summary\n\nCorrect the answer file so the fixture returns the right value.\n\n## Testing\n\nThe unit check passes on this commit.';
/** Real Git/store services; explicit controller evidence fixtures, not a claim of executed container checks. */
export async function publicationFixture(root: string, options: { claimSlot?: boolean } = {}) {
  const checkout = join(root, 'checkout'), remote = join(root, 'remote.git'); mkdirSync(checkout);
  const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, GIT_EDITOR: 'true', GIT_SEQUENCE_EDITOR: 'true', GIT_TERMINAL_PROMPT: '0' } }).trim();
  git(checkout, 'init', '-b', 'main'); git(checkout, 'config', 'user.name', 'Fixture'); git(checkout, 'config', 'user.email', 'fixture@example.invalid');
  writeFileSync(join(checkout, 'answer'), 'original'); git(checkout, 'add', '.'); git(checkout, 'commit', '-m', 'initial'); git(root, 'init', '--bare', '-b', 'main', remote); git(checkout, 'push', remote, 'main');
  const store = new Workbench(join(root, 'workflow.sqlite')); const work = new RepositoryWork(store, join(root, 'owned')); const evidence = new Evidence(store, work, join(root, 'artifacts'));
  const repository = saveRepositoryFixture(store, { name: 'Fixture', path: checkout, targetBranch: 'main' });
  const task = store.importJira({ siteUrl: 'https://fixture.atlassian.net', issues: [{ id: '100', key: 'SIM-1', fields: { summary: 'Correct the answer', description: '', status: { name: 'To Do' } } }] })[0];
  store.assignRepository(task.id, { revision: 1, repositoryId: repository.id });
  store.refine(task.id, { revision: 2, sourceRevision: 1, brief: 'Return the right answer', criteria: ['Correct output'] }); const run = store.queue(task.id, { revision: 3 });
  if (options.claimSlot) store.claimCodingSlot(run.id, true);
  const owned = await work.prepare(run.id, { mode: 'new' });
  const role = { ...defaultRoles[1], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const coordinator = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate' });
  const writer = store.admitAssignment(run.id, { name: 'Writer', role, model, coordinator: false, input: 'Implement' });
  const reviewer = store.admitAssignment(run.id, { name: 'Reviewer', role: { ...role, id: 'reviewer' }, model, coordinator: false, input: 'Review' });
  const reporter = store.admitAssignment(run.id, { name: 'Reporter', role: { ...role, id: 'delivery-reporter' }, model, coordinator: false, input: 'Report delivery' });
  for (const [i, item] of [coordinator, writer, reviewer, reporter].entries()) { store.claimAttempt(item.attempt.id); store.recordAttemptIdentity(item.attempt.id, { hostPid: i + 1, ompPid: i + 4, sessionId: `fixture-${i}`, hostUrl: `http://127.0.0.1:${9000 + i}`, pane: `%${i}` }); }
  store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
  writeFileSync(join(owned.path, 'answer'), 'correct'); git(owned.path, 'commit', '-am', 'Fix answer'); store.recordOwnership(writer.attempt.id, true);
  const check = { id: 'unit', image: `sha256:${'a'.repeat(64)}`, command: '/bin/true', args: [], timeoutMs: 1000, required: true };
  evidence.configure(run.id, [check]); const candidate = await evidence.registerCandidate(run.id, [writer.attempt.id]);
  evidence.recordCheck(run.id, { candidateId: candidate.id, attemptId: reviewer.attempt.id, checkId: 'unit', definition: check, environment: { image: check.image, platform: 'fixture', network: 'none' }, exitCode: 0, timedOut: false,
    output: evidence.artifact('Controlled controller check receipt; process-backed checks are a separate integration gate.'), source: evidence.artifact(JSON.stringify(await work.listFiles(run.id))), sourceIntact: true, started: true, completed: true });
  evidence.assignReview(coordinator.attempt.id, 'review', { assignee: 'Reviewer', candidateId: candidate.id, instruction: 'Review this candidate' });
  evidence.review(reviewer.attempt.id, { candidateId: candidate.id, verdict: 'pass', summary: 'Correct output', findings: [], coverage: [{ criterion: 0, assessment: 'Reviewed output', checkIds: ['unit'] }] });
  return { checkout, remote, git, store, work, evidence, run, candidate, coordinator, reporter, owned };
}
