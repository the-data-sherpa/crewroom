import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';
import { FinalizationObserver } from './for14-finalization-observer';

const fixture = (name: string) => JSON.parse(readFileSync(new URL(`../fixtures/for14-finalization/${name}.json`, import.meta.url), 'utf8'));
const held = () => fixture('held');
const settled = () => fixture('settled');
const cell = (value: any) => value.cells[0];
const handoff = (value: any) => cell(value).events.find((event: any) => event.type === 'handoff');
const seal = (value: any) => cell(value).events.find((event: any) => event.type === 'finalization');

test('retained final-cell projection permits bounded assessment without changing failed wrapper history', () => {
  assert.equal(fixture('provenance').originalWrapperPassed, false);
  const observer = new FinalizationObserver();
  const first = observer.observe(held(), 1000);
  assert.equal(first.state, 'waiting'); assert.equal(first.deadline, 91000);
  assert.deepEqual(observer.observe(held(), 2000), first);
  assert.deepEqual(observer.observe(settled(), 5000), { ...first, state: 'settled' });
  assert.equal(observer.observe(settled(), 6000).state, 'none');
});

test('complete and incomplete accepted handoffs have the same terminal assessment policy', () => {
  const value = held();
  handoff(value).payload.arguments.status = 'incomplete';
  handoff(value).payload.receipt.status = 'incomplete';
  assert.equal(new FinalizationObserver().observe(value, 1000).state, 'waiting');
});

test('unsealed and duplicate handoff or finalization evidence fails closed', () => {
  for (const type of ['handoff', 'finalization']) {
    const absent = held(); cell(absent).events = cell(absent).events.filter((event: any) => event.type !== type);
    assert.throws(() => new FinalizationObserver().observe(absent, 1000));
    const duplicate = held(); cell(duplicate).events.push(structuredClone(cell(duplicate).events.find((event: any) => event.type === type)));
    assert.throws(() => new FinalizationObserver().observe(duplicate, 1000));
  }
});

test('other reasons including captured-source mismatch never receive an observation allowance', () => {
  for (const reason of [undefined, '', 'Captured evaluation source differs from the accepted handoff', 'Unknown runner failure']) {
    const value = held(); value.attentionReason = reason; cell(value).attentionReason = reason;
    assert.throws(() => new FinalizationObserver().observe(value, 1000));
  }
  const multiple = held(); multiple.cells.push(structuredClone(cell(multiple)));
  assert.throws(() => new FinalizationObserver().observe(multiple, 1000));
});

test('handoff authority, receipt and exact source/reference bindings are required', () => {
  const changes = [
    (v: any) => { handoff(v).attemptId = 'unretained'; },
    (v: any) => { handoff(v).runId = 'another-run'; },
    (v: any) => { seal(v).cellId = 'another-cell'; },
    (v: any) => { seal(v).comparisonId = 'another-comparison'; },
    (v: any) => { handoff(v).payload.arguments.status = 'incomplete'; },
    (v: any) => { handoff(v).payload.receipt.status = 'unknown'; },
    (v: any) => { handoff(v).payload.arguments.summary = 'different'; },
    (v: any) => { seal(v).payload.handoffEventId = 'another-handoff'; },
    (v: any) => { seal(v).payload.handoffRequestId = 'another-request'; },
    (v: any) => { seal(v).payload.source.sha256 = '0'.repeat(64); },
    (v: any) => { seal(v).payload.source.files.pop(); },
    (v: any) => { seal(v).payload.source.sha256 = '0'.repeat(64); handoff(v).payload.receipt.source.sha256 = '0'.repeat(64); },
    (v: any) => { delete v.manifestHash; },
    (v: any) => { delete v.id; delete handoff(v).comparisonId; delete seal(v).comparisonId; },
  ];
  for (const change of changes) { const value = held(); change(value); assert.throws(() => new FinalizationObserver().observe(value, 1000)); }
});

