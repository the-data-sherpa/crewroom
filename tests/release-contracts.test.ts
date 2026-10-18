import assert from 'node:assert/strict';
import { test, type TestContext } from 'node:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { defaultRoles } from '../server/roles';
import { resolveAssignment, isolatedModelSettings } from '../server/runtime/model-policy';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { savedInputs } from '../server/runtime/inputs';
import { RepositoryWork } from '../server/repository-work';

test('RTE-01/03: all 950 captured role/model bindings preserve complete identity independently of eligibility', () => {
  const fixture = JSON.parse(readFileSync('docs/testing/fixtures/omp-model-catalog-2026-09-26.json', 'utf8'));
  const roles = JSON.parse(readFileSync('docs/testing/model-profiles.json', 'utf8')).roles;
  let bindings = 0;
  for (const role of roles) for (const model of fixture.models) {
    const capability = { provider: model.provider, id: model.id, api: model.api, inputs: model.declaredInput, efforts: ['off'], tools: null };
    const plan = resolveAssignment({ ...defaultRoles[0], id: role.id, model: model.selector, effort: 'off' }, [capability], {}, {});
    assert.equal(plan.resolved.selector, model.selector);
    assert.equal(plan.resolved.id, model.id);
    assert.equal(plan.resolved.api, model.api);
    assert.equal(plan.resolved.tools, null, 'A binding is not proof of tool capability.');
    bindings++;
  }
  assert.equal(bindings, 950);
});

test('RTE-10/11: deferred advisers and secondary OMP routes cannot silently select another model', () => {
  const plan = resolveAssignment({ ...defaultRoles[0], model: 'fixture/model', effort: 'low' }, [
    { provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true },
  ], {}, {});
  const settings = isolatedModelSettings(plan);
  assert.equal(settings.advisor.enabled, false);
  assert.equal(settings.prewalk.enabled, false);
  assert.equal(settings.compaction.enabled, false);
  assert.equal(settings.retry.modelFallback, false);
  assert.ok(Object.values(settings.modelRoles).every(selector => selector === 'fixture/model:low'));
  assert.ok(Object.values(settings.retry.fallbackChains).every(chain => chain.length === 0));
});

function fixture(t: TestContext) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-release-'));
  seedRepository(root);
  let store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Release fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Retain bounded messages', description: '', repositoryId: repo.id });
  const accepted = store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Bound the pending inbox', criteria: ['Preserve accepted work'] });
  const run = store.queue(task.id, { revision: accepted.revision });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const sender = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Coordinate.' });
  const recipient = store.admitAssignment(run.id, { name: 'Specialist', role, model, coordinator: false, input: 'Read.' });
  return { root, task: accepted, get store() { return store; }, run, sender, recipient, reopen() { store.close(); store = new Workbench(join(root, 'workflow.sqlite')); } };
}

test('MSG-09/LIM-03: a stalled inbox applies durable backpressure without losing accepted sends or keyed replays', t => {
  const f = fixture(t);
  const sent = Array.from({ length: 100 }, (_, i) => f.store.prepareMessage(f.sender.attempt.id, `send-${i}`, { to: 'Specialist', body: `Body ${i}` }));
  assert.throws(() => f.store.prepareMessage(f.sender.attempt.id, 'overflow', { to: 'Specialist', body: 'Wait for the reader' }), /backlog|pending.*limit/i);
  f.reopen();
  assert.equal(f.store.runtime(f.run.id).messages.length, 100);
  assert.equal(f.store.prepareMessage(f.sender.attempt.id, 'send-0', { to: 'Specialist', body: 'Body 0' }).id, sent[0].id);
  assert.throws(() => f.store.prepareMessage(f.sender.attempt.id, 'large', { to: 'Specialist', body: 'x'.repeat(40001) }));
  assert.throws(() => f.store.prepareMessage(f.sender.attempt.id, 'forged', { to: 'Specialist', body: 'Body', from: f.recipient.assignment.id }));
  assert.equal(f.store.runtime(f.run.id).messages.length, 100);
  f.store.recordMessageStored(sent[0].id, 1); f.store.acceptMessage(sent[0].id); f.store.recordDispatch(sent[0].id, true);
  assert.ok(f.store.prepareMessage(f.sender.attempt.id, 'released', { to: 'Specialist', body: 'Room after receipt' }).id);
  assert.equal(f.store.runtime(f.run.id).messages.length, 101, 'History is retained when pending capacity is released.');
});

