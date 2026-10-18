import assert from 'node:assert/strict';
import { createServer, type Server } from 'node:http';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { RuntimeSupervisor, type RuntimeConfig } from '../server/runtime/supervisor';
import type { Assignment, Attempt } from '../server/runtime/types';
import { inspectOwned, launchOwned, stopOwned } from '../server/runtime/processes';
import { TerminalHub, followFile } from '../server/terminals';

type Scripted = { attemptId: string; runId: string; pane: string; token: string; emit: (type: string, extra?: object) => void; requests: () => number; delay: (active: boolean) => void; stop: () => Promise<void> };
type Fixture = { root: string; store: Workbench; supervisor: RuntimeSupervisor; addRun: (names: string[], pane?: string, slowFirst?: boolean) => Promise<{ runId: string; hosts: Scripted[] }>; close: () => Promise<void> };

// Integration clock is deliberate: HTTP AbortSignal timeouts and real process leases
// cannot be advanced by fake timers. Poll the observable condition, not a fixed sleep.
async function until(check: () => boolean, label: string, timeout = 12000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert.fail(`Timed out waiting for ${label}`);
}

async function fixture(withMail = false): Promise<Fixture> {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-hosts-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'db'));
  const config = { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused', hostPorts: [4001, 4002, 4003, 4004],
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true }, catalog: [], policy: {}, models: { providers: {} } };
  const processes: string[] = []; const servers: Server[] = []; const children: ReturnType<typeof launchOwned>[] = [];
  if (withMail) {
    const mail = createServer(async (req, res) => {
      let raw = ''; for await (const chunk of req) raw += chunk;
      const request = JSON.parse(raw);
      res.setHeader('Content-Type', 'application/json');
      if (request.method === 'notifications/initialized') { res.end(); return; }
      const tool = request.params?.name;
      const result = tool === 'register_agent' ? { name: request.params.arguments.name }
        : tool === 'fetch_inbox_events' ? { events: [], next_cursor: 0, has_more: false } : {};
      res.end(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: tool ? { structuredContent: result } : result }));
    });
    servers.push(mail);
    await new Promise<void>(resolve => mail.listen(0, '127.0.0.1', resolve));
    const address = mail.address(); if (!address || typeof address === 'string') throw new Error('Missing mail fixture port.');
    config.mail.url = `http://127.0.0.1:${address.port}/mcp/`;
  }
  const supervisor = new RuntimeSupervisor(store, config, 'http://127.0.0.1:4000');
  let number = 0;
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  return { root, store, supervisor, async addRun(names, pane, slowFirst = false) {
    number++;
    const task = store.createTask({ title: 'Observe agents', description: '', repositoryId: repo.id });
    store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Keep independent processes visible', criteria: ['No false recovery'] });
    const run = store.queue(task.id, { revision: 2 });
    const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
    const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
    const hosts: Scripted[] = [];
    const admitted = names.map((name, index) => store.admitAssignment(run.id, { name, role, model, coordinator: index === 0, input: 'Stay active.' }));
    for (const [index, { attempt }] of admitted.entries()) {
      const directory = join(config.root, attempt.id); processes.push(directory);
      children.push(launchOwned(join(directory, 'host-process'), '/bin/sleep', ['60'], { cwd: root, env: { PATH: process.env.PATH } }));
      children.push(launchOwned(join(directory, 'omp-process'), '/bin/sleep', ['60'], { cwd: root, env: { PATH: process.env.PATH } }));
      let hostPid: number | undefined; let ompPid: number | undefined;
      const deadline = Date.now() + 5000;
      while ((!hostPid || !ompPid) && Date.now() < deadline) {
        hostPid = (await inspectOwned(join(directory, 'host-process'))).owner?.pid;
        ompPid = (await inspectOwned(join(directory, 'omp-process'))).owner?.pid;
        if (!hostPid || !ompPid) await new Promise(resolve => setTimeout(resolve, 20));
      }
      assert.ok(hostPid && ompPid, 'Both independent process leases became live.');
      let slow = slowFirst && index === 0; let requests = 0; const events: { sequence: number; event: object }[] = [];
      const listeners = new Set<() => void>();
      const server = createServer(async (req, res) => {
        let raw = ''; for await (const chunk of req) raw += chunk;
        const body = JSON.parse(raw);
        res.setHeader('Content-Type', 'application/json');
        if (body.op === 'identity') { res.end(JSON.stringify({ hostPid, ompPid, sessionId: 'fixture-session' })); return; }
        if (body.op === 'close') { for (const listener of listeners) listener(); res.end(JSON.stringify({ events: [] })); return; }
        if (body.op === 'state') { res.end(JSON.stringify({ isStreaming: false, isCompacting: false, hasPendingAsyncWork: false })); return; }
        if (body.op === 'events') {
          requests++;
          const respond = () => { if (!res.destroyed) res.end(JSON.stringify(events.filter(item => item.sequence > body.after).slice(0, 128))); };
          if (slow) { setTimeout(respond, 8200); return; }
          if (events.some(item => item.sequence > body.after)) { respond(); return; }
          const timeout = setTimeout(() => { listeners.delete(wake); respond(); }, Math.min(body.wait ?? 0, 25000));
          const wake = () => { clearTimeout(timeout); listeners.delete(wake); respond(); };
          listeners.add(wake);
          return;
        }
        res.end('{}');
      });
      servers.push(server);
      await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
      const address = server.address(); if (!address || typeof address === 'string') throw new Error('Missing scripted host port.');
      store.claimAttempt(attempt.id);
      const targetPane = pane && index === 0 ? pane : `%${index + 100 + number * 4}`;
      store.recordAttemptIdentity(attempt.id, { hostPid, ompPid, sessionId: 'fixture-session', hostUrl: `http://127.0.0.1:${address.port}/`, pane: targetPane });
      store.recordAgentEvent(attempt.id, 1, { type: 'agent_start' });
      const host = { attemptId: attempt.id, runId: run.id, token: `token-${attempt.id}`, url: `http://127.0.0.1:${address.port}/`, pane: targetPane, cursor: 1 };
      const observerSeam = supervisor as unknown as { register: (item: { attemptId: string; runId: string; token: string; url: string; pane: string; cursor: number }) => void };
      observerSeam.register(host);
      hosts.push({ attemptId: attempt.id, runId: run.id, pane: targetPane, token: host.token,
        emit(type, extra = {}) { events.push({ sequence: events.length + 2, event: { type, ...extra } }); for (const listener of listeners) listener(); },
        requests: () => requests, delay: active => { slow = active; },
        stop: async () => { await stopOwned(join(directory, 'host-process')); await stopOwned(join(directory, 'omp-process')); } });
    }
    store.setDesiredState(run.id, 'running'); store.startExecution(run.id);
    return { runId: run.id, hosts };
  }, async close() {
    const privateSupervisor = supervisor as unknown as { hosts: Map<string, unknown>; stopObserver: (host: unknown) => Promise<void>; timer?: NodeJS.Timeout };
    for (const host of privateSupervisor.hosts.values()) await privateSupervisor.stopObserver(host);
    clearInterval(privateSupervisor.timer);
    for (const directory of processes) { await stopOwned(join(directory, 'host-process')); await stopOwned(join(directory, 'omp-process')); }
    for (const child of children) child.stdin.end();
    for (const server of servers) { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); }
    store.close(); rmSync(root, { recursive: true, force: true });
  } };
}

