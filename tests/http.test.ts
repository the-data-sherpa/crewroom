import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { request } from 'node:http';
import { test } from 'node:test';
import { startServer } from '../server/app';

test('UI-01/INT-04: the loopback API persists a ticket and exposes three separate status facts', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-http-'));
  const server = await startServer({ port: 0, dataDir: root, serveUi: false });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const response = await fetch(`${server.origin}/api/tasks`, { method: 'POST',
    headers: { Origin: server.origin, 'Content-Type': 'application/json' },
    body: JSON.stringify({ title: 'Check zero', description: 'A local request' }) });
  assert.equal(response.status, 201);
  const created = await response.json();
  const snapshot = await (await fetch(`${server.origin}/api/workspace`)).json();
  assert.equal(snapshot.tasks[0].id, created.id);
  assert.equal(snapshot.tasks[0].jiraStatus, 'Not linked');
  assert.equal(snapshot.runs.length, 0);
  assert.equal(snapshot.connections.runner, false);
  assert.equal(snapshot.connections.jira, false);
  assert.deepEqual(snapshot.activity, []);
});

test('SEC-01: foreign, absent and forged origins cannot mutate the workbench', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-origin-'));
  const server = await startServer({ port: 0, dataDir: root, serveUi: false });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const attempts: Record<string, string>[] = [
    {}, { Origin: 'https://foreign.example' }, { Origin: 'null' },
    { Origin: server.origin, 'Sec-Fetch-Site': 'cross-site' },
    { Origin: 'http://127.0.0.1:1', 'X-Forwarded-Host': server.origin.slice(7) },
  ];
  for (const headers of attempts) {
    const response = await fetch(`${server.origin}/api/tasks`, { method: 'POST',
      headers: { 'Content-Type': 'application/json', ...headers },
      body: JSON.stringify({ title: 'Unwanted ticket', description: '' }) });
    assert.equal(response.status, 403);
  }
  assert.equal((await (await fetch(`${server.origin}/api/workspace`)).json()).tasks.length, 0);
  const response = await fetch(`${server.origin}/api/workspace`, { headers: { Origin: 'https://foreign.example' } });
  assert.equal(response.status, 403);
  assert.equal(response.headers.get('access-control-allow-origin'), null);
  const forgedHost = await new Promise<number | undefined>((resolve, reject) => {
    const req = request(`${server.origin}/api/workspace`, { headers: { Host: 'attacker.example', 'X-Forwarded-Host': server.origin.slice(7) } }, res => {
      res.resume(); resolve(res.statusCode);
    });
    req.once('error', reject); req.end();
  });
  assert.equal(forgedHost, 403);
});

test('LIF-05: a second server cannot become controller of the same workbench', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-controller-'));
  const first = await startServer({ port: 0, dataDir: root, serveUi: false });
  t.after(async () => { await first.close(); rmSync(root, { recursive: true, force: true }); });
  let second: Awaited<ReturnType<typeof startServer>> | undefined;
  try {
    await assert.rejects(async () => { second = await startServer({ port: 0, dataDir: root, serveUi: false }); }, /controller/i);
  } finally { await second?.close(); }
  assert.equal((await fetch(first.origin + '/api/workspace')).status, 200);
});
