import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, existsSync, readFileSync, writeFileSync, mkdirSync, appendFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import WebSocket from 'ws';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { LiveHub, checkKey } from '../server/live';
import { TerminalHub, decodeControlOutput } from '../server/terminals';
import { PaneFormatter } from '../server/runtime/pane-format';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';

const hasTmux = spawnSync('tmux', ['-V']).status === 0;
const wait = async (condition: () => boolean, message: string, timeout = 8000) => {
  const deadline = Date.now() + timeout;
  while (!condition()) { if (Date.now() > deadline) throw new Error(message); await new Promise(resolve => setTimeout(resolve, 25)); }
};

function fixture(root: string) {
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Live view', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Watch agents', criteria: ['Stream output'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Start' });
  return { store, run, attempt };
}

async function serve(hub: (server: ReturnType<typeof createServer>, origin: string) => LiveHub) {
  const server = createServer((_req, res) => { res.end(); });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as { port: number }).port; const origin = `http://127.0.0.1:${port}`;
  const live = hub(server, origin);
  return { server, live, origin, url: `ws://127.0.0.1:${port}/api/live`, async close() { live.close(); await new Promise(resolve => server.close(resolve)); } };
}

function client(url: string, origin: string) {
  const ws = new WebSocket(url, { origin });
  const messages: any[] = [];
  ws.on('message', raw => messages.push(JSON.parse(String(raw))));
  return { ws, messages, open: new Promise<void>((resolve, reject) => { ws.once('open', () => resolve()); ws.once('error', reject); }) };
}

test('control-mode output decoding restores control characters and backslashes', () => {
  assert.deepEqual(decodeControlOutput('a\\033[1mb\\015\\012\\134'), Buffer.from('a\x1b[1mb\r\n\\'));
  assert.deepEqual(decodeControlOutput('✓ ok'), Buffer.from('✓ ok'));
});

test('pane formatting streams response text and names tool activity without JSON', () => {
  let out = ''; const pane = new PaneFormatter(text => { out += text; }, () => new Date('2026-09-28T14:31:02'));
  pane.frame({ type: 'host_ready', hostPid: 10, ompPid: 11 });
  pane.frame({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'Checking the ' } });
  pane.frame({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'dialog.' } });
  pane.frame({ type: 'message_end' });
  pane.frame({ type: 'host_tool_call', toolName: 'write_file', arguments: { path: 'src/dialog.tsx', content: 'a\nb' } });
  pane.output('Running 4 tests\n  ✓ one\n');
  const plain = out.replace(/\x1b\[[0-9;]*m/g, '');
  assert.match(plain, /host ready · host 10 · omp 11/);
  assert.match(plain, /› Checking the dialog\.\n/);
  assert.match(plain, /→ write_file path=src\/dialog\.tsx 2 lines/);
  assert.match(plain, /│ Running 4 tests\n│ {3}✓ one\n/);
  assert.ok(!plain.includes('{"'), 'The pane shows readable activity, not event JSON.');
});

test('the push channel rejects other origins and pushes state only when it changes', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-live-'));
  const { store, run } = fixture(root);
  let views = 0;
  const app = await serve((server, origin) => new LiveHub(server, { store, workspace: () => { views++; return store.snapshot(); }, run: id => store.runtime(id), result: async id => ({ id }) },
    req => req.headers.origin === origin));
  t.after(async () => { await app.close(); store.close(); rmSync(root, { recursive: true, force: true }); });

  const forged = new WebSocket(app.url, { origin: 'http://evil.example' });
  await new Promise<void>(resolve => { forged.once('error', () => resolve()); forged.once('unexpected-response', () => resolve()); });
  assert.notEqual(forged.readyState, WebSocket.OPEN);

  const browser = client(app.url, app.origin); await browser.open;
  browser.ws.send(JSON.stringify({ op: 'subscribe', topic: 'workspace' }));
  browser.ws.send(JSON.stringify({ op: 'subscribe', topic: `run:${run.id}` }));
  await wait(() => browser.messages.filter(item => item.kind === 'state').length === 2, 'initial state');
  const before = browser.messages.length;
  await new Promise(resolve => setTimeout(resolve, 2300)); // One heartbeat with no change.
  assert.equal(browser.messages.length, before, 'Unchanged views are not re-sent.');
  store.steer(run.id, { requestId: 'live-note', body: 'Keep the patch small.' });
  await wait(() => browser.messages.some((item, index) => index >= before && item.topic === 'workspace' && JSON.stringify(item.data).includes('Keep the patch small.')), 'pushed change');
  assert.ok(views >= 2);
  browser.ws.send('not json');
  await wait(() => browser.messages.some(item => item.kind === 'error'), 'invalid request reported');
  browser.ws.close();
});