test('four independent hosts record three healthy streams while a fourth times out, without recovery', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator', 'Alpha', 'Beta', 'Gamma'], undefined, true);
  const start = Date.now();
  for (const host of hosts.slice(1)) host.emit('agent_text', { text: 'working' });
  await until(() => hosts.slice(1).every(host => f.store.agentEvents(runId).some(item => item.attemptId === host.attemptId && item.event.type === 'agent_text')), 'three prompt streams', 1000);
  assert.ok(Date.now() - start < 1000, 'Healthy host events are independent of the timed-out host.');
  await until(() => f.supervisor.isDegraded(hosts[0].attemptId), 'slow host degraded');
  assert.equal(f.store.getRun(runId).status, 'running');
  assert.ok(f.store.runtime(runId).assignments.every(assignment => (assignment.automaticCycles ?? 0) === 0));
  hosts[0].delay(false);
  await until(() => !f.supervisor.isDegraded(hosts[0].attemptId), 'slow host recovered');
  assert.equal(f.store.getRun(runId).status, 'running');
});

test('idle hosts long-poll rather than spin and wake promptly for new events', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator', 'Specialist']);
  await new Promise(resolve => setTimeout(resolve, 2100)); // Deliberate real-time observation-window contract.
  assert.ok(hosts.every(host => host.requests() <= 2), `Idle request counts: ${hosts.map(host => host.requests())}`);
  const start = Date.now(); hosts[1].emit('agent_text', { text: 'now' });
  await until(() => f.store.agentEvents(runId).some(item => item.attemptId === hosts[1].attemptId && item.event.type === 'agent_text'), 'new event', 1000);
  assert.ok(Date.now() - start < 1000);
  assert.equal(f.store.runtime(runId).assignments[1].automaticCycles ?? 0, 0);
});

