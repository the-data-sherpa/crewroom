/** Read-only audit of actual supervised runner catalogs, after ownership stops. */
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { canonical, digest } from './for14-case-pack';

export function assertLiveProfile(profile: any) {
  assert.notEqual(profile.inference, 'scripted', 'A scripted provider contract cannot supply the live profile gate');
  assert.notEqual(profile.executionMode, 'scripted', 'A scripted comparison cannot supply the live profile gate');
}

/** Bind a passing profile to the exact runner, models and catalog being admitted.
 * Application paths differ between isolated copies; callers separately bind binary hashes. */
export function assertProfileExecution(profile: any, execution: any) {
  for (const field of ['runner', 'coordinatorRunner', 'model', 'coordinatorModel', 'tools']) {
    assert.ok(profile && Object.hasOwn(profile, field) && execution && Object.hasOwn(execution, field), `Tool/profile execution must retain ${field}`);
    assert.deepEqual(profile[field], execution[field], `Tool/profile smoke must cover the same execution.${field}`);
  }
}

export function auditToolCatalogs(cell: any, manifest: any, runtime: string) {
  const actors = cell.result.attempts.filter((attempt: any) => attempt.hostPid).map((attempt: any) => {
    assert.equal(attempt.ownership, 'stopped', 'Catalog evidence must be read after owned processes stop');
    const assignment = cell.result.assignments.find((item: any) => item.id === attempt.assignmentId);
    assert.ok(assignment, 'Every launched attempt must retain its frozen assignment');
    const role = assignment.coordinator ? 'coordinator' : 'worker';
    const expected = manifest.execution.tools[role];
    const eventsPath = join(runtime, attempt.id, 'events.jsonl');
    const eventBytes = readFileSync(eventsPath);
    const events = eventBytes.toString('utf8').split('\n').filter(Boolean).map(line => JSON.parse(line));
    const ready = events.filter(frame => frame.event?.type === 'host_ready');
    assert.equal(ready.length, 1, 'Exactly one retained host_ready must bind this attempt');
    assert.equal(ready[0].event.hostPid, attempt.hostPid);
    assert.equal(ready[0].event.toolCatalogHashKind, 'canonical-json-sha256');
    assert.equal(ready[0].event.toolCatalogHash, digest(canonical(expected)), `${role} actual host catalog differs from the frozen manifest`);
    const actor: any = { attemptId: attempt.id, assignmentId: assignment.id, role,
      actualToolsSha256: ready[0].event.toolCatalogHash, frozenToolsSha256: digest(canonical(expected)),
      hostEvents: { path: eventsPath, sha256: digest(eventBytes), bytes: eventBytes.length } };
    if (attempt.runnerKind === 'codex') {
      assert.ok(typeof attempt.sessionId === 'string' && attempt.sessionId.trim().length > 0, 'Codex attempt must retain its actual session identity');
      assert.equal(ready[0].event.sessionId, attempt.sessionId, 'Actual host session must bind the retained Codex attempt');
      const dynamic = expected.map((tool: any) => ({ type: 'function', name: tool.name, description: tool.description, inputSchema: tool.parameters }));
      const sessions = join(attempt.runnerCwd, 'codex-session', 'sessions');
      const candidates = readdirSync(sessions, { recursive: true }).filter((name): name is string => typeof name === 'string' && name.endsWith('.jsonl'));
      const matches = candidates.flatMap(name => {
        const path = join(sessions, name), bytes = readFileSync(path);
        const frame = JSON.parse(bytes.toString('utf8').split('\n', 1)[0]);
        return frame.type === 'session_meta' && frame.payload?.id === attempt.sessionId ? [{ path, bytes, tools: frame.payload.dynamic_tools }] : [];
      });
      if (matches.length === 0) {
        const inputsPath = join(runtime, attempt.id, 'inputs.jsonl'), inputs = readFileSync(inputsPath);
        assert.equal(inputs.length, 0, 'A missing Codex rollout requires an empty retained input journal');
        assert.ok(events.every(frame => ['host_ready', 'rpc_stream_closed', 'process_exit'].includes(frame.event?.type)),
          'A Codex attempt with any execution or unknown activity requires its retained rollout');
        actor.codex = { sessionId: attempt.sessionId, state: 'unused', independentCatalogProof: false,
          reason: 'Stopped before any input or execution; Codex did not materialize a rollout. Exact host catalog proof is retained separately.',
          inputJournal: { path: inputsPath, sha256: digest(inputs), bytes: inputs.length } };
        return actor;
      }
      assert.equal(matches.length, 1, 'Exactly one retained Codex rollout must bind this attempt');
      const actual = matches[0];
      assert.deepEqual(actual.tools, dynamic, `${role} actual Codex catalog differs from the frozen manifest`);
      actor.codex = { sessionId: attempt.sessionId, state: 'verified', independentCatalogProof: true,
        actualToolsSha256: digest(canonical(actual.tools)), frozenToolsSha256: digest(canonical(dynamic)),
        rollout: { path: actual.path, sha256: digest(actual.bytes), bytes: actual.bytes.length } };
    }
    return actor;
  });
  assert.ok(actors.some((actor: any) => actor.role === 'worker') && actors.some((actor: any) => actor.role === 'coordinator'));
  return { cellId: cell.id, passed: true, scope: 'Actual host catalogs match the frozen manifest; executed Codex attempts also require independent rollout catalogs, while unused attempts are explicitly classified without that proof', actors };
}
