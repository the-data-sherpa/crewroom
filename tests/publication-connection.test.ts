import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createServer } from 'node:http';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BitbucketClient } from '../server/bitbucket-client';
import { loadPublicationConnections } from '../server/publication-connection';

test('server uses the configured bkt credential scheme without exposing or retrying credentials', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-bitbucket-auth-'));
  const repository = { workspace: 'fixture', slug: 'conductor', uuid: '{00000000-0000-4000-8000-000000000011}' };
  const metadata = join(root, 'bitbucket.json');
  writeFileSync(metadata, JSON.stringify([{ repositoryPath: root, repository, policy: { statusKeys: ['ci'], pipelineSteps: ['Tests'] } }]));
  let expected = ''; let requests = 0; let status = 200;
  const provider = createServer((req, res) => {
    requests++; assert.equal(req.headers.authorization, expected);
    res.writeHead(status); res.end(JSON.stringify({ uuid: repository.uuid, full_name: 'fixture/conductor' }));
  });
  await new Promise<void>(resolve => provider.listen(0, '127.0.0.1', resolve));
  t.after(async () => { await new Promise<void>(resolve => provider.close(() => resolve())); rmSync(root, { recursive: true, force: true }); });
  const apiUrl = `http://127.0.0.1:${(provider.address() as { port: number }).port}`;
  for (const method of ['basic', 'oauth']) {
    let reads = 0;
    const boundary = async (file: string, args: string[]) => {
      if (file === '/usr/bin/security') {
        reads++; return { stdout: method === 'basic' ? 'fixture-api-token\n' : JSON.stringify({ access_token: 'fixture-oauth-token', expires_at: new Date(Date.now() + 3600000).toISOString() }) + '\n' };
      }
      assert.equal(file, '/opt/homebrew/bin/bkt');
      return { stdout: args.includes('status') ? JSON.stringify({ active_context: 'conductor',
        contexts: [{ name: 'conductor', host: 'api.bitbucket.org' }],
        hosts: [{ key: 'api.bitbucket.org', kind: 'cloud', base_url: 'https://api.bitbucket.org/2.0', username: 'fixture@example.test', auth_method: method, token_source: 'keyring' }] }) : '{}' };
    };
    const connection = loadPublicationConnections(metadata, boundary)[0];
    const client = new BitbucketClient({ ...connection, apiUrl });
    expected = method === 'basic' ? `Basic ${Buffer.from('fixture@example.test:fixture-api-token').toString('base64')}` : 'Bearer fixture-oauth-token';
    assert.equal((await client.verifyRepository()).uuid, repository.uuid);
    await client.verifyRepository(); assert.equal(reads, 1, 'A valid credential stays in server memory.');
    status = 503; const before = requests;
    await assert.rejects(client.verifyRepository(), error => error instanceof Error && !error.message.includes('fixture-api-token') && !error.message.includes('fixture-oauth-token'));
    assert.equal(requests, before + 1, 'The direct adapter does not add transport retries.'); status = 200;
  }
});

test('unsupported accounts and malformed Keychain credentials fail closed and do not repeatedly prompt', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-bitbucket-invalid-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const path = join(root, 'bitbucket.json');
  const repository = { workspace: 'fixture', slug: 'conductor', uuid: '{00000000-0000-4000-8000-000000000011}' };
  writeFileSync(path, JSON.stringify([{ repositoryPath: root, repository, policy: { statusKeys: ['ci'], pipelineSteps: ['Tests'] } }]));
  for (const fault of ['wrong-host', 'unknown-scheme', 'missing-user', 'multiline-token', 'expired-oauth']) {
    let calls = 0;
    const boundary = async (file: string, args: string[]) => {
      calls++;
      if (file === '/usr/bin/security') return { stdout: fault === 'expired-oauth' ? JSON.stringify({ access_token: 'private-expired-token', expires_at: '2020-01-01T00:00:00Z' }) : 'private-token\ninjected-header\n' };
      return { stdout: args.includes('status') ? JSON.stringify({ active_context: 'conductor', contexts: [{ name: 'conductor', host: 'api.bitbucket.org' }],
        hosts: [{ key: 'api.bitbucket.org', kind: 'cloud', base_url: fault === 'wrong-host' ? 'https://other.example/2.0' : 'https://api.bitbucket.org/2.0',
          username: fault === 'missing-user' ? '' : 'fixture@example.test', auth_method: fault === 'unknown-scheme' ? 'custom' : fault === 'expired-oauth' ? 'oauth' : 'basic', token_source: 'keyring' }] }) : '{}' };
    };
    const connection = loadPublicationConnections(path, boundary)[0];
    await assert.rejects(connection.credentials(), error => error instanceof Error && !error.message.includes('private-'));
    const before = calls;
    await assert.rejects(connection.credentials(), /credentials are unavailable/);
    assert.equal(calls, before, 'A failed Keychain read requires an explicit new loader/server, not repeated prompts.');
  }
});