test('long-running container check remains executing across host timeout and long-poll cycles', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator']);
  hosts[0].emit('tool_started', { toolCallId: 'check-1', commandId: 'check' });
  await until(() => f.store.agentEvents(runId).some(item => item.event.type === 'tool_started'), 'check start');
  const output = join(f.root, 'running-check.log'); writeFileSync(output, '');
  let observed = '';
  const follower = followFile(output, chunk => { observed += chunk.toString('utf8'); });
  t.after(() => follower.stop());
  const started = Date.now();
  await new Promise(resolve => setTimeout(resolve, 3500)); // Exercise the real 3s ordinary request boundary.
  writeFileSync(output, 'check still running\n');
  await until(() => observed.includes('check still running'), 'live check output');
  await new Promise(resolve => setTimeout(resolve, Math.max(0, 6200 - (Date.now() - started)))); // Cross a 5s long-poll cycle.
  assert.equal(f.store.getRun(runId).status, 'running');
  assert.equal(f.store.getAttempt(hosts[0].attemptId).state, 'executing');
  assert.equal(f.store.runtime(runId).assignments[0].automaticCycles ?? 0, 0);
});

test('safety accounting pauses an exhausted run without recovery cycles', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator']);
  f.store.setActiveLimit(runId, { activeLimitMs: 1000 });
  await until(() => f.store.getRun(runId).status === 'paused', 'budget pause confirmed', 6000);
  assert.equal(f.store.getRun(runId).desiredState, 'paused');
  assert.equal(f.store.getAttempt(hosts[0].attemptId).ownership, 'stopped');
  assert.equal(f.store.runtime(runId).assignments[0].automaticCycles ?? 0, 0);
});

test('operator pause confirms stopped leases without charging recovery cycles', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator', 'Specialist']);
  const [first, second] = await Promise.all([f.supervisor.pause(runId), f.supervisor.pause(runId)]);
  assert.equal(first.status, 'paused'); assert.equal(second.status, 'paused');
  assert.ok(hosts.every(host => f.store.getAttempt(host.attemptId).ownership === 'stopped'));
  assert.ok(f.store.runtime(runId).assignments.every(assignment => (assignment.automaticCycles ?? 0) === 0));
});

