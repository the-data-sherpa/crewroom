import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench } from '../server/workbench';
import { steeringTransitions, steeringClosedReason } from '../shared/steering';
import { steeringFixture } from './fixtures/steering';

function fixture(t: { after: (fn: () => void) => void }) {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-steering-'));
  const f = steeringFixture(root); let store = f.store;
  t.after(() => { store.close(); rmSync(root, { recursive: true, force: true }); });
  return { ...f, get store() { return store; }, reopen: () => { store.close(); store = new Workbench(f.path, f.clock); return store; } };
}

test('STE-01/02: recorded times survive reopen; replies cannot apply steering and receipts replay unchanged', t => {
  const f = fixture(t); const id = f.attempt.id;
  const message = f.store.steer(f.run.id, { requestId: 'one', body: 'Cover the empty case.' });
  assert.equal(message.createdAt, new Date(f.clock()).toISOString());
  const dispatchAt = f.tick(); f.store.dispatchSteering(id, [message.id], 'input-one');
  f.reopen(); const receivedAt = f.tick(); f.store.receiveSteering(id, 'input-one');
  f.tick(); f.store.receiveSteering(id, 'input-one');
  f.store.recordAgentEvent(id, 1, { type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: 'Applied and completed!' }] } });
  assert.equal(f.store.steering(f.run.id)[0].delivery, 'received');
  assert.deepEqual(f.store.steering(f.run.id)[0].transitions, [{ state: 'dispatched', at: dispatchAt }, { state: 'received', at: receivedAt }]);
  const appliedAt = f.tick(); const args = { messageId: message.id, disposition: 'applied', reason: 'Included in the plan.' };
  const applied = f.store.applySteering(id, 'receipt', args); f.tick();
  assert.deepEqual(f.store.applySteering(id, 'receipt', args), applied);
  assert.deepEqual(applied.transitions?.at(-1), { state: 'applied', at: appliedAt, reason: args.reason });
  assert.throws(() => f.store.applySteering(id, 'different', { ...args, disposition: 'rejected' }), /different disposition/);
  f.reopen(); assert.deepEqual(f.store.steering(f.run.id)[0], applied);
  assert.deepEqual(f.store.steer(f.run.id, { requestId: 'one', body: message.body }), applied);
});

test('STE-01/03: lost receipt stays unconfirmed; authenticated rejection establishes receipt once', t => {
  const f = fixture(t); const message = f.store.steer(f.run.id, { requestId: 'one', body: 'Change the entire product.' });
  f.tick(); f.store.dispatchSteering(f.attempt.id, [message.id], 'lost');
  f.reopen(); assert.equal(f.store.steering(f.run.id)[0].delivery, 'dispatched');
  assert.throws(() => f.store.dispatchSteering(f.attempt.id, [message.id], 'duplicate'), /not pending/);
  const at = f.tick(); const result = f.store.applySteering(f.attempt.id, 'receipt', { messageId: message.id, disposition: 'rejected', reason: 'Outside accepted requirements.' });
  assert.deepEqual(result.transitions?.slice(-2), [{ state: 'received', at }, { state: 'rejected', at, reason: 'Outside accepted requirements.' }]);
  f.store.receiveSteering(f.attempt.id, 'lost'); assert.deepEqual(f.store.steering(f.run.id)[0], result);
});

