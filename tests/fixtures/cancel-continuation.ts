import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { publicationFixture, reviewerSummary } from './publication';
import { Publication } from '../../server/publication';
import { RunContinuation } from '../../server/run-continuation';
import { defaultRoles } from '../../server/roles';
import type { BitbucketPullRequest } from '../../server/bitbucket-client';

/** Real local Git/store; simulated remote provider and controller evidence receipts. */
export async function cancelledFixture(root: string, published = false) {
  const f = await publicationFixture(root, { claimSlot: true });
  const roles = defaultRoles.map(role => ({ ...role, model: 'fixture/model', effort: 'low' as const }));
  const repository = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
  const policy = { statusKeys: ['build'], pipelineSteps: ['Unit Tests'], optionalSteps: [] };
  let pr: BitbucketPullRequest | undefined; let posts = 0, puts = 0, unavailable = false, losePut = false;
  const head = () => f.git(root, '--git-dir', f.remote, 'rev-parse', `refs/heads/${f.owned.branch}`);
  const provider = createServer(async (req, res) => {
    res.setHeader('Content-Type', 'application/json'); const url = new URL(req.url!, 'http://fixture');
    if (unavailable) { res.statusCode = 503; res.end('{}'); return; }
    if (url.pathname === '/2.0/repositories/fixture/repository') { res.end(JSON.stringify({ ...repository, full_name: 'fixture/repository' })); return; }
    let body = ''; for await (const chunk of req) body += chunk;
    if (req.method === 'POST') {
      posts++; const input = JSON.parse(body);
      pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-10-01T00:00:00Z',
        source: { branch: input.source.branch, commit: { hash: head() }, repository },
        destination: { branch: input.destination.branch, commit: { hash: f.candidate.revision.base }, repository },
        links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
      res.end(JSON.stringify(pr)); return;
    }
    if (pr) { pr.source.commit.hash = head(); pr.destination.commit.hash = f.git(root, '--git-dir', f.remote, 'rev-parse', 'refs/heads/main'); }
    if (req.method === 'PUT' && pr) {
      puts++; Object.assign(pr, JSON.parse(body));
      if (losePut) { losePut = false; req.socket.destroy(); return; }
      res.end(JSON.stringify(pr)); return;
    }
    if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr ? [pr] : [] })); return; }
    if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify(pr)); return; }
    res.end(JSON.stringify({ values: [] }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const connections = [{ repositoryPath: f.checkout, repository, policy, fixtureGitPath: f.remote,
    apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }];
  const publication = new Publication(f.store, f.work, f.evidence, join(root, 'publication'), connections);
  if (published) { await publication.prepare(f.coordinator.attempt.id, { candidateId: f.candidate.id, summary: reviewerSummary }); await publication.flush(); }
  const cancel = async (runId = f.run.id) => {
    f.store.setDesiredState(runId, 'cancelled'); f.store.beginPause(runId);
    for (const attempt of f.store.runtime(runId).attempts) f.store.recordOwnership(attempt.id, true);
    f.store.finishPause(runId); await f.work.reconcileStopped(runId);
    f.store.checkpointRepository(runId, { id: randomUUID(), at: new Date().toISOString(), reason: 'interrupted', revision: (await f.work.status(runId)).revision });
  };
  return { ...f, roles, connections, publication, cancel, continuation: new RunContinuation(f.store, f.work),
    request: () => ({ requestId: randomUUID(), revision: f.store.getTask(f.run.taskId).revision, roleId: 'coordinator' }),
    state: (state: BitbucketPullRequest['state']) => { pr!.state = state; }, outage: (value: boolean) => { unavailable = value; },
    loseUpdate: () => { losePut = true; }, counts: () => ({ posts, puts }),
    close: async () => { await publication.close(); f.store.close(); await new Promise<void>(resolve => provider.close(() => resolve())); } };
}