test('LIF-09: failed durable writes roll back the whole operation and corrupt input journals retain uncertainty', t => {
  const f = fixture(t);
  const connection = new DatabaseSync(join(f.root, 'workflow.sqlite'));
  t.after(() => connection.close());
  const before = f.store.getTask(f.task.id); const history = f.store.requirementRevisions(f.task.id);
  connection.exec("CREATE TRIGGER fail_revision BEFORE INSERT ON requirement_revisions BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END;");
  assert.throws(() => f.store.refine(f.task.id, { revision: before.revision, sourceRevision: 1, brief: 'Unsaved revision', criteria: ['Unsaved'] }), /injected storage failure/);
  assert.deepEqual(f.store.getTask(f.task.id), before, 'The preceding ticket UPDATE must roll back too.');
  connection.exec('DROP TRIGGER fail_revision;');
  f.reopen();
  assert.deepEqual(f.store.requirementRevisions(f.task.id), history);
  assert.deepEqual(f.store.getTask(f.task.id), before);
  connection.exec("CREATE TRIGGER fail_message BEFORE INSERT ON peer_messages BEGIN SELECT RAISE(ABORT, 'injected unavailable storage'); END;");
  assert.throws(() => f.store.prepareMessage(f.sender.attempt.id, 'failed-save', { to: 'Specialist', body: 'Do not dispatch' }), /unavailable storage/);
  assert.equal(f.store.runtime(f.run.id).messages.length, 0);
  connection.exec('DROP TRIGGER fail_message;');
  const path = join(f.root, 'inputs.jsonl');
  const retained = '{"id":"first","disposition":"received","message":"Saved input"}\n{"id":';
  writeFileSync(path, retained);
  assert.throws(() => savedInputs(f.root));
  assert.equal(readFileSync(path, 'utf8'), retained, 'A corrupt journal is never reset or interpreted as an empty inbox.');
});

test('ROL-04: one reusable role receives independent repository instructions and check policies', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-projects-'));
  const store = new Workbench(join(root, 'workflow.sqlite'));
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const original = structuredClone(role);
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const work = new RepositoryWork(store, join(root, 'worktrees'));
  const snapshots = [];
  for (const name of ['alpha', 'beta']) {
    const path = join(root, name); seedRepository(path);
    writeFileSync(join(path, 'AGENTS.md'), `Only change ${name}. Run ${name}-check.\n`);
    execFileSync('git', ['-C', path, 'add', 'AGENTS.md']);
    execFileSync('git', ['-C', path, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'project contract'], { stdio: 'ignore' });
    const repository = saveRepositoryFixture(store, { name, path, targetBranch: 'main', checks: [{ id: `${name}-check`, image: 'sha256:' + 'a'.repeat(64), command: '/bin/true', args: [], timeoutMs: 1000, required: true }] });
    const task = store.createTask({ title: name, description: '', repositoryId: repository.id });
    store.refine(task.id, { revision: 1, sourceRevision: 1, brief: name, criteria: ['Match project'] });
    const run = store.queue(task.id, { revision: 2 });
    const owned = await work.prepare(run.id, { mode: 'new' });
    const assignment = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Apply the project contract.' });
    assert.deepEqual(assignment.assignment.role, original);
    assert.equal(owned.repository.checks?.[0].id, `${name}-check`);
    assert.ok(JSON.stringify(owned.instructions).includes(`Only change ${name}.`));
    snapshots.push(owned);
  }
  assert.notDeepEqual(snapshots[0].instructions, snapshots[1].instructions);
  assert.deepEqual(role, original);
});