test('event topics resume from a cursor without repeating delivered events', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-live-events-'));
  const { store, run, attempt } = fixture(root);
  const app = await serve((server, origin) => new LiveHub(server, { store, workspace: () => ({}), run: () => ({}), result: async () => ({}) }, req => req.headers.origin === origin));
  t.after(async () => { await app.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  for (let sequence = 1; sequence <= 250; sequence++) store.recordAgentEvent(attempt.id, sequence, { type: 'tool_started', toolCallId: `call-${sequence}`, commandId: 'unit' });
  const first = client(app.url, app.origin); await first.open;
  first.ws.send(JSON.stringify({ op: 'subscribe', topic: `events:${run.id}` }));
  await wait(() => first.messages.flatMap(item => item.events ?? []).length === 250, 'backlog pages');
  const last = first.messages.at(-1).after; first.ws.close();
  store.recordAgentEvent(attempt.id, 251, { type: 'tool_exited', toolCallId: 'call-250', result: { exitCode: 0 } });
  const second = client(app.url, app.origin); await second.open;
  second.ws.send(JSON.stringify({ op: 'subscribe', topic: `events:${run.id}`, after: last }));
  await wait(() => second.messages.some(item => item.events), 'resumed page');
  const resumed = second.messages.flatMap(item => item.events ?? []);
  assert.deepEqual(resumed.map((item: any) => item.sequence), [251]);
  store.recordAgentEvent(attempt.id, 252, { type: 'process_exit', code: 0 });
  await wait(() => second.messages.flatMap(item => item.events ?? []).length === 2, 'live event after resume');
  second.ws.close();
});

test('a running check streams its live output file to a subscribed tile', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-live-check-'));
  const { store, attempt } = fixture(root);
  const runtimeRoot = join(root, 'runtime');
  const toolRoot = join(runtimeRoot, attempt.id, 'tools', checkKey('call-1')); mkdirSync(toolRoot, { recursive: true });
  writeFileSync(join(toolRoot, 'live.log'), 'Running 4 tests\n');
  const app = await serve((server, origin) => new LiveHub(server, { store, workspace: () => ({}), run: () => ({}), result: async () => ({}), runtimeRoot }, req => req.headers.origin === origin));
  t.after(async () => { await app.close(); store.close(); rmSync(root, { recursive: true, force: true }); });
  const tile = client(app.url, app.origin); await tile.open;
  tile.ws.send(JSON.stringify({ op: 'subscribe', topic: `check:${attempt.id}:call-1` }));
  await wait(() => tile.messages.some(item => item.kind === 'reset'), 'initial check output');
  assert.equal(Buffer.from(tile.messages[0].data, 'base64').toString(), 'Running 4 tests\n');
  appendFileSync(join(toolRoot, 'live.log'), '  ✓ scroll exposes Start run\n');
  await wait(() => tile.messages.some(item => item.kind === 'data'), 'streamed check output');
  assert.match(Buffer.from(tile.messages.find(item => item.kind === 'data').data, 'base64').toString(), /scroll exposes Start run/);
  tile.ws.close();
});