test('manual resume after confirmed pause does not consume an automatic cycle when runner is unavailable', async t => {
  const f = await fixture(true); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator']);
  assert.equal((await f.supervisor.pause(runId)).status, 'paused');
  assert.equal(f.store.getAttempt(hosts[0].attemptId).ownership, 'stopped');
  await assert.rejects(async () => { await f.supervisor.resume(runId); }, /provider|configuration|model/i);
  assert.equal(f.store.runtime(runId).assignments[0].automaticCycles ?? 0, 0);
  assert.equal(f.store.getRun(runId).status, 'attention_required');
});

test('confirmed specialist exit charges one automatic cycle only to its assignment', async t => {
  const f = await fixture(true); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator', 'Specialist']);
  await hosts[1].stop();
  f.supervisor.paneExited(hosts[1].pane);
  await until(() => f.store.getRun(runId).status === 'attention_required', 'targeted recovery after stopped owner');
  const assignments = f.store.runtime(runId).assignments;
  assert.equal(assignments[0].automaticCycles ?? 0, 0);
  assert.equal(assignments[1].automaticCycles, 1);
  assert.equal(f.store.getAttempt(hosts[1].attemptId).ownership, 'stopped');
});

test('pause, recovery signal and reconciliation serialize on one run in FIFO order', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator']);
  const trace: string[] = [];
  const begin = f.store.beginPause.bind(f.store), finish = f.store.finishPause.bind(f.store), account = f.store.accountTime.bind(f.store);
  f.store.beginPause = (id, reason) => { trace.push('begin'); return begin(id, reason); };
  f.store.finishPause = id => { trace.push('finish'); return finish(id); };
  f.store.accountTime = id => { trace.push('account'); return account(id); };
  const privateSupervisor = f.supervisor as unknown as { publication?: { stopRun: (runId: string) => Promise<void> } };
  privateSupervisor.publication = { stopRun: async () => { await new Promise(resolve => setTimeout(resolve, 150)); } };
  const first = f.supervisor.pause(runId);
  hosts[0].emit('process_exit'); f.supervisor.paneExited(hosts[0].pane); f.supervisor.notify(runId);
  const second = f.supervisor.pause(runId);
  await Promise.all([first, second]);
  await until(() => trace.includes('account'), 'queued reconciliation after pause');
  assert.deepEqual(trace.slice(0, 3), ['begin', 'finish', 'account']);
  assert.equal(trace.filter(item => item === 'begin').length, 1);
  assert.equal(f.store.runtime(runId).assignments[0].automaticCycles ?? 0, 0);
});

test('a slow transition in one run never delays event observation in another run', async t => {
  const f = await fixture(); t.after(f.close);
  const y = await f.addRun(['Coordinator']); const x = await f.addRun([]);
  const seam = f.supervisor as unknown as { publication?: { stopRun: (runId: string) => Promise<void> } };
  const original = seam.publication;
  seam.publication = { stopRun: async (runId: string) => { if (runId === x.runId) await new Promise(resolve => setTimeout(resolve, 1800)); } };
  const pause = f.supervisor.pause(x.runId);
  const start = Date.now(); y.hosts[0].emit('agent_text', { text: 'unblocked' });
  await until(() => f.store.agentEvents(y.runId).some(item => item.event.type === 'agent_text'), 'independent run observation', 1000);
  assert.ok(Date.now() - start < 1000);
  await pause;
  seam.publication = original;
  assert.equal(f.store.getRun(y.runId).status, 'running');
});

