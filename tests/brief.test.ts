import assert from 'node:assert/strict';
import { test } from 'node:test';
import { presentBriefs, snapshotBriefs, type BriefRequest } from '../server/runtime/brief';

const context = { criteria: ['Dialog scrolls', 'Input is retained'], checkIds: ['unit', 'browser'], existing: [] };
const request = (name: string, extra: Partial<BriefRequest> = {}): BriefRequest => ({ name, criteria: [], dependsOn: [], ...extra });

test('R6: a brief snapshots the accepted criterion text with its obligation, checks and dependencies', () => {
  const briefs = snapshotBriefs([
    request('Implementer', { criteria: [{ index: 0, obligation: 'Fix the scroll container.', checkIds: ['unit', 'unit'] }] }),
    request('Reviewer', { criteria: [{ index: 0, obligation: 'Verify scrolling on the candidate.', checkIds: ['browser'] }, { index: 1, obligation: 'Confirm retention.', checkIds: [] }], dependsOn: ['Implementer'] }),
  ], context);
  assert.deepEqual(briefs.get('Implementer'), { criteria: [{ index: 0, criterion: 'Dialog scrolls', obligation: 'Fix the scroll container.', checkIds: ['unit'] }], dependsOn: [] });
  assert.deepEqual(briefs.get('Reviewer')?.criteria.map(item => item.criterion), ['Dialog scrolls', 'Input is retained']);
  assert.deepEqual(briefs.get('Reviewer')?.dependsOn, ['Implementer']);
});

test('R6: briefs reject unknown criteria, unconfigured checks, unknown or self dependencies, and cycles', () => {
  assert.throws(() => snapshotBriefs([request('A', { criteria: [{ index: 2, obligation: 'x', checkIds: [] }] })], context), /criterion 2/);
  assert.throws(() => snapshotBriefs([request('A', { criteria: [{ index: 0, obligation: 'x', checkIds: [] }, { index: 0, obligation: 'y', checkIds: [] }] })], context), /once/);
  assert.throws(() => snapshotBriefs([request('A', { criteria: [{ index: 0, obligation: 'x', checkIds: ['lint'] }] })], context), /check lint/);
  assert.throws(() => snapshotBriefs([request('A', { dependsOn: ['Ghost'] })], context), /dependency Ghost/);
  assert.throws(() => snapshotBriefs([request('A', { dependsOn: ['A'] })], context), /itself/);
  assert.throws(() => snapshotBriefs([request('A', { dependsOn: ['B'] }), request('B', { dependsOn: ['A'] })], context), /cycle/);
  // An existing teammate's recorded dependency closes a cycle with a newly proposed one.
  const existing = [{ name: 'Implementer', brief: { criteria: [], dependsOn: ['Tester'] } }];
  assert.throws(() => snapshotBriefs([request('Tester', { dependsOn: ['Implementer'] })], { ...context, existing }), /cycle/);
  assert.equal(snapshotBriefs([request('Tester')], { ...context, existing }).get('Tester')?.dependsOn.length, 0);
});

test('R6: a retired teammate stays a valid dependency and its brief stays visible to dependents', () => {
  const implementer = { name: 'Implementer', role: { name: 'Implementer' }, retiredAt: '2026-09-29T00:00:00Z', brief: { criteria: [{ index: 0, criterion: 'Dialog scrolls', obligation: 'Implement behavior', checkIds: [] }], dependsOn: [] } };
  const reviewer = { name: 'Reviewer', role: { name: 'Reviewer' }, brief: { criteria: [], dependsOn: ['Implementer'] } };
  const unrelated = { name: 'Writer', role: { name: 'Writer' }, retiredAt: '2026-09-29T00:00:00Z', brief: { criteria: [], dependsOn: [] } };
  assert.equal(snapshotBriefs([request('Tester', { dependsOn: ['Implementer'] })], { ...context, existing: [implementer, reviewer, unrelated] }).get('Tester')?.dependsOn[0], 'Implementer');
  const prompt = presentBriefs(reviewer, [implementer, reviewer, unrelated]);
  assert.match(prompt, /Implement behavior/); assert.match(prompt, /"retired":true/);
  assert.doesNotMatch(prompt, /"name":"Writer"/, 'An unreferenced retired teammate is not presented.');
});
