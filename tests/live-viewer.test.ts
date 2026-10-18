import assert from 'node:assert/strict';
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import WebSocket from 'ws';
import { LiveHub, checkKey } from '../server/live';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';

// The file watcher, TCP backpressure and LiveHub heartbeat are real OS events;
// poll their observable state, never sleep a fixed period before asserting.
async function until(condition: () => boolean, description: string, timeout = 8000) {
  const deadline = Date.now() + timeout;
  while (!condition()) {
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${description}`);
    await new Promise<void>(resolve => setTimeout(resolve, 10));
  }
}

function fixture(root: string) {
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Live check output', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Watch check', criteria: ['Stream output'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Start' });
  return { store, attempt };
}

async function viewer(url: string, origin: string) {
  const ws = new WebSocket(url, { origin });
  const kinds: string[] = [];
  const resets: string[] = [];
  const chunks: Buffer[] = [];
  let received = 0;
  ws.on('message', raw => {
    const message = JSON.parse(String(raw));
    kinds.push(message.kind);
    if (message.kind === 'reset') resets.push(Buffer.from(message.data, 'base64').toString());
    if (message.kind === 'data') {
      const chunk = Buffer.from(message.data, 'base64');
      chunks.push(chunk);
      received += chunk.length;
    }
  });
  await new Promise<void>((resolve, reject) => { ws.once('open', resolve); ws.once('error', reject); });
  return { ws, kinds, resets, chunks, get received() { return received; } };
}

test('a stalled check viewer resyncs without throttling the producer or a fast viewer', { timeout: 20000 }, async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-live-viewer-'));
  const { store, attempt } = fixture(root);
  const toolRoot = join(root, 'runtime', attempt.id, 'tools', checkKey('slow-check'));
  mkdirSync(toolRoot, { recursive: true });
  const log = join(toolRoot, 'live.log');
  writeFileSync(log, 'Check started\n');
  const server = createServer((_req, res) => { res.end(); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const port = address.port;
  const origin = `http://127.0.0.1:${port}`;
  const hub = new LiveHub(server, { store, workspace: () => ({}), run: () => ({}), result: async () => ({}), runtimeRoot: join(root, 'runtime') }, req => req.headers.origin === origin);
  t.after(async () => {
    hub.close();
    await new Promise<void>(resolve => server.close(() => resolve()));
    store.close();
    rmSync(root, { recursive: true, force: true });
  });
  const topic = `check:${attempt.id}:slow-check`;
  const url = `ws://127.0.0.1:${port}/api/live`;
  const slow = await viewer(url, origin);
  const fast = await viewer(url, origin);
  slow.ws.send(JSON.stringify({ op: 'subscribe', topic }));
  fast.ws.send(JSON.stringify({ op: 'subscribe', topic }));
  await until(() => slow.resets.length === 1 && fast.resets.length === 1, 'both initial snapshots');
  assert.equal(slow.resets[0], 'Check started\n');
  assert.equal(fast.resets[0], 'Check started\n');

  // This metric is observed on the server: a paused client socket's own bufferedAmount
  // cannot tell us whether Crewroom is accumulating unsent frames for that viewer.
  const observableHub = hub as unknown as { clients: Map<WebSocket, Map<string, { lagging?: boolean }>> };
  const clients = observableHub.clients;
  const slowSocket = clients.keys().next().value!;
  const slowSub = clients.get(slowSocket)!.get(topic)!;
  // ws exposes its TCP socket only to tests; pausing it does not pause the producer.
  const pausedClient = slow.ws as WebSocket & { _socket: { pause(): void; resume(): void } };
  pausedClient._socket.pause();

  const chunkSize = 128 * 1024;
  let produced = 0;
  const append = async (index: number) => {
    const chunk = Buffer.alloc(chunkSize, 120);
    chunk.write(`${index.toString().padStart(6, '0')}:`);
    appendFileSync(log, chunk);
    produced += chunk.length;
    await until(() => fast.received >= produced, `fast viewer to receive chunk ${index}`, 3000);
  };
  let nextIndex = 0;
  for (; nextIndex < 256 && !slowSub.lagging; nextIndex++) await append(nextIndex);
  assert.equal(slowSub.lagging, true, 'slow viewer enters lagging on real socket backpressure');
  const queuedAtLag = slowSocket.bufferedAmount;
  assert.ok(queuedAtLag > 2 * 1024 * 1024, `server buffer did not cross high water: ${queuedAtLag} bytes`);
  assert.ok(queuedAtLag <= 4 * 1024 * 1024, `server buffer at cutoff is ${queuedAtLag} bytes`);

  for (let count = 0; count < 32; count++) await append(nextIndex++);
  const marker = 'still producing while lagging\n';
  appendFileSync(log, marker);
  produced += Buffer.byteLength(marker);
  await until(() => fast.received >= produced, 'fast viewer to receive output produced while lagging');
  const queuedAfterMore = slowSocket.bufferedAmount;
  assert.ok(queuedAfterMore <= queuedAtLag, `server buffer grew while lagging: ${queuedAtLag} -> ${queuedAfterMore}`);
  assert.equal(fast.received, produced);
  assert.deepEqual(Buffer.concat(fast.chunks), readFileSync(log).subarray('Check started\n'.length));
  assert.ok(!fast.kinds.includes('lagging'), 'fast viewer never lags');
  assert.equal(readFileSync(log).length, produced + 'Check started\n'.length, 'producer keeps appending while the viewer stalls');

  pausedClient._socket.resume();
  await until(() => slow.kinds.includes('lagging') && slow.resets.length === 2, 'slow viewer lag notice and fresh reset', 6000);
  assert.ok(slow.resets[1].includes('still producing while lagging\n'), 'resync contains output produced while the viewer was lagging');
  assert.equal(Buffer.byteLength(slow.resets[1]), 96 * 1024, 'resync replays only the bounded check tail');
  const deliveredBefore = slow.received;
  await append(nextIndex);
  await until(() => slow.received > deliveredBefore, 'live data after resync');
  assert.equal(slow.chunks.at(-1)?.subarray(0, 7).toString(), `${nextIndex.toString().padStart(6, '0')}:`);
  assert.ok(!fast.kinds.includes('lagging'));
  slow.ws.close();
  fast.ws.close();
});