test('R7: one host authorization binds each token to its own current assignment and run', async t => {
  const f = await fixture(); t.after(f.close);
  const a = await f.addRun(['Coordinator', 'Specialist']); const b = await f.addRun(['Coordinator']);
  const roles = (host: Scripted, attemptId = host.attemptId) => f.supervisor.coordinatorOperation(`Bearer ${host.token}`, { attemptId, toolCallId: 'roles', arguments: {} }, 'roles');
  assert.deepEqual(roles(a.hosts[0]), []);
  assert.deepEqual(roles(b.hosts[0]), [], 'Two runs hold agents under one supervisor.');
  assert.throws(() => roles(a.hosts[1]), /Only the active coordinator/);
  assert.throws(() => roles(a.hosts[0], b.hosts[0].attemptId), /Only the active coordinator/, 'A token authorizes only its own attempt.');
  assert.throws(() => f.supervisor.steeringReceipt('Bearer forged', { attemptId: a.hosts[0].attemptId, toolCallId: 'x', arguments: {} }), /not authorized/);
  await assert.rejects(f.supervisor.message(`Bearer ${a.hosts[1].token}`, { attemptId: b.hosts[0].attemptId, toolCallId: 'x', arguments: { to: 'Coordinator', body: 'cross-run' } }), /not authorized/);
  assert.equal((await f.supervisor.pause(a.runId)).status, 'paused');
  assert.throws(() => roles(a.hosts[0]), /Only the active coordinator/, 'A stopped host loses authorization.');
  assert.deepEqual(roles(b.hosts[0]), []);
  assert.equal(f.store.getRun(b.runId).status, 'running');
});

test('R6: a coordinator proposal snapshots validated briefs and replays idempotently', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator', 'Implementer']);
  // Test seam: the shared fixture constructs the supervisor without a role or model catalog.
  const seam = f.supervisor as unknown as { roleCatalog: () => unknown[]; config: { catalog: unknown[] } };
  seam.roleCatalog = () => [{ ...defaultRoles[1], model: 'fixture/model', effort: 'low' }];
  seam.config.catalog = [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }];
  const propose = (toolCallId: string, specialist: object) => f.supervisor.coordinatorOperation(`Bearer ${hosts[0].token}`, { attemptId: hosts[0].attemptId, toolCallId, arguments: { specialists: [specialist] } }, 'propose');
  const reviewer = { name: 'Reviewer', roleId: defaultRoles[1].id, input: 'Review the candidate.', criteria: [{ index: 0, obligation: 'Confirm no false recovery.' }], dependsOn: ['Implementer'] };
  assert.throws(() => propose('bad-index', { ...reviewer, criteria: [{ index: 3, obligation: 'Out of range.' }] }), /criterion 3/);
  assert.throws(() => propose('bad-dependency', { ...reviewer, dependsOn: ['Ghost'] }), /dependency Ghost/);
  const first = propose('good', reviewer);
  const [saved] = f.store.teamProposals(runId);
  assert.deepEqual(saved.specialists[0].brief, { criteria: [{ index: 0, criterion: 'No false recovery', obligation: 'Confirm no false recovery.', checkIds: [] }], dependsOn: ['Implementer'] });
  const replay = propose('good', reviewer);
  assert.ok(!Array.isArray(first) && !Array.isArray(replay) && replay.id === first.id && replay.id === saved.id, 'A replayed tool call returns the saved proposal.');
  assert.throws(() => propose('good', { ...reviewer, dependsOn: [] }), /different content/);
  assert.equal(f.store.teamProposals(runId).length, 1);
  // A proposal saved before structured briefs has no brief; replaying its tool call still returns it.
  const legacySeam = f.store as unknown as { db: { prepare: (sql: string) => { run: (...values: string[]) => void } } };
  legacySeam.db.prepare('UPDATE team_proposals SET data=? WHERE id=?').run(JSON.stringify({ ...saved, specialists: saved.specialists.map(({ brief: _, ...rest }) => rest) }), saved.id);
  const legacyReplay = propose('good', { ...reviewer, criteria: [], dependsOn: [] });
  assert.ok(!Array.isArray(legacyReplay) && legacyReplay.id === saved.id);
});

