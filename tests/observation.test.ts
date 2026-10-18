import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { DatabaseSync } from 'node:sqlite';
import { Workbench } from '../server/workbench';
import { seedRepository, saveRepositoryFixture } from './fixtures/repository';
import { defaultRoles } from '../server/roles';
import { resolveAssignment } from '../server/runtime/model-policy';
import { ObservationStream } from '../server/runtime/observation';
import { startServer } from '../server/app';

test('runtime observation pages every attempt event exactly once without embedding history in snapshots', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-event-pages-'));
  seedRepository(root);
  const store = new Workbench(join(root, 'workflow.sqlite'));
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Page observations', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Retain events', criteria: ['Ordered observation'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const first = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Start' });
  const second = store.admitAssignment(run.id, { name: 'Observer', role, model, coordinator: false, input: 'Observe' });
  for (let index = 1; index <= 7; index++) store.recordAgentEvent(index % 2 ? first.attempt.id : second.attempt.id, Math.ceil(index / 2), { type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: `event-${index}` }] } });
  store.close();
  const server = await startServer({ port: 0, dataDir: root, serveUi: false, rolePath: join(root, 'roles.json') });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true, force: true }); });
  const snapshot = await (await fetch(`${server.origin}/api/runs/${run.id}/runtime`)).json();
  assert.equal('events' in snapshot, false);
  let cursor = 0; const received: string[] = [];
  for (let page = 0; page < 4; page++) {
    const response = await fetch(`${server.origin}/api/runs/${run.id}/events?after=${cursor}&limit=2`);
    assert.equal(response.status, 200);
    const result = await response.json();
    received.push(...result.events.map((item: { event: { message: { content: { text: string }[] } } }) => item.event.message.content[0].text));
    assert.ok(result.after >= cursor); cursor = result.after;
  }
  assert.deepEqual(received, Array.from({ length: 7 }, (_, index) => `event-${index + 1}`));
  assert.deepEqual((await (await fetch(`${server.origin}/api/runs/${run.id}/events?after=${cursor}`)).json()).events, []);
});

test('high-frequency progress is coalesced without suppressing final messages or tool calls', () => {
  let now = 0; const stream = new ObservationStream(() => now); let count = 0; let bytes = 0;
  for (; now < 12000; now++) {
    const message = { content: [{ type: 'toolCall', arguments: { content: 'x'.repeat(now * 10) } }] };
    const event = stream.accept({ type: 'message_update', messageId: 'long-edit', message,
      assistantMessageEvent: { type: 'toolcall_delta', contentIndex: 0, partial: message } });
    if (event) { count++; bytes += JSON.stringify(event).length; }
  }
  assert.equal(count, 48); assert.ok(bytes < 16000);
  const completed = { type: 'host_tool_call', toolName: 'write_file', arguments: { content: 'x'.repeat(120000) } };
  assert.equal(stream.accept(completed), completed);
  const final = { type: 'message_end', message: { content: [{ text: 'Actual final result' }] } };
  assert.equal(stream.accept(final), final);
  assert.ok(stream.accept({ type: 'message_update', messageId: 'next-message', assistantMessageEvent: { type: 'text_delta' } }));
});

test('stream observations stay bounded while completed evidence and replay identities survive reopen', t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-observation-')); const path = join(root, 'db');
  seedRepository(root);
  let store = new Workbench(path); t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const repo = saveRepositoryFixture(store, { name: 'Fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Stream a large edit', description: '', repositoryId: repo.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Observe progress', criteria: ['Retain completed evidence'] });
  const run = store.queue(task.id, { revision: 2 });
  const role = { ...defaultRoles[0], model: 'fixture/model', effort: 'low' as const };
  const model = resolveAssignment(role, [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }], {}, {});
  const { attempt } = store.admitAssignment(run.id, { name: 'Coordinator', role, model, coordinator: true, input: 'Start' });
  const message = { role: 'assistant', content: [{ type: 'toolCall', name: 'write_file', arguments: { content: 'source text\n'.repeat(20000) } }] };
  const partial = { type: 'message_update', messageId: 'message-1', assistantMessageEvent: { type: 'toolcall_delta', contentIndex: 0, partial: message }, message };
  store.recordAgentEvent(attempt.id, 1, partial);
  const observed = store.agentEvents(run.id)[0];
  assert.ok(JSON.stringify(observed).length < 1024, 'Progress must not persist repeated cumulative source snapshots.');
  assert.equal(observed.sequence, 1); assert.equal(observed.event.messageId, 'message-1');
  const completed = { type: 'host_tool_call', id: 'request-1', toolCallId: 'call-1', toolName: 'write_file', arguments: { path: 'source.ts', content: 'complete source' } };
  store.recordAgentEvent(attempt.id, 2, completed); store.recordAgentEvent(attempt.id, 3, { type: 'model_policy_rejected' });
  store.close();
  // Reproduce an existing v13 database containing the original cumulative event.
  const legacy = new DatabaseSync(path); legacy.prepare('UPDATE agent_events SET data=? WHERE sequence=1').run(JSON.stringify(partial)); legacy.exec('PRAGMA user_version=13'); legacy.close();
  store = new Workbench(path);
  assert.ok(JSON.stringify(store.agentEvents(run.id)).length < 2048);
  assert.deepEqual(store.agentEvents(run.id)[1].event, completed);
  assert.equal(store.hasAgentEvent(attempt.id, 'model_policy_rejected'), true);
  assert.equal(store.hasAgentEvent(attempt.id, 'process_exit'), false);
  store.recordAgentEvent(attempt.id, 1, partial);
  assert.equal(store.agentEvents(run.id).length, 3, 'Legacy journal replay keeps sequence deduplication.');
});
