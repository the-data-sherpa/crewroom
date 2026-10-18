/** Retained Bitbucket wire cases exercised against the native client. */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { NativeBitbucketClient as BitbucketClient } from './provider-client-adapter';
const repository = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
const head = 'a'.repeat(40), base = 'b'.repeat(40);
const pr = (id: number) => ({ id, title: 'SIM-1: Fix zero', description: 'Crewroom publication fixture', state: 'OPEN', updated_on: '2026-09-28T00:00:00Z',
  source: { branch: { name: 'fix/crewroom/test' }, commit: { hash: head }, repository: { uuid: repository.uuid } },
  destination: { branch: { name: 'main' }, commit: { hash: base }, repository: { uuid: repository.uuid } },
  links: { html: { href: `https://bitbucket.org/fixture/repository/pull-requests/${id}` } } });

test('Native DEL-03/04: Bitbucket reads all matching PR pages and never retries a lost create response', async t => {
  let posts = 0; const ledger: string[] = [];
  const provider = createServer((req, res) => {
    const url = new URL(req.url!, 'http://fixture'); ledger.push(req.method + ' ' + url.pathname);
    assert.equal(req.headers.authorization, 'Bearer synthetic');
    if (req.method === 'POST') { posts++; req.socket.destroy(); return; }
    const second = url.searchParams.has('page');
    res.end(JSON.stringify({ values: [pr(second ? 2 : 1)], ...(second ? {} : { next: `http://127.0.0.1:${(provider.address() as { port: number }).port}${url.pathname}?page=2` }) }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve)); t.after(() => new Promise<void>(resolve => provider.close(() => resolve())));
  const client = new BitbucketClient({ repository, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' });
  t.after(() => client.close());
  const existing = await client.pullRequests('fix/crewroom/test', 'main'); assert.deepEqual(existing.map(item => item.id), [1, 2]);
  await assert.rejects(client.createPullRequest({ title: 'SIM-1: Fix zero', description: 'Crewroom publication fixture', branch: 'fix/crewroom/test', target: 'main', head }), /unavailable/);
  assert.equal(posts, 1); assert.equal(ledger.length, 3);
});

test('Native DEL-03: an accepted PR create with an unreadable response stays marked as sent', async t => {
  let posts = 0; let body = '{}';
  // First an unreadable 201, then a valid PR whose abbreviated hash cannot be resolved (404).
  const provider = createServer((req, res) => { if (req.method === 'POST') { posts++; res.statusCode = 201; res.end(body); return; } res.statusCode = 404; res.end('{}'); });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve)); t.after(() => new Promise<void>(resolve => provider.close(() => resolve())));
  const client = new BitbucketClient({ repository, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' });
  t.after(() => client.close());
  await assert.rejects(client.createPullRequest({ title: 'SIM-1: Fix zero', description: 'Crewroom publication fixture', branch: 'fix/crewroom/test', target: 'main', head }),
    (error: any) => error.requestSent === true, 'Publication must treat the create as uncertain, never as unsent.');
  body = JSON.stringify({ ...pr(3), source: { ...pr(3).source, commit: { hash: head.slice(0, 12) } } });
  await assert.rejects(client.createPullRequest({ title: 'SIM-1: Fix zero', description: 'Crewroom publication fixture', branch: 'fix/crewroom/test', target: 'main', head }),
    (error: any) => error.requestSent === true && error.status === undefined, "A failed follow-up lookup's status is not a rejection of the create.");
  assert.equal(posts, 2);
});

test('Native SEC-02/DEL-06: provider pagination cannot send credentials outside the configured repository', async t => {
  let requests = 0;
  const provider = createServer((_req, res) => { requests++; res.end(JSON.stringify({ values: [pr(1)], next: `http://127.0.0.1:${(provider.address() as { port: number }).port}/2.0/repositories/another/repository/pullrequests?page=2` })); });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve)); t.after(() => new Promise<void>(resolve => provider.close(() => resolve())));
  const client = new BitbucketClient({ repository, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' });
  t.after(() => client.close());
  await assert.rejects(client.pullRequests('fix/crewroom/test', 'main'), /pagination/i); assert.equal(requests, 1);
});

test('Native EVD-02/DEL-06: hosted observations retain exact revision, pipeline and skipped-step identities', async t => {
  const observed: string[] = [];
  const provider = createServer((req, res) => {
    const url = new URL(req.url!, 'http://fixture'); observed.push(url.pathname);
    if (url.pathname.endsWith('/pullrequests/7')) { res.end(JSON.stringify(pr(7))); return; }
    if (url.pathname.endsWith('/statuses')) {
      assert.ok(url.pathname.includes(`/commit/${head}/`));
      res.end(JSON.stringify({ values: [{ key: 'prs:**:main', state: 'SUCCESSFUL', name: 'PR pipeline', url: 'https://bitbucket.org/fixture/repository/pipelines/results/4', updated_on: '2026-09-28T00:00:00Z' }] })); return;
    }
    if (url.pathname.endsWith('/steps/')) { res.end(JSON.stringify({ values: [{ uuid: '{step}', name: 'Unit Tests', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } } }, { uuid: '{optional}', name: 'Renovate', state: { name: 'COMPLETED', result: { name: 'SKIPPED' } } }] })); return; }
    // Real Bitbucket: /commit/<abbreviation> resolves the full hash; PR pipelines report a 12-character destination commit.
    if (url.pathname.endsWith(`/commit/${base.slice(0, 12)}`)) { res.end(JSON.stringify({ hash: base })); return; }
    assert.equal(url.searchParams.get('target.commit.hash'), head);
    // Real Bitbucket returns no pipelines when filtered by `target.selector.type`; selection happens locally.
    assert.equal(url.searchParams.has('target.selector.type'), false);
    res.end(JSON.stringify({ values: [{ uuid: '{pipeline}', build_number: 4, run_number: 1, created_on: '2026-09-28T00:00:00Z', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } }, target: { type: 'pipeline_pullrequest_target', source: 'fix/crewroom/test', destination: 'main', commit: { hash: head }, destination_commit: { hash: base.slice(0, 12) }, pullrequest: { id: 7 }, selector: { type: 'pull-requests', pattern: '**' } } }] }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve)); t.after(() => new Promise<void>(resolve => provider.close(() => resolve())));
  const client = new BitbucketClient({ repository, apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' });
  t.after(() => client.close());
  const observation = await client.observe(7);
  assert.equal(observation.pr.source.commit.hash, head); assert.equal(observation.pr.destination.commit.hash, base);
  assert.equal(observation.statuses[0].key, 'prs:**:main'); assert.equal(observation.pipelines[0].target.pullrequest?.id, 7);
  assert.equal(observation.pipelines[0].steps[1].state.result?.name, 'SKIPPED'); assert.equal(observation.pipelines[0].target.destination_commit?.hash, base);
  assert.equal(observed.filter(path => path.endsWith('/pullrequests/7')).length, 2, 'Recheck PR revision after fetching its observations.');
});

test('Native DEL-04/06: PR feedback reads all inline, general and reply pages without losing partial progress', async t => {
  const comments = [
    { id: 12, created_on: '2026-09-28T00:00:00Z', updated_on: '2026-09-28T00:00:01Z', deleted: false,
      content: { raw: 'Fix the boundary' }, user: { uuid: '{reviewer}', display_name: 'Reviewer' }, inline: { path: 'src/value.ts', to: 7, from: null, outdated: false } },
    { id: 13, created_on: '2026-09-28T00:01:00Z', updated_on: '2026-09-28T00:01:01Z', deleted: false,
      content: { raw: 'Also document this' }, parent: { id: 12 }, user: { uuid: '{reviewer}', display_name: 'Reviewer' } },
  ];
  let failSecond = false, wrongPr = false;
  const provider = createServer((req, res) => {
    const url = new URL(req.url!, 'http://fixture');
    if (url.pathname.endsWith('/comments')) {
      if (url.searchParams.has('page') && failSecond) { res.statusCode = 503; res.end('{}'); return; }
      const second = url.searchParams.has('page');
      const address = provider.address(); if (!address || typeof address === 'string') { res.statusCode = 500; res.end('{}'); return; }
      const row = second && wrongPr ? { ...comments[1], pullrequest: { id: 8 } } : comments[second ? 1 : 0];
      res.end(JSON.stringify({ values: [row], ...(second ? {} : { next: `http://127.0.0.1:${address.port}${url.pathname}?page=2` }) })); return;
    }
    res.statusCode = 404; res.end('{}');
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve)); t.after(() => new Promise<void>(resolve => provider.close(() => resolve())));
  const address = provider.address(); if (!address || typeof address === 'string') throw new Error('Feedback provider fixture did not bind.');
  const client = new BitbucketClient({ repository, apiUrl: `http://127.0.0.1:${address.port}`, credentials: async () => 'synthetic' });
  t.after(() => client.close());
  const complete = await client.comments(7);
  assert.equal(complete.state, 'complete'); assert.deepEqual(complete.comments.map(item => item.id), [12, 13]);
  assert.deepEqual(complete.comments[0].inline, { path: 'src/value.ts', to: 7, from: null, outdated: false });
  assert.equal(complete.comments[1].parent?.id, 12);
  failSecond = true;
  const partial = await client.comments(7);
  assert.equal(partial.state, 'partial'); assert.deepEqual(partial.comments.map(item => item.id), [12]);
  failSecond = false; wrongPr = true;
  const mismatched = await client.comments(7);
  assert.equal(mismatched.state, 'partial'); assert.deepEqual(mismatched.comments.map(item => item.id), [12]);
});