test('ROL-04: disabled pending specialist is rejected before the supervisor stops a running team', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator']);
  const role = { ...defaultRoles.find(item => item.id === 'tech-writer')!, model: 'fixture/model', effort: 'low' as const };
  let currentRoles = [role];
  const seam = f.supervisor as unknown as { roleCatalog: () => typeof currentRoles; config: { catalog: unknown[] }; applyPendingTeam: (runId: string) => Promise<void> };
  seam.roleCatalog = () => currentRoles;
  seam.config.catalog = [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }];
  const propose = (toolCallId: string) => f.supervisor.coordinatorOperation(`Bearer ${hosts[0].token}`, {
    attemptId: hosts[0].attemptId, toolCallId, arguments: { specialists: [{ name: 'Writer', roleId: role.id, input: 'Write.', criteria: [{ index: 0, obligation: 'Write and verify.' }], dependsOn: [] }] },
  }, 'propose');
  const first = propose('before-disable');
  assert.ok(!Array.isArray(first));
  assert.equal(first.status, 'pending');
  currentRoles = [{ ...role, enabled: false }];
  await seam.applyPendingTeam(runId);
  assert.equal(f.store.teamProposals(runId)[0].status, 'rejected');
  assert.equal(f.store.getRun(runId).status, 'running');
  assert.equal(f.store.runtime(runId).assignments.length, 1);
  assert.equal(f.store.getAttempt(hosts[0].attemptId).ownership, 'held', 'The coordinator process was not stopped.');
  assert.equal(f.store.deliveryNotices(runId)[0].payload.subjectId, first.id);
  currentRoles = [role];
  const next = propose('after-rejection');
  assert.ok(!Array.isArray(next)); assert.equal(next.status, 'pending');
});

