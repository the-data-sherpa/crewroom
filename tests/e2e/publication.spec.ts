import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { publicationFixture, reviewerSummary } from '../fixtures/publication';
import { Publication } from '../../server/publication';
import { startServer } from '../../server/app';

test('PR creation, hosted validation and a skipped required check remain separate in the workbench', async ({ page }) => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-publication-browser-')); const fixture = (await publicationFixture(root));
  const repository = { workspace: 'fixture', slug: 'repository', uuid: '{00000000-0000-4000-8000-000000000011}' };
  const policy = { statusKeys: ['pr-pipeline'], pipelineSteps: ['Unit Tests'], optionalSteps: ['Renovate'] };
  let mode = 'pending', posts = 0; let pr: any;
  const provider = createServer(async (req, res) => {
    const url = new URL(req.url!, 'http://fixture'); res.setHeader('Content-Type', 'application/json');
    if (url.pathname === '/2.0/repositories/fixture/repository') { res.end(JSON.stringify({ uuid: '{00000000-0000-4000-8000-000000000011}', full_name: 'fixture/repository' })); return; }
    if (req.method === 'POST') {
      let text = ''; for await (const chunk of req) text += chunk; const input = JSON.parse(text); posts++;
      pr = { id: 9, title: input.title, description: input.description, state: 'OPEN', updated_on: '2026-09-28T00:00:00Z',
        source: { branch: input.source.branch, commit: { hash: fixture.candidate.revision.head }, repository },
        destination: { branch: input.destination.branch, commit: { hash: fixture.candidate.revision.base }, repository },
        links: { html: { href: 'https://bitbucket.org/fixture/repository/pull-requests/9' } } };
      res.statusCode = 201; res.end(JSON.stringify(pr)); return;
    }
    if (url.pathname.endsWith('/pullrequests')) { res.end(JSON.stringify({ values: pr ? [pr] : [] })); return; }
    if (url.pathname.endsWith('/pullrequests/9')) { res.end(JSON.stringify(pr)); return; }
    if (url.pathname.endsWith('/statuses')) { res.end(JSON.stringify({ values: [{ key: 'pr-pipeline', state: 'SUCCESSFUL', name: 'Pipeline', url: 'https://example.invalid/check', updated_on: '2026-09-28T00:00:00Z' }] })); return; }
    if (url.pathname.endsWith('/steps/')) { res.end(JSON.stringify({ values: [{ uuid: 'unit', name: 'Unit Tests', state: { name: 'COMPLETED', result: { name: mode === 'skipped' ? 'SKIPPED' : 'SUCCESSFUL' } } }, { uuid: 'renovate', name: 'Renovate', state: { name: 'COMPLETED', result: { name: 'SKIPPED' } } }] })); return; }
    res.end(JSON.stringify({ values: mode === 'pending' ? [] : [{ uuid: '{pipeline}', build_number: 1, created_on: '2026-09-28T00:00:00Z', state: { name: 'COMPLETED', result: { name: 'SUCCESSFUL' } }, target: { type: 'pipeline_pullrequest_target', source: fixture.owned.branch, destination: 'main', commit: { hash: fixture.candidate.revision.head }, destination_commit: { hash: fixture.candidate.revision.base }, pullrequest: { id: 9 }, selector: { type: 'pull-requests', pattern: '**' } } }] }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  const connections = [{ repositoryPath: fixture.checkout, repository, policy, fixtureGitPath: fixture.remote,
    apiUrl: `http://127.0.0.1:${(provider.address() as { port: number }).port}`, credentials: async () => 'synthetic' }];
  const service = new Publication(fixture.store, fixture.work, fixture.evidence, join(root, 'publication'), connections);
  (await service.prepare(fixture.coordinator.attempt.id, { candidateId: fixture.candidate.id, summary: reviewerSummary })); await service.close(); fixture.store.close();
  const app = await startServer({ port: 0, dataDir: root, production: true, rolePath: join(root, 'roles.json'), publication: connections });
  try {
    await page.goto(app.origin + '/#' + fixture.run.taskId); await page.getByRole('tab', { name: 'Change & evidence' }).click(); const panel = page.getByRole('region', { name: 'PR delivery', exact: true });
    const reconcile = async () => {
      const [response] = await Promise.all([
        page.waitForResponse(response => response.url().endsWith(`/api/runs/${fixture.run.id}/publication/reconcile`) && response.request().method() === 'POST', { timeout: 30000 }),
        panel.getByRole('button', { name: 'Reconcile publication', exact: true }).click(),
      ]);
      expect(response.ok()).toBe(true);
    };
    await expect(panel.getByText('Push: pending', { exact: true })).toBeVisible();
    await reconcile();
    await expect(panel.getByRole('link', { name: 'Open pull request', exact: true })).toHaveAttribute('href', 'https://bitbucket.org/fixture/repository/pull-requests/9');
    await expect(panel.getByText('Validation pending or blocked', { exact: true })).toBeVisible();
    mode = 'ready'; await reconcile();
    await expect(panel.getByText('Ready for human review', { exact: true })).toBeVisible();
    await expect(panel.getByText('Renovate · SKIPPED', { exact: true })).toBeVisible();
    mode = 'skipped'; await reconcile();
    await expect(panel.getByText('Validation pending or blocked', { exact: true })).toBeVisible();
    await expect(panel.getByText('Unit Tests · SKIPPED', { exact: true })).toBeVisible(); expect(posts).toBe(1);
    await page.reload(); await expect(panel.getByText('Validation pending or blocked', { exact: true })).toBeVisible();
  } finally { await app.close(); await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); }
});
