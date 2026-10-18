import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test, { type TestContext } from 'node:test';
import { canonical, digest } from './for14-case-pack';
import { assertLiveProfile, assertProfileExecution, auditToolCatalogs } from './for14-tool-audit';

test('scripted provider evidence cannot pass the live matrix profile gate', () => {
  assert.doesNotThrow(() => assertLiveProfile({ inference: 'live', executionMode: 'live' }));
  assert.throws(() => assertLiveProfile({ inference: 'scripted', executionMode: 'live' }), /scripted provider/);
  assert.throws(() => assertLiveProfile({ inference: 'live', executionMode: 'scripted' }), /scripted comparison/);
});

test('a smoke binds exact runner identities, models and tools while allowing isolated application paths', () => {
  const execution = { runner: { executable: '/provider/codex', profileHash: 'worker-profile' },
    coordinatorRunner: { executable: '/provider/codex', profileHash: 'coordinator-profile' },
    model: { id: 'worker-model', effort: 'low' }, coordinatorModel: { id: 'coordinator-model', effort: 'low' },
    tools: { worker: [{ name: 'write_file', parameters: { type: 'object' } }], coordinator: [{ name: 'send_message' }] },
    application: { daemon: '/matrix/bin/crewroomd' } };
  const profile = { ...structuredClone(execution), application: { daemon: '/smoke/bin/crewroomd' } };
  assert.doesNotThrow(() => assertProfileExecution(profile, execution));
  for (const field of ['runner', 'coordinatorRunner', 'model', 'coordinatorModel', 'tools']) {
    const changed: any = structuredClone(execution);
    changed[field] = { ...changed[field], changed: true };
    assert.throws(() => assertProfileExecution(profile, changed), new RegExp(`same execution.${field}`));
    delete changed[field];
    assert.throws(() => assertProfileExecution(profile, changed), new RegExp(`retain ${field}`));
  }
  for (const field of ['executable', 'profileHash']) {
    const changed = structuredClone(execution); changed.runner[field as 'executable' | 'profileHash'] = 'different';
    assert.throws(() => assertProfileExecution(profile, changed), /same execution.runner/);
  }
});

function fixture(kind: string, context: TestContext) {
  const runtime = mkdtempSync(join(tmpdir(), 'cr-catalog-'));
  context.after(() => rmSync(runtime, { recursive: true, force: true }));
  const tool = (recipient: string) => ({ name: 'send_message', description: 'Send a scoped message', label: 'Message', loadMode: 'essential', parameters: {
    type: 'object', properties: { to: { type: 'string', enum: [recipient] }, body: { type: 'string' } }, required: ['to', 'body'], additionalProperties: false,
  } });
  const manifest = { execution: { tools: { coordinator: [tool('Builder')], worker: [tool('Lead')] } } };
  const assignments = [{ id: 'lead', coordinator: true }, { id: 'builder', coordinator: false }];
  const attempts = assignments.map((assignment, index) => ({ id: 'attempt-' + index, assignmentId: assignment.id, hostPid: index + 100,
    ownership: 'stopped', runnerKind: kind, runnerCwd: join(runtime, assignment.id), sessionId: 'session-' + index }));
  for (const attempt of attempts) {
    const tools = manifest.execution.tools[attempt.assignmentId === 'lead' ? 'coordinator' : 'worker'];
    mkdirSync(join(runtime, attempt.id), { recursive: true });
    writeFileSync(join(runtime, attempt.id, 'events.jsonl'), JSON.stringify({ sequence: 1, event: { type: 'host_ready', hostPid: attempt.hostPid,
      sessionId: attempt.sessionId, toolCatalogHashKind: 'canonical-json-sha256', toolCatalogHash: digest(canonical(tools)) } }) + '\n');
    if (kind === 'codex') {
      const sessions = join(attempt.runnerCwd, 'codex-session', 'sessions'); mkdirSync(sessions, { recursive: true });
      writeFileSync(join(sessions, 'rollout.jsonl'), JSON.stringify({ type: 'session_meta', payload: { id: attempt.sessionId, dynamic_tools:
        tools.map(tool => ({ type: 'function', name: tool.name, description: tool.description, inputSchema: tool.parameters })) } }) + '\n');
    }
  }
  return { runtime, manifest, cell: { id: 'cell', result: { assignments, attempts } } };
}

test('actual host evidence binds both actors for each harness and Codex also binds its session catalog', context => {
  for (const kind of ['codex', 'omp', 'claude-code']) {
    const f = fixture(kind, context), result = auditToolCatalogs(f.cell, f.manifest, f.runtime);
    assert.equal(result.passed, true); assert.equal(result.actors.length, 2);
    assert.ok(result.actors.every((actor: any) => actor.actualToolsSha256 === actor.frozenToolsSha256));
    assert.equal(result.actors.every((actor: any) => !!actor.codex), kind === 'codex');
  }
});