test('a real tmux pane streams read-only to a viewer, survives viewer reconnect, and replays after the server stops', { skip: !hasTmux && 'tmux is not installed' }, async t => {
  const root = mkdtempSync(join(tmpdir(), 'crlive-'));
  const socket = join(root, 't.sock'); const runtimeRoot = join(root, 'runtime');
  const { store, attempt } = fixture(root);
  mkdirSync(join(runtimeRoot, attempt.id), { recursive: true });
  const tmux = (...args: string[]) => execFileSync('tmux', ['-S', socket, '-f', '/dev/null', ...args], { encoding: 'utf8' }).trim();
  const script = join(root, 'agent.sh');
  const firstRelease = join(root, 'first-release');
  const secondRelease = join(root, 'second-release');
  writeFileSync(script, [
    'printf "\\033[32mhost ready\\033[0m\\n";',
    'while [ ! -f ' + JSON.stringify(firstRelease) + ' ]; do sleep 0.02; done;',
    'i=0; while [ $i -lt 41 ]; do echo "tick $i"; i=$((i+1)); sleep 0.02; done;',
    'while [ ! -f ' + JSON.stringify(secondRelease) + ' ]; do sleep 0.02; done;',
    'while [ $i -lt 160 ]; do echo "tick $i"; i=$((i+1)); sleep 0.02; done; read _',
  ].join(' '));
  const pane = tmux('new-session', '-d', '-s', 'crewroom', '-x', '80', '-y', '24', '-P', '-F', '#{pane_id}', 'sh', script);
  const saved = store.getAttempt(attempt.id);
  (store as any).db.prepare('UPDATE attempts SET data=? WHERE id=?').run(JSON.stringify({ ...saved, pane, ownership: 'held', state: 'executing' }), attempt.id);
  const terminals = new TerminalHub(socket, runtimeRoot, id => store.attemptForPane(id));
  const app = await serve((server, origin) => new LiveHub(server, { store, workspace: () => ({}), run: () => ({}), result: async () => ({}), runtimeRoot, terminals }, req => req.headers.origin === origin));
  t.after(async () => { terminals.close(); await app.close(); try { tmux('kill-server'); } catch {} store.close(); rmSync(root, { recursive: true, force: true }); });
  await wait(() => terminals.connected, 'tmux observer connected');

  const viewer = client(app.url, app.origin); await viewer.open;
  viewer.ws.send(JSON.stringify({ op: 'subscribe', topic: `terminal:${attempt.id}` }));
  await wait(() => viewer.messages.some(item => item.kind === 'reset'), 'terminal snapshot');
  const reset = viewer.messages.find(item => item.kind === 'reset');
  assert.deepEqual([reset.cols, reset.rows, reset.source, reset.live], [80, 24, 'pane', true]);
  writeFileSync(firstRelease, '');
  await wait(() => viewer.messages.filter(item => item.kind === 'data').map(item => Buffer.from(item.data, 'base64').toString()).join('').includes('tick 40'), 'live pane output');
  viewer.ws.close();

  // Viewer detach does not affect the pane; a new viewer gets a fresh screen and live output.
  const again = client(app.url, app.origin); await again.open;
  again.ws.send(JSON.stringify({ op: 'subscribe', topic: `terminal:${attempt.id}` }));
  await wait(() => again.messages.some(item => item.kind === 'reset' && Buffer.from(item.data, 'base64').toString().includes('tick')), 'reconnected snapshot');
  const reconnected = again.messages.find(item => item.kind === 'reset');
  assert.deepEqual([reconnected.source, reconnected.live], ['pane', true]);
  writeFileSync(secondRelease, '');
  await wait(() => again.messages.filter(item => item.kind === 'data').map(item => Buffer.from(item.data, 'base64').toString()).join('').includes('tick 159'), 'reconnected live output');
  again.ws.close();

  await wait(() => existsSync(terminals.logPath(attempt.id)) && readFileSync(terminals.logPath(attempt.id), 'utf8').includes('tick 159'), 'bounded replay log');
  tmux('kill-server');
  const replay = client(app.url, app.origin); await replay.open;
  replay.ws.send(JSON.stringify({ op: 'subscribe', topic: `terminal:${attempt.id}` }));
  await wait(() => replay.messages.some(item => item.kind === 'reset'), 'replay after tmux stopped');
  const retained = replay.messages.find(item => item.kind === 'reset');
  assert.equal(retained.source, 'log');
  assert.match(Buffer.from(retained.data, 'base64').toString(), /tick 159/);
  replay.ws.close();
});
