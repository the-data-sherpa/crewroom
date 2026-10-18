import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { Workbench } from '../server/workbench';
import { publicationFixture } from './fixtures/publication';

test('DEL-04/LIF-03: controller delivery notices survive restart, retain receipt uncertainty and target only the coordinator', async t => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-notices-')); const fixture = (await publicationFixture(root));
  let store = fixture.store; t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  const { run, coordinator, reporter } = fixture;
  const payload = { kind: 'publication', subjectId: 'publication-1', prState: 'confirmed', ready: false };
  const first = store.noticeDelivery(run.id, payload);
  assert.equal(store.noticeDelivery(run.id, payload).id, first.id);
  assert.throws(() => store.dispatchDeliveryNotice(reporter.attempt.id, first.id), /coordinator/);
  store.dispatchDeliveryNotice(coordinator.attempt.id, first.id);
  store.close(); store = new Workbench(join(root, 'workflow.sqlite'));
  assert.equal(store.deliveryNotices(run.id)[0].disposition, 'dispatched');
  assert.throws(() => store.restoreUndispatchedInput(coordinator.attempt.id, first.id), /stopped/);
  store.recordOwnership(coordinator.attempt.id, true);
  store.restoreUndispatchedInput(coordinator.attempt.id, first.id);
  assert.equal(store.deliveryNotices(run.id)[0].disposition, 'pending');
  const next = store.noticeDelivery(run.id, { ...payload, ready: true });
  assert.notEqual(next.id, first.id); assert.equal(store.deliveryNotices(run.id).length, 2);
});