test('STE-04: cancellation settles only pending messages and preserves uncertain or received delivery', t => {
  const f = fixture(t);
  const pending = f.store.steer(f.run.id, { requestId: 'pending', body: 'Wait.' });
  const uncertain = f.store.steer(f.run.id, { requestId: 'uncertain', body: 'Receipt lost.' });
  const received = f.store.steer(f.run.id, { requestId: 'received', body: 'Await disposition.' });
  f.store.dispatchSteering(f.attempt.id, [uncertain.id], 'lost');
  f.store.dispatchSteering(f.attempt.id, [received.id], 'received'); f.store.receiveSteering(f.attempt.id, 'received');
  const at = f.tick(); f.store.setDesiredState(f.run.id, 'cancelled');
  assert.deepEqual(f.store.steering(f.run.id).map(m => m.delivery), ['not_delivered', 'dispatched', 'received']);
  assert.deepEqual(f.store.steering(f.run.id)[0].transitions, [{ state: 'not_delivered', at, reason: 'Cancellation was requested before delivery.' }]);
  f.store.beginPause(f.run.id); f.store.recordOwnership(f.attempt.id, true); f.store.finishPause(f.run.id); f.reopen();
  assert.equal(f.store.steer(f.run.id, { requestId: 'pending', body: pending.body }).id, pending.id);
  assert.throws(() => f.store.steer(f.run.id, { requestId: 'new', body: 'No more work.' }), /cancelled/);
  assert.throws(() => f.store.dispatchSteering(f.attempt.id, [pending.id], 'bad'), /cancelled/);
  // The stopped host's positive non-dispatch reconciliation must not requeue terminal input.
  f.tick(); f.store.restoreUndispatchedInput(f.attempt.id, 'lost');
  assert.equal(f.store.steering(f.run.id)[1].delivery, 'not_delivered');
  assert.equal(f.store.steering(f.run.id)[1].transitions?.at(-1)?.at, new Date(f.clock()).toISOString());
  assert.equal(f.store.steering(f.run.id)[2].delivery, 'received');
});

test('STE-03/04: paused input waits, stopped non-dispatch retains history, closure refuses new steering', t => {
  const f = fixture(t); f.store.setDesiredState(f.run.id, 'paused'); f.store.beginPause(f.run.id);
  f.store.recordOwnership(f.attempt.id, true); f.store.finishPause(f.run.id);
  const note = f.store.steer(f.run.id, { requestId: 'paused', body: 'After Resume.' });
  f.reopen(); assert.equal(f.store.steering(f.run.id)[0].delivery, 'pending');
  f.store.setDesiredState(f.run.id, 'running'); f.store.startExecution(f.run.id);
  f.tick(); f.store.dispatchSteering(f.attempt.id, [note.id], 'absent');
  f.tick(); f.store.restoreUndispatchedInput(f.attempt.id, 'absent');
  assert.deepEqual(f.store.steering(f.run.id)[0].transitions?.map(x => x.state), ['dispatched', 'pending']);
  f.store.beginPause(f.run.id); f.store.finishPause(f.run.id); f.tick(); f.store.closeRefinement(f.run.id);
  const saved = f.store.steering(f.run.id)[0]; assert.equal(saved.delivery, 'not_delivered');
  assert.equal(saved.dispositionReason, 'The run closed before delivery.');
  assert.throws(() => f.store.steer(f.run.id, { requestId: 'new', body: 'Late.' }), /closed/);
  assert.match(steeringClosedReason({ ...f.run, status: 'completed', desiredState: 'cancelled' })!, /completed/);
});

test('STE-04: v23 terminal migration preserves history and unknown times, including completed research', t => {
  const f = fixture(t);
  const note = f.store.steer(f.run.id, { requestId: 'old', body: 'Legacy pending.' });
  const legacyApplied = { ...note, delivery: 'applied' as const, dispositionReason: 'Legacy decision.', transitions: undefined };
  assert.deepEqual(steeringTransitions(legacyApplied).map(x => [x.state, x.at]), [['dispatched', undefined], ['received', undefined], ['applied', undefined]]);
  f.store.close(); const db = new DatabaseSync(f.path);
  db.prepare('UPDATE runs SET data=? WHERE id=?').run(JSON.stringify({ ...f.run, status: 'completed', desiredState: 'cancelled', purpose: 'research' }), f.run.id);
  db.prepare('UPDATE steering SET data=? WHERE id=?').run(JSON.stringify({ ...note, transitions: undefined }), note.id);
  db.exec('PRAGMA user_version=23;'); db.close();
  f.reopen(); const saved = f.store.steering(f.run.id)[0];
  assert.equal(saved.createdAt, note.createdAt); assert.equal(saved.delivery, 'not_delivered');
  assert.deepEqual(saved.transitions, [{ state: 'not_delivered', reason: 'Research completed before delivery. This message will not be sent if the run reopens.' }]);
  assert.throws(() => f.store.steer(f.run.id, { requestId: 'new', body: 'Late.' }), /completed/);
  f.reopen(); assert.deepEqual(f.store.steering(f.run.id)[0], saved);
});