test('one fixed deadline survives repeated held, running, finalizing and cancelled snapshots', () => {
  const observer = new FinalizationObserver(); observer.observe(held(), 1000);
  const value = held(); value.attentionReason = null; cell(value).attentionReason = null;
  for (const [state, now] of [['running', 2000], ['finalizing', 4000], ['cancelled', 6000]] as const) {
    value.state = state; cell(value).state = 'finalizing';
    assert.equal(observer.observe(value, now).deadline, 91000);
  }
  assert.equal(observer.observe(held(), 90000).deadline, 91000);
  assert.throws(() => observer.observe(value, 91000), /within 90 seconds/);
});

test('another cell result cannot satisfy the tracked assessment and another held cell cannot reset it', () => {
  const observer = new FinalizationObserver(); observer.observe(held(), 1000);
  const value = held(); value.state = 'running'; cell(value).state = 'finalizing';
  value.cells.push({ id: 'other', result: {} });
  assert.equal(observer.observe(value, 2000).state, 'waiting');
  const other = held(); cell(other).id = 'other';
  for (const event of cell(other).events) event.cellId = 'other';
  assert.throws(() => observer.observe(other, 3000));
});

test('tracked cell/run/source identity stays mandatory when aggregate is no longer held', () => {
  const changes = [
    (v: any) => { v.cells = []; },
    (v: any) => { cell(v).runId = 'other'; },
    (v: any) => { v.manifestHash = '0'.repeat(64); },
    (v: any) => { seal(v).payload.source.sha256 = '0'.repeat(64); handoff(v).payload.receipt.source.sha256 = '0'.repeat(64); },
    (v: any) => { cell(v).events = []; },
  ];
  for (const change of changes) {
    const observer = new FinalizationObserver(); observer.observe(held(), 1000);
    const value = held(); value.state = 'running'; cell(value).state = 'finalizing'; change(value);
    assert.throws(() => observer.observe(value, 2000));
  }
});

test('a durable result must bind both assessed and handed-off source and arrive before deadline', () => {
  for (const change of [
    (v: any) => { delete cell(v).resultHash; },
    (v: any) => { cell(v).resultHash = '0'.repeat(64); },
    (v: any) => { cell(v).result.source.sha256 = '0'.repeat(64); },
    (v: any) => { cell(v).result.handoff.source.files.pop(); },
  ]) {
    const observer = new FinalizationObserver(); observer.observe(held(), 1000);
    const value = settled(); change(value); assert.throws(() => observer.observe(value, 2000));
  }
  const observer = new FinalizationObserver(); observer.observe(held(), 1000);
  assert.throws(() => observer.observe(settled(), 91000), /within 90 seconds/);
});

test('an ordinary unheld snapshot does not create an allowance', () => {
  const value = held(); value.state = 'running'; cell(value).state = 'running';
  assert.deepEqual(new FinalizationObserver().observe(value, 1000), { state: 'none' });
});

const retainedRoot = process.env.CREWROOM_FOR14_OBSERVER_RETAINED;
test('actual retained held15 snapshot and stopped durable16 database exercise the same observer without actors', { skip: !retainedRoot }, () => {
  const value = JSON.parse(readFileSync(join(retainedRoot!, 'comparison-held.json'), 'utf8'));
  const database = new DatabaseSync(join(retainedRoot!, 'data/workflow.sqlite'), { readOnly: true });
  try {
    database.exec('PRAGMA query_only=ON');
    const rows = database.prepare('SELECT data FROM comparison_cells ORDER BY ordinal').all();
    const cells = rows.map(row => JSON.parse(String(row.data)));
    assert.equal(cells.length, 16); assert.ok(cells.every(entry => entry.result?.observed));
    const target = cells[15]; assert.equal(target.resultHash, fixture('provenance').cellResultHash);
    const final = structuredClone(value); final.state = 'cancelled'; final.attentionReason = null;
    Object.assign(final.cells[15], { state: target.state, result: target.result, resultHash: target.resultHash, attentionReason: null });
    const observer = new FinalizationObserver(); assert.equal(observer.observe(value, 1000).state, 'waiting');
    assert.equal(observer.observe(final, 7000).state, 'settled');
    assert.equal(JSON.parse(readFileSync(join(retainedRoot!, 'comparison-result.json'), 'utf8')).passed, false);
  } finally { database.close(); }
});