test('ROL-04: interrupted disabled team transitions reconcile old owners and resume original assignments', async t => {
  for (const oldProcessesLive of [true, false]) await t.test(oldProcessesLive ? 'live old processes' : 'old processes already stopped', async () => {
    const f = await fixture(true);
    type Seam = { hosts: Map<string, unknown>; messages: Map<string, unknown>; config: RuntimeConfig;
      stopObserver: (host: unknown) => Promise<void>; stopRun: (id: string) => Promise<{ status: string }>;
      launchAssignments: (id: string, admitted: { assignment: Assignment; attempt: Attempt }[]) => Promise<void>;
      register: (host: { attemptId: string; runId: string; token: string; url: string; pane: string; cursor: number }) => void;
      notify: (id: string) => void; reconcile: (id: string) => Promise<void>; timer?: NodeJS.Timeout };
    const old = f.supervisor as unknown as Seam;
    const replacements: { root: string; children: ChildProcessWithoutNullStreams[]; server: Server }[] = [];
    const freshes: Seam[] = [];
    let reopened: Workbench | undefined;
    try {
      const { runId, hosts } = await f.addRun(['Coordinator', 'Existing']);
      for (const host of old.hosts.values()) await old.stopObserver(host);
      old.hosts.clear(); // The controller restarted; saved OS process leases did not disappear.
      const role = { ...defaultRoles.find(item => item.id === 'tech-writer')!, model: 'fixture/model', effort: 'low' as const };
      let currentRoles = [{ ...role, enabled: false }];
      const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
      const proposal = f.store.proposeTeam(hosts[0].attemptId, 'interrupted-team', [{ name: 'NewWriter', role, model, input: 'Write.' }], [{ name: 'Existing', reason: 'Rotate specialist.' }]);
      f.store.beginTeamTransition(proposal.id);
      if (!oldProcessesLive) assert.equal((await old.stopRun(runId)).status, 'paused');
      else assert.equal((await inspectOwned(join(old.config.root, hosts[0].attemptId, 'host-process'))).state, 'live');
      reopened = new Workbench(join(f.root, 'db'));
      const store = reopened;
      const captured = store.runtime(runId).assignments.map(item => ({ id: item.id, role: item.role, model: item.model }));
      const inputs: { id: string; message: string }[] = [];
      const makeFresh = () => {
        const fresh = new RuntimeSupervisor(store, old.config, 'http://127.0.0.1:4000', () => currentRoles) as unknown as Seam;
        freshes.push(fresh);
        fresh.notify = () => {}; // Explicit reconcile below controls only notice delivery timing.
        fresh.launchAssignments = async (id, admitted) => {
          for (const { attempt } of admitted) {
            const root = join(old.config.root, attempt.id);
            const children = ['host-process', 'omp-process'].map(name => launchOwned(join(root, name), '/bin/sleep', ['60'], { cwd: f.root, env: { PATH: process.env.PATH } }));
            let hostPid: number | undefined; let ompPid: number | undefined;
            for (let i = 0; i < 250 && (!hostPid || !ompPid); i++) {
              hostPid = (await inspectOwned(join(root, 'host-process'))).owner?.pid;
              ompPid = (await inspectOwned(join(root, 'omp-process'))).owner?.pid;
              if (!hostPid || !ompPid) await new Promise(resolve => setTimeout(resolve, 20));
            }
            assert.ok(hostPid && ompPid, 'Replacement process leases became live.');
            const server = createServer(async (req, res) => {
              let raw = ''; for await (const chunk of req) raw += chunk;
              const body = JSON.parse(raw); res.setHeader('Content-Type', 'application/json');
              if (body.op === 'events') res.end('[]');
              else if (body.op === 'close') res.end('{"events":[]}');
              else if (body.op === 'state') res.end('{"isStreaming":false,"isCompacting":false,"hasPendingAsyncWork":false}');
              else { if (body.op === 'input') inputs.push(body); res.end('{}'); }
            });
            replacements.push({ root, children, server });
            await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
            const address = server.address(); if (!address || typeof address === 'string') throw new Error('Missing replacement host port.');
            const url = `http://127.0.0.1:${address.port}/`;
            store.claimAttempt(attempt.id);
            store.recordAttemptIdentity(attempt.id, { hostPid: hostPid!, ompPid: ompPid!, sessionId: attempt.id, hostUrl: url, pane: `%replay${replacements.length}` });
            fresh.register({ attemptId: attempt.id, runId: id, token: 'fixture', url, pane: `%replay${replacements.length}`, cursor: 0 });
          }
        };
        return fresh;
      };
      const first = makeFresh();
      await (first as unknown as RuntimeSupervisor).initialize();
      const check = (fresh: Seam) => {
        assert.equal(store.getRun(runId).status, 'running', store.getRun(runId).attentionReason ?? 'Interrupted team transition should continue.');
        assert.equal(store.getRun(runId).desiredState, 'running');
        assert.equal(store.getRun(runId).teamTransition, undefined);
        assert.equal(store.teamProposals(runId)[0].status, 'rejected');
        assert.deepEqual(store.runtime(runId).assignments.map(item => ({ id: item.id, role: item.role, model: item.model })), captured);
        assert.ok(store.runtime(runId).assignments.every(item => !item.retiredAt && (item.automaticCycles ?? 0) === 0));
        assert.equal(fresh.hosts.size, 2);
        assert.ok(fresh.messages.has(runId), 'Messaging is registered for the resumed team.');
      };
      check(first);
      for (const host of hosts) {
        assert.equal(store.getAttempt(host.attemptId).ownership, 'stopped');
        for (const name of ['host-process', 'omp-process']) assert.equal((await inspectOwned(join(old.config.root, host.attemptId, name))).state, 'stopped');
      }
      assert.ok(inputs.some(item => item.id.startsWith('continuation:') && JSON.parse(item.message).teamProposals[0].status === 'rejected'));
      await first.reconcile(runId);
      const notice = store.deliveryNotices(runId)[0];
      assert.equal(notice.disposition, 'received');
      assert.equal(store.deliveryNotices(runId).length, 1);
      assert.equal(inputs.filter(item => item.id === notice.id).length, 1);

      // Replay a crash after continuation but before the transition marker was cleared.
      store.beginTeamTransition(proposal.id);
      currentRoles = [{ ...role, enabled: true }];
      for (const host of first.hosts.values()) await first.stopObserver(host);
      first.hosts.clear();
      const second = makeFresh();
      await (second as unknown as RuntimeSupervisor).initialize();
      check(second);
      assert.deepEqual(store.deliveryNotices(runId).map(item => item.id), [notice.id]);
      assert.equal(store.applyTeamProposal(proposal.id, currentRoles).status, 'rejected');
      assert.equal(store.deliveryNotices(runId).length, 1);
      await second.reconcile(runId);
      assert.equal(store.deliveryNotices(runId)[0].disposition, 'received');
    } finally {
      for (const fresh of freshes) { for (const host of fresh.hosts.values()) await fresh.stopObserver(host); clearInterval(fresh.timer); }
      for (const item of replacements) {
        await stopOwned(join(item.root, 'host-process')); await stopOwned(join(item.root, 'omp-process'));
        for (const child of item.children) child.stdin.end();
        item.server.closeAllConnections(); await new Promise<void>(resolve => item.server.close(() => resolve()));
      }
      reopened?.close(); await f.close();
    }
  });
});

