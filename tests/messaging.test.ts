import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { test } from 'node:test';
import { RustMailClient } from '../server/runtime/messaging';

// Rust Mail 0.3.36 reports SQLite contention between concurrent runs as this tool error.
const busy = JSON.stringify({ error: { type: 'RESOURCE_BUSY', recoverable: true, data: { db_error_classification: { class: 'busy_retryable', safe_to_retry: true } } } });
const unknownAgent = JSON.stringify({ error: { type: 'NOT_FOUND', recoverable: false } });

async function mailFixture(replies: string[]) {
  const calls: string[] = [];
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    const request = JSON.parse(raw);
    res.setHeader('Content-Type', 'application/json');
    if (request.method !== 'tools/call') { res.end(request.id === undefined ? '' : JSON.stringify({ jsonrpc: '2.0', id: request.id, result: {} })); return; }
    calls.push(request.params.name);
    const reply = replies.shift();
    res.end(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: reply === undefined ? { structuredContent: { events: [] } } : { isError: true, content: [{ type: 'text', text: reply }] } }));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  return { calls, client: new RustMailClient(`http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp/`, 'fixture'), close: () => new Promise<void>(resolve => server.close(() => resolve())) };
}

test('R7: a retry-safe busy mailbox shared by concurrent runs is retried within a bound; other rejections are not repeated', async t => {
  const recovered = await mailFixture([busy, busy]); t.after(recovered.close);
  assert.deepEqual(await recovered.client.call('fetch_inbox_events', {}), { events: [] });
  assert.equal(recovered.calls.length, 3);

  const exhausted = await mailFixture([busy, busy, busy, busy, busy]); t.after(exhausted.close);
  await assert.rejects(exhausted.client.call('fetch_inbox_events', {}), /RESOURCE_BUSY/);
  assert.equal(exhausted.calls.length, 5);

  const rejected = await mailFixture([unknownAgent]); t.after(rejected.close);
  await assert.rejects(rejected.client.call('send_message', {}), /NOT_FOUND/);
  assert.deepEqual(rejected.calls, ['send_message']);
});

test('MSG-09: HTTP throttling uses bounded backoff, honors Retry-After, and never blindly repeats uncertain writes', async t => {
  const delays: number[] = []; const counts: Record<string, number> = {};
  let statuses: number[] = [];
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    const request = JSON.parse(raw);
    res.setHeader('Content-Type', 'application/json');
    if (request.method !== 'tools/call') { res.end(request.id ? JSON.stringify({ id: request.id, result: {} }) : ''); return; }
    counts[request.params.name] = (counts[request.params.name] ?? 0) + 1;
    const status = statuses.shift() ?? 200;
    if (status !== 200) { res.writeHead(status, { 'Retry-After': status === 429 ? '1' : '0' }); res.end('{}'); return; }
    res.end(JSON.stringify({ id: request.id, result: { structuredContent: { ok: true } } }));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise<void>(resolve => server.close(() => resolve())));
  const client = new RustMailClient(`http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp/`, 'fixture', async ms => { delays.push(ms); });
  statuses = [429, 429];
  await client.call('send_message', { idempotency_key: 'durable-key' });
  assert.equal(counts.send_message, 3); assert.deepEqual(delays, [1000, 1000]);
  statuses = [503];
  await assert.rejects(client.call('send_message', { idempotency_key: 'pending-key' }), /503/);
  assert.equal(counts.send_message, 4, 'An unavailable service may have accepted a write; retain the pending intent.');
  statuses = [503, 503]; delays.length = 0;
  await client.call('fetch_inbox_events', {});
  assert.deepEqual(delays, [100, 200]); assert.equal(counts.fetch_inbox_events, 3);
  statuses = [429, 429, 429, 429, 429]; delays.length = 0;
  await assert.rejects(client.call('fetch_inbox_events', {}), /429/);
  assert.equal(counts.fetch_inbox_events, 8); assert.deepEqual(delays, [1000, 1000, 1000, 1000]);
});

test('MSG-07: a stalled read consumes one overall timeout budget instead of multiplying the lifecycle wait', async t => {
  let calls = 0;
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    const request = JSON.parse(raw); res.setHeader('Content-Type', 'application/json');
    if (request.method !== 'tools/call') { res.end(request.id ? JSON.stringify({ id: request.id, result: {} }) : ''); return; }
    calls++; // Deliberately leave the passive read unanswered.
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); return new Promise<void>(resolve => server.close(() => resolve())); });
  const client = new RustMailClient(`http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp/`, 'fixture', async () => { assert.fail('An expired timeout must not back off and retry.'); }, 100);
  const before = Date.now();
  await assert.rejects(client.call('fetch_inbox_events', {}), error => error instanceof DOMException && error.name === 'TimeoutError');
  assert.equal(calls, 1); assert.ok(Date.now() - before < 1000);
});

test('MSG-07: initialization and its notification share the same overall deadline', async t => {
  const calls: string[] = [];
  const timers: ReturnType<typeof setTimeout>[] = [];
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    const request = JSON.parse(raw); calls.push(request.method);
    res.setHeader('Content-Type', 'application/json');
    timers.push(setTimeout(() => res.end(request.id ? JSON.stringify({ id: request.id, result: {} }) : ''), 150));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { for (const timer of timers) clearTimeout(timer); server.closeAllConnections(); return new Promise<void>(resolve => server.close(() => resolve())); });
  const client = new RustMailClient(`http://127.0.0.1:${(server.address() as AddressInfo).port}/mcp/`, 'fixture', async () => { assert.fail('Initialization must not retry.'); }, 250);
  await assert.rejects(client.call('fetch_inbox_events', {}), error => error instanceof DOMException && error.name === 'TimeoutError');
  assert.deepEqual(calls, ['initialize', 'notifications/initialized']);
});
