import { createServer } from 'node:http';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import type { PublicationRecord } from '../shared/publication';
import assert from 'node:assert/strict';

/** Only external Jira/Bitbucket boundaries are simulated. Git uses a real bare remote. */
export async function deliveryProviderFixture(root: string, checkout: string, port = 0, issueDescription = 'Return the sum, including empty input.') {
  const remote = join(root, 'remote.git');
  execFileSync('git', ['clone', '--bare', checkout, remote], { stdio: 'pipe' });
  const repository = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
  const policy = { statusKeys: ['pr-pipeline'], pipelineSteps: ['Unit Tests'], optionalSteps: ['Renovate'] };
  const fault = { bitbucket: 'forbidden', jira: 'outage', hideComments: false, pipeline: 'pending', hostedAt: '2026-09-28T00:00:00Z', pipelineRevision: undefined as { head: string; base: string } | undefined, pipelineNumber: 1 };
  const requests: string[] = []; const comments: any[] = []; let pr: any; let prPosts = 0, prPuts = 0, commentPosts = 0, transitionPosts = 0;
  const issue = () => ({ id: '100', key: 'SIM-1', fields: { summary: 'Correct sum behavior', description: issueDescription, status: { id: transitionPosts ? '3' : '1', name: transitionPosts ? 'In Progress' : 'To Do' } } });
  let issues = [issue()];
  const gitRef = (branch: string) => execFileSync('git', ['--git-dir', remote, 'rev-parse', `refs/heads/${branch}`], { encoding: 'utf8', stdio: 'pipe' }).trim();
  const server = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture'); requests.push(req.method + ' ' + url.pathname); res.setHeader('Content-Type', 'application/json');
    try {
      if (url.pathname.includes('/search/jql')) { res.end(JSON.stringify({ issues, isLast: true })); return; }
      if (url.pathname.startsWith('/rest/api/3/') && fault.jira === 'outage') { res.statusCode = 503; res.end('{}'); return; }
      if (url.pathname.endsWith('/issue/100')) { res.end(JSON.stringify(issue())); return; }
      if (req.method === 'GET' && /^\/rest\/api\/3\/issue\/[^/]+$/.test(url.pathname)) {
        const found = issues.find(item => url.pathname.endsWith(`/issue/${item.id}`));
        if (!found) { res.statusCode = 404; res.end('{}'); return; }
        res.end(JSON.stringify(found)); return;
      }
      if (url.pathname.endsWith('/issue/100/transitions')) {
        if (req.method === 'POST') { let body = ''; for await (const chunk of req) body += chunk; assert.equal(JSON.parse(body).transition.id, '6'); transitionPosts++; req.socket.destroy(); return; }
        res.end(JSON.stringify({ transitions: [{ id: '6', to: { id: '3', name: 'In Progress' }, fields: {} }] })); return;
      }
      if (url.pathname.endsWith('/issue/100/comment')) {
        if (fault.jira === 'outage') { res.statusCode = 503; res.end('{}'); return; }
        if (req.method === 'POST') {
          let body = ''; for await (const chunk of req) body += chunk;
          assert.ok(body.includes(pr.source.commit.hash)); assert.ok(body.includes(pr.links.html.href));
          comments.push({ ...JSON.parse(body), id: String(++commentPosts) }); req.socket.destroy(); return;
        }
        res.end(JSON.stringify({ comments: fault.hideComments ? [] : comments, startAt: 0, total: fault.hideComments ? 0 : comments.length })); return;
      }
      if (fault.bitbucket !== 'available') { res.statusCode = fault.bitbucket === 'forbidden' ? 403 : 503; res.end('{}'); return; }
      if (url.pathname === '/2.0/repositories/fixture/repository') { res.end(JSON.stringify({ ...repository, full_name: 'fixture/repository' })); return; }
      if (url.pathname.endsWith('/pullrequests') && req.method === 'POST') {
        let body = ''; for await (const chunk of req) body += chunk; const input = JSON.parse(body); prPosts++;
        assert.equal(gitRef(input.source.branch.name), input.source.commit.hash);
        pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-09-28T00:00:00Z',
          source: { branch: input.source.branch, commit: input.source.commit, repository }, destination: { branch: input.destination.branch, commit: { hash: gitRef('main') }, repository },
          links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
        req.socket.destroy(); return;
      }
      if(pr){pr.source.commit.hash=gitRef(pr.source.branch.name);pr.destination.commit.hash=gitRef('main');}
      if(req.method==='PUT' && url.pathname.endsWith('/pullrequests/9')){let body='';for await(const chunk of req)body+=chunk;const input=JSON.parse(body);pr.title=input.title;pr.description=input.description;prPuts++;res.end(JSON.stringify(pr));return;}
      if(url.pathname.endsWith('/pullrequests/9/comments')){res.end(JSON.stringify({values:fault.pipeline==='failed'?[{id:1,created_on:fault.hostedAt,updated_on:fault.hostedAt,content:{raw:'Current CI failed. Inspect before explicitly resuming.'},user:{display_name:'Fixture reviewer'},pullrequest:{id:9}}]:[]}));return;}
      if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr ? [pr] : [] })); return; }
      if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify(pr)); return; }
      if (url.pathname.endsWith('/statuses')) { res.end(JSON.stringify({ values: [{ key: 'pr-pipeline', name: 'PR pipeline', state: fault.pipeline === 'failed' ? 'FAILED' : 'SUCCESSFUL', updated_on: fault.hostedAt, url: 'https://example.invalid/check' }] })); return; }
      if (url.pathname.endsWith('/steps/')) { res.end(JSON.stringify({ values: [{ uuid: 'unit', name: 'Unit Tests', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } } }, { uuid: 'renovate', name: 'Renovate', state: { name: 'COMPLETED', result: { name: 'SKIPPED' } } }] })); return; }
      if (url.pathname.endsWith('/pipelines/')) { res.end(JSON.stringify({ values: fault.pipeline === 'pending' ? [] : [{ uuid: '{pipeline}', build_number: fault.pipelineNumber, created_on: fault.hostedAt, state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } }, target: { type: 'pipeline_pullrequest_target', source: pr.source.branch.name, destination: 'main', commit: {hash:fault.pipelineRevision?.head ?? pr.source.commit.hash}, destination_commit: {hash:fault.pipelineRevision?.base ?? pr.destination.commit.hash}, pullrequest: { id: 9 }, selector: { type: 'pull-requests', pattern: '**' } } }] })); return; }
      res.statusCode = 404; res.end('{}');
    } catch { res.statusCode = 500; res.end('{"error":"fixture assertion failed"}'); }
  });
  await new Promise<void>(resolve => server.listen(port, '127.0.0.1', resolve));
  const apiUrl = `http://127.0.0.1:${(server.address() as { port: number }).port}`;
  return { fault, requests, gitRef, remote, receipts: () => ({ prPosts, prPuts, commentPosts, transitionPosts, pr, comments }),
    setIssues: (next: typeof issues) => { issues = next; },
    seedPR: (record: PublicationRecord) => {
      execFileSync('git', ['--git-dir', remote, 'update-ref', `refs/heads/${record.branch}`, record.revision.head]);
      pr = { id: record.pr.id, title: record.title, description: record.description, state: 'OPEN', updated_on: '2026-09-28T00:00:00Z',
        source: { branch: { name: record.branch }, commit: { hash: record.revision.head }, repository },
        destination: { branch: { name: record.target }, commit: { hash: gitRef(record.target) }, repository },
        links: { html: { href: `https://bitbucket.org/fixture/repository/pull-requests/${record.pr.id}` } } };
      return createHash('sha256').update(record.description).digest('hex');
    },
    setPRState: (state: 'OPEN' | 'DECLINED' | 'MERGED', mergeCommit?: string) => {
      assert.ok(pr, 'Seed a PR before changing provider state.');
      pr.state = state; pr.updated_on = new Date().toISOString();
      if (state === 'MERGED') pr.merge_commit = { hash: mergeCommit ?? gitRef('main') };
      else delete pr.merge_commit;
    },
    options: { jira: { source: { siteUrl: 'https://fixture.atlassian.net', cloudId: '00000000-0000-4000-8000-000000000022', filterId: '12345', assigneeAccountId: 'operator', name: 'Synthetic delivery', inProgressStatusId: '3', inProgressTransitionId: '6' }, apiUrl, credentials: async () => 'private-jira-fixture-token' },
      publication: [{ repositoryPath: checkout, repository, policy, fixtureGitPath: remote, apiUrl, credentials: async () => 'private-bitbucket-fixture-token' }] },
    close: () => new Promise<void>(resolve => server.close(() => resolve())) };
}