test('LIM-03/UI-02: a stalled coordinator retains steering and refuses new input at the pending limit', t => {
  const f = fixture(t);
  const first = f.store.steer(f.run.id, { requestId: 'steer-0', body: 'Keep this instruction' });
  for (let i = 1; i < 100; i++) f.store.steer(f.run.id, { requestId: `steer-${i}`, body: `Instruction ${i}` });
  assert.throws(() => f.store.steer(f.run.id, { requestId: 'overflow', body: 'Not saved' }), /backlog|pending.*limit/i);
  f.reopen();
  assert.equal(f.store.steer(f.run.id, { requestId: 'steer-0', body: 'Keep this instruction' }).id, first.id);
  assert.equal(f.store.steering(f.run.id).length, 100);
  f.store.dispatchSteering(f.sender.attempt.id, [first.id], 'steering-receipt');
  f.store.receiveSteering(f.sender.attempt.id, 'steering-receipt');
  assert.ok(f.store.steer(f.run.id, { requestId: 'released', body: 'Room after receipt' }).id);
});

test('LIM-04: seeded duplicates, delayed receipts and restarts preserve one logical message and terminal cancellation', t => {
  // Seed/event index is part of every assertion so a failed schedule can be replayed.
  for (const seed of [7, 42, 20261002]) {
    const f = fixture(t); let random = seed;
    const next = () => { random = (Math.imul(random, 1664525) + 1013904223) >>> 0; return random; };
    const effects = new Map<number, string>();
    for (let step = 0; step < 100; step++) {
      const key = next() % 12; const event = next() % 4;
      const trace = `seed=${seed}, step=${step}, event=${event}, key=${key}`;
      if (event === 0) f.reopen();
      const message = f.store.prepareMessage(f.sender.attempt.id, `key-${key}`, { to: 'Specialist', body: `Message ${key}` });
      if (effects.has(key)) assert.equal(message.id, effects.get(key), trace);
      effects.set(key, message.id);
      if (event >= 2) f.store.recordMessageStored(message.id, key + 1);
      if (event === 3) {
        f.store.acceptMessage(message.id); f.store.recordDispatch(message.id, true);
        f.store.recordDispatch(message.id); // Delayed dispatch acknowledgement must not undo a receipt.
        assert.equal(f.store.getPeerMessage(message.id).disposition, 'received', trace);
      }
      assert.equal(f.store.runtime(f.run.id).messages.length, effects.size, trace);
    }
    f.store.setDesiredState(f.run.id, 'cancelled'); f.store.beginPause(f.run.id); f.store.finishPause(f.run.id);
    f.reopen();
    assert.equal(f.store.getRun(f.run.id).status, 'cancelled', `seed=${seed}`);
    assert.throws(() => f.store.setDesiredState(f.run.id, 'running'), /cancelled/);
    assert.equal(f.store.runtime(f.run.id).messages.length, effects.size, `seed=${seed}`);
  }
});

test('MSG-09: an idle mailbox observes at most once per 1.5 seconds despite notification bursts', async t => {
  const { Messaging, RustMailClient } = await import('../server/runtime/messaging');
  const { createServer } = await import('node:http');
  const f = fixture(t); f.store.bindMailbox(f.sender.assignment.id, 'BlueLake'); f.store.bindMailbox(f.recipient.assignment.id, 'GreenCastle');
  let count = 0; let now = 0;
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    const request = JSON.parse(raw); res.setHeader('Content-Type', 'application/json');
    if (request.method !== 'tools/call') { res.end(request.id ? JSON.stringify({ id: request.id, result: {} }) : ''); return; }
    assert.equal(request.params.name, 'fetch_inbox_events'); count++;
    res.end(JSON.stringify({ id: request.id, result: { structuredContent: { events: [], next_cursor: 0, has_more: false } } }));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise<void>(resolve => server.close(() => resolve())));
  const messaging = new Messaging(f.store, new RustMailClient(`http://127.0.0.1:${(server.address() as { port: number }).port}/mcp/`, 'fixture'), f.root, 86400, () => now);
  await messaging.observe(f.run.id); assert.equal(count, 2);
  for (let i = 0; i < 100; i++) { now = i * 10; await messaging.observe(f.run.id); }
  assert.equal(count, 2, 'A burst does not poll per event.');
  now = 1499; await messaging.observe(f.run.id); assert.equal(count, 2);
  now = 1500; await messaging.observe(f.run.id); assert.equal(count, 4);
  assert.equal(f.store.runtime(f.run.id).messages.length, 0);
});