function unusedBeforeReplacement(context: TestContext) {
  const f = fixture('codex', context), replacement = f.cell.result.attempts[1];
  const unused = { ...replacement, id: 'unused-attempt', hostPid: 200, sessionId: 'unprompted-session' };
  f.cell.result.attempts.splice(1, 0, unused);
  mkdirSync(join(f.runtime, unused.id));
  writeFileSync(join(f.runtime, unused.id, 'inputs.jsonl'), '');
  const ready = JSON.parse(readFileSync(join(f.runtime, replacement.id, 'events.jsonl'), 'utf8'));
  ready.event.hostPid = unused.hostPid; ready.event.sessionId = unused.sessionId;
  writeFileSync(join(f.runtime, unused.id, 'events.jsonl'), [ready,
    { sequence: 2, event: { type: 'rpc_stream_closed', reason: 'Native runner output closed.' } },
    { sequence: 3, event: { type: 'process_exit', code: 0, signal: null } }].map(v => JSON.stringify(v)).join('\n') + '\n');
  return { ...f, unused };
}

test('an unprompted stopped Codex attempt is explicit unused evidence while its replacement requires a real rollout', context => {
  const f = unusedBeforeReplacement(context), result = auditToolCatalogs(f.cell, f.manifest, f.runtime);
  const unused = result.actors.find((a: any) => a.attemptId === f.unused.id);
  assert.equal(unused.codex.state, 'unused'); assert.equal(unused.codex.independentCatalogProof, false);
  assert.equal(unused.codex.inputJournal.bytes, 0); assert.equal(unused.codex.rollout, undefined);
  assert.equal(unused.actualToolsSha256, unused.frozenToolsSha256);
  assert.ok(result.actors.filter((a: any) => a.attemptId !== f.unused.id).every((a: any) => a.codex.independentCatalogProof === true));
  rmSync(join(f.unused.runnerCwd, 'codex-session', 'sessions', 'rollout.jsonl'));
  assert.throws(() => auditToolCatalogs(f.cell, f.manifest, f.runtime), /inputs.jsonl|rollout/);
});

test('missing Codex rollouts cannot be excused by absent journals, accepted input, execution, unknown activity or mismatched identity', context => {
  for (const mutation of ['missing-journal', 'input', 'agent_start', 'provider_usage', 'host_tool_call', 'message_end', 'unknown_activity', 'session', 'missing-session', 'host-catalog']) {
    const f = unusedBeforeReplacement(context), directory = join(f.runtime, f.unused.id);
    if (mutation === 'missing-journal') rmSync(join(directory, 'inputs.jsonl'));
    else if (mutation === 'input') writeFileSync(join(directory, 'inputs.jsonl'), JSON.stringify({ id: 'input', disposition: 'accepted' }) + '\n');
    else {
      const frames = readFileSync(join(directory, 'events.jsonl'), 'utf8').trim().split('\n').map(s => JSON.parse(s));
      if (mutation === 'session') frames[0].event.sessionId = 'another-session';
      else if (mutation === 'missing-session') { Reflect.deleteProperty(f.unused, 'sessionId'); delete frames[0].event.sessionId; }
      else if (mutation === 'host-catalog') frames[0].event.toolCatalogHash = 'tampered';
      else frames.push({ sequence: 4, event: { type: mutation } });
      writeFileSync(join(directory, 'events.jsonl'), frames.map(v => JSON.stringify(v)).join('\n') + '\n');
    }
    assert.throws(() => auditToolCatalogs(f.cell, f.manifest, f.runtime), mutation);
  }
});

test('a changed recipient, missing catalog proof or live ownership cannot count as an audited profile', context => {
  for (const mutation of ['recipient', 'missing-proof', 'live']) {
    const f = fixture('omp', context);
    if (mutation === 'recipient') f.manifest.execution.tools.worker[0].parameters.properties.to.enum.push('Builder');
    if (mutation === 'missing-proof') {
      const path = join(f.runtime, 'attempt-1', 'events.jsonl'), frame = JSON.parse(readFileSync(path, 'utf8'));
      delete frame.event.toolCatalogHash; writeFileSync(path, JSON.stringify(frame) + '\n');
    }
    if (mutation === 'live') f.cell.result.attempts[1].ownership = 'running';
    assert.throws(() => auditToolCatalogs(f.cell, f.manifest, f.runtime));
  }
});

test('Codex actual schema drift is rejected even when the host claims the expected catalog hash', context => {
  const f = fixture('codex', context), attempt = f.cell.result.attempts[1];
  const path = join(attempt.runnerCwd, 'codex-session', 'sessions', 'rollout.jsonl');
  const frame = JSON.parse(readFileSync(path, 'utf8'));
  frame.payload.dynamic_tools[0].inputSchema.properties.to.enum.push('Builder');
  writeFileSync(path, JSON.stringify(frame) + '\n');
  assert.throws(() => auditToolCatalogs(f.cell, f.manifest, f.runtime), /actual Codex catalog differs/);
});
