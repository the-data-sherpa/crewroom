import assert from 'node:assert/strict';
import test from 'node:test';
import { comparisonSettled } from './for14-stop-state';

const settled = () => ({ state: 'cancelled', cells: [
  { runId: 'observed', state: 'completed', runState: 'completed', result: { state: 'completed', cleanup: { state: 'stopped' } }, attempts: [{ state: 'idle', ownership: 'stopped' }] },
  { runId: 'prepared', state: 'cancelled', runState: 'cancelled', result: { state: 'cancelled', cleanup: { state: 'stopped' } }, attempts: [] },
  { state: 'cancelled' },
] });
const free = { plans: [] };

test('cleanup accepts stopped observations plus an unobserved cancelled run only after slot release', () => {
  assert.equal(comparisonSettled(settled(), free), true);
  assert.equal(comparisonSettled(settled(), { plans: [], slot: { runId: 'prepared' } }), false);
  assert.equal(comparisonSettled(settled(), undefined), false);
  assert.equal(comparisonSettled(settled(), {}), false);
});

test('an older daemon reporting cancelled cannot hide a preparation-only pending cell', () => {
  const value: any = settled();
  value.cells[1] = { runId: 'prepared', state: 'cancelling', runState: 'awaiting_runner', attempts: [] };
  assert.equal(comparisonSettled(value, free), false);
  assert.equal(comparisonSettled(value, { plans: [], slot: { runId: 'prepared' } }), false);
});

test('missing or unresolved ownership, run state, cleanup and cancellation remain pending', () => {
  for (const change of [
    (v: any) => { v.cells[0].attempts[0].ownership = 'uncertain'; },
    (v: any) => { delete v.cells[0].attempts; },
    (v: any) => { v.cells[1].runState = 'awaiting_runner'; },
    (v: any) => { v.cells[1].result.cleanup.state = 'unknown'; },
    (v: any) => { v.state = 'cancelling'; },
    (v: any) => { v.cells[2].state = 'planned'; },
  ]) {
    const value = settled(); change(value);
    assert.equal(comparisonSettled(value, free), false);
  }
});
