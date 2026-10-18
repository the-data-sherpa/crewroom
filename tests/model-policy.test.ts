import assert from 'node:assert/strict';
import { test } from 'node:test';
import { defaultRoles } from '../server/roles';
import { resolveAssignment, resolveRuntimeFallback, verifyAssignmentIdentity } from '../server/runtime/model-policy';

const catalog = [
  { provider: 'fixture', id: 'code', api: 'openai-responses', inputs: ['text'], efforts: ['off', 'low', 'high'], tools: true },
  { provider: 'fixture', id: 'plain', api: 'anthropic-messages', inputs: ['text'], efforts: ['off'], tools: false },
];
test('RTE-01/04: assignment defaults resolve explicitly and unsupported effort is rejected before launch', () => {
  const role = { ...defaultRoles[1], model: '', effort: 'default' as const };
  const policy = { default: { model: 'fixture/code', effort: 'low' } };
  const plan = resolveAssignment(role, catalog, policy, { inputs: ['text'], tools: true });
  assert.equal(plan.resolved.selector, 'fixture/code');
  assert.equal(plan.resolved.effort, 'low');
  assert.equal(plan.requested.model, '');
  assert.throws(() => resolveAssignment({ ...role, effort: 'medium' }, catalog, policy, {}), /Unsupported effort/);
  assert.throws(() => resolveAssignment(role, catalog, {}, {}), /explicit default/);
});

test('RTE-05/06/07: project constraints and verified capabilities apply to explicit fallbacks too', () => {
  const role = { ...defaultRoles[1], model: 'fixture/absent', effort: 'high' as const };
  assert.throws(() => resolveAssignment(role, catalog, {}, {}), /unavailable/);
  const policy = { fallbacks: { 'fixture/absent': [{ model: 'fixture/code', effort: 'low' }] } };
  const plan = resolveAssignment(role, catalog, policy, { tools: true, allowedModels: ['fixture/code'] });
  assert.equal(plan.resolved.selector, 'fixture/code');
  assert.equal(plan.fallbackFrom, 'fixture/absent');
  assert.equal(plan.requested.effort, 'high');
  assert.throws(() => resolveAssignment(role, catalog, policy, { allowedModels: ['fixture/plain'] }), /Project disallows/);
  assert.throws(() => resolveAssignment({ ...role, model: 'fixture/plain', effort: 'default' }, catalog, { default: { model: 'fixture/plain', effort: 'off' } }, { tools: true }), /tool capability/);
  assert.throws(() => resolveAssignment({ ...role, model: 'fixture/code' }, catalog, {}, { inputs: ['image'] }), /input capability/);
});

test('RTE-08: a different model, API or normalized effort cannot be admitted as the requested assignment', () => {
  const plan = resolveAssignment({ ...defaultRoles[1], model: 'fixture/code', effort: 'high' }, catalog, {}, {});
  const state = { model: { provider: 'fixture', id: 'code', api: 'openai-responses' }, thinkingLevel: 'high', sessionId: 'session-1', isStreaming: false, autoCompactionEnabled: false };
  assert.equal(verifyAssignmentIdentity(plan, state).sessionId, 'session-1');
  for (const changed of [
    { ...state, thinkingLevel: 'low' }, { ...state, model: { ...state.model, id: 'plain' } },
    { ...state, model: { ...state.model, api: 'other' } }, { ...state, autoCompactionEnabled: true },
  ]) assert.throws(() => verifyAssignmentIdentity(plan, changed), /mismatch|compaction/);
});

test('RTE-04: duplicate selectors are ambiguous even when their metadata agrees', () => {
  const role = { ...defaultRoles[1], model: 'fixture/code', effort: 'low' as const };
  assert.throws(() => resolveAssignment(role, [...catalog, { ...catalog[0] }], {}, {}), /ambiguous|duplicate/i);
  assert.throws(() => resolveAssignment({ ...role, model: 'fixture/missing' }, [...catalog, { ...catalog[0] }], {
    fallbacks: { 'fixture/missing': [{ model: 'fixture/code', effort: 'low' }] },
  }, {}), /ambiguous|duplicate/i);
  assert.throws(() => resolveAssignment({ ...role, model: 'code' }, catalog, {}, {}), /unavailable/);
});


test('RTE-09: runtime alternatives must be explicit, compatible, untried and preserve the original request', () => {
  const role = { ...defaultRoles[1], model: 'fixture/code', effort: 'low' as const };
  const extended = [...catalog, { ...catalog[0], id: 'alternate' }];
  const current = resolveAssignment(role, extended, {}, { tools: true });
  const policy = { fallbacks: { 'fixture/code': [{ model: 'fixture/missing', effort: 'low' }, { model: 'fixture/plain', effort: 'off' },
    { model: 'fixture/alternate', effort: 'medium' }, { model: 'fixture/alternate', effort: 'high' }] } };
  assert.equal(resolveRuntimeFallback(role, current, extended, {}, { tools: true }, []), undefined);
  const next = resolveRuntimeFallback(role, current, extended, policy, { tools: true, inputs: ['text'] }, ['fixture/code'])!;
  assert.equal(next.resolved.selector, 'fixture/alternate'); assert.equal(next.resolved.effort, 'high');
  assert.deepEqual(next.requested, current.requested); assert.equal(next.fallbackFrom, 'fixture/code');
  assert.equal(resolveRuntimeFallback(role, current, extended, policy, { allowedModels: ['fixture/code'] }, []), undefined);
  assert.equal(resolveRuntimeFallback(role, current, extended, policy, { tools: true }, ['fixture/alternate']), undefined);
  assert.throws(() => resolveRuntimeFallback(role, current, [...extended, extended[0]], policy, {}, []), /ambiguous/);
});