test('an observer failure requires attention and pause still confirms stopped ownership', async t => {
  const f = await fixture(); t.after(f.close);
  const { runId, hosts } = await f.addRun(['Coordinator']);
  // A complete but malformed journal line fails collection after the owner is confirmed stopped.
  writeFileSync(join(f.root, 'hosts', hosts[0].attemptId, 'events.jsonl'), '{malformed\n');
  await hosts[0].stop();
  f.supervisor.paneExited(hosts[0].pane);
  await until(() => f.store.getRun(runId).status === 'attention_required', 'observer failure surfaced');
  assert.match(f.store.getRun(runId).attentionReason ?? '', /observation failed|unreadable/i);
  assert.equal(f.store.getAttempt(hosts[0].attemptId).ownership, 'stopped');
  assert.equal(f.supervisor.observes(hosts[0].attemptId), false);
  assert.equal(f.store.runtime(runId).assignments[0].automaticCycles ?? 0, 0);
});

test('tmux dead-pane signal wakes ownership check before its RPC long-poll expires', { skip: !(() => { try { execFileSync('tmux', ['-V']); return true; } catch { return false; } })() }, async t => {
  const f = await fixture();
  const socket = join(f.root, 'private.sock');
  const tmux = (...args: string[]) => execFileSync('tmux', ['-S', socket, '-f', '/dev/null', ...args], { encoding: 'utf8' }).trim();
  // The pane exits only when the test ends its input, after control mode is attached; a timed exit races slow setup.
  const pane = tmux('new-session', '-d', '-s', 'crewroom', '-P', '-F', '#{pane_id}', '/bin/cat');
  tmux('set-option', '-w', '-t', pane, 'remain-on-exit', 'on');
  const { runId, hosts } = await f.addRun(['Coordinator'], pane);
  const terminals = new TerminalHub(socket, join(f.root, 'hosts'), () => hosts[0].attemptId);
  terminals.onPaneExited(exited => f.supervisor.paneExited(exited));
  t.after(async () => { terminals.close(); try { tmux('kill-server'); } catch {} await f.close(); });
  await until(() => terminals.connected && hosts[0].requests() > 0, 'control mode and host observer connected');
  await hosts[0].stop(); tmux('send-keys', '-t', pane, 'C-d');
  const start = Date.now();
  await until(() => f.store.getAttempt(hosts[0].attemptId).ownership === 'stopped', 'pane-exit ownership check', 4000);
  assert.ok(Date.now() - start < 4000, 'Pane signal preempts five-second long poll.');
  assert.equal(f.store.getRun(runId).status, 'attention_required');
});
