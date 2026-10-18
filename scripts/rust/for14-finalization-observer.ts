import assert from 'node:assert/strict';
import { canonical, digest } from './for14-case-pack';

export const reconciliationReason = 'Host tool outcome requires reconciliation. The original intent is retained and no completed result was asserted.';
const observationMs = 90_000;

type Pending = { cellId: string; runId: string; manifestHash: string; comparisonId: string;
  handoffId: string; finalizationId: string; source: unknown; deadline: number };

function sealedIdentity(value: any, cell: any) {
  for (const id of [value.id, cell.id, cell.runId]) assert.ok(typeof id === 'string' && id.length > 0);
  assert.match(value.manifestHash ?? '', /^[a-f0-9]{64}$/);
  const handoffs = cell.events.filter((event: any) => event.type === 'handoff');
  const seals = cell.events.filter((event: any) => event.type === 'finalization');
  assert.equal(handoffs.length, 1, 'Held cell must have one accepted handoff');
  assert.equal(seals.length, 1, 'Held cell must have one durable finalization intent');
  const handoff = handoffs[0], seal = seals[0], receipt = handoff.payload?.receipt;
  for (const event of [handoff, seal]) {
    assert.equal(event.comparisonId, value.id);
    assert.equal(event.cellId, cell.id);
    assert.equal(event.runId, cell.runId);
    assert.ok(typeof event.id === 'string' && event.id.length > 0);
    assert.ok(typeof event.requestId === 'string' && event.requestId.length > 0);
  }
  assert.ok(cell.attempts.some((attempt: any) => attempt.id === handoff.attemptId && attempt.runId === cell.runId));
  assert.ok(['complete', 'incomplete'].includes(receipt?.status));
  assert.equal(handoff.payload.arguments?.status, receipt.status);
  assert.equal(handoff.payload.arguments?.summary, receipt.summary);
  assert.equal(seal.payload?.handoffEventId, handoff.id);
  assert.equal(seal.payload?.handoffRequestId, handoff.requestId);
  assert.match(receipt.source?.sha256 ?? '', /^[a-f0-9]{64}$/);
  assert.ok(Array.isArray(receipt.source.files) && receipt.source.files.length > 0);
  assert.equal(new Set(receipt.source.files.map((file: any) => file.path)).size, receipt.source.files.length);
  for (const file of receipt.source.files) {
    assert.ok(typeof file.path === 'string' && file.path.length > 0);
    assert.match(file.sha256 ?? '', /^[a-f0-9]{64}$/);
  }
  assert.equal(receipt.source.sha256, digest(canonical(receipt.source.files)), 'Accepted source manifest hash differs from its files');
  assert.equal(canonical(seal.payload.source), canonical(receipt.source));
  return { cellId: cell.id, runId: cell.runId, manifestHash: value.manifestHash, comparisonId: value.id,
    handoffId: handoff.id, finalizationId: seal.id, source: structuredClone(receipt.source) };
}

/** Observe trusted assessment after an already accepted handoff. This helper
 * never starts, resumes, retries or repairs a cell. */
export class FinalizationObserver {
  private pending?: Pending;

  observe(value: any, now: number) {
    assert.ok(Number.isFinite(now));
    const held = value.cells.filter((cell: any) => !cell.result && cell.state === 'held');
    if (value.state === 'held') {
      assert.equal(value.attentionReason, reconciliationReason, 'Comparison held: ' + value.attentionReason);
      assert.equal(held.length, 1, 'Reconciliation observation requires exactly one unfinished held cell');
      const cell = held[0];
      assert.equal(cell.attentionReason, reconciliationReason);
      const identity = sealedIdentity(value, cell);
      if (this.pending) {
        const { deadline: _deadline, ...prior } = this.pending;
        assert.deepEqual(identity, prior, 'The held cell or accepted finalization changed');
      } else this.pending = { ...identity, deadline: now + observationMs };
    }
    if (!this.pending) return { state: 'none' as const };
    const pending = this.pending;
    assert.equal(value.id, pending.comparisonId);
    assert.equal(value.manifestHash, pending.manifestHash);
    assert.ok(now < pending.deadline, 'Sealed finalization did not produce a durable result within 90 seconds');
    const cell = value.cells.find((candidate: any) => candidate.id === pending.cellId);
    assert.ok(cell && cell.runId === pending.runId, 'The observed finalization cell disappeared or changed run');
    const { deadline: _deadline, ...identity } = pending;
    assert.deepEqual(sealedIdentity(value, cell), identity, 'The accepted finalization identity changed while observing');
    if (cell.result) {
      assert.match(cell.resultHash ?? '', /^[a-f0-9]{64}$/);
      assert.equal(cell.resultHash, digest(canonical(cell.result)), 'Durable assessment result hash differs from its bytes');
      assert.equal(canonical(cell.result.handoff?.source), canonical(pending.source));
      assert.equal(canonical(cell.result.source), canonical(pending.source));
      this.pending = undefined;
      return { state: 'settled' as const, cellId: pending.cellId, deadline: pending.deadline };
    }
    return { state: 'waiting' as const, cellId: pending.cellId, deadline: pending.deadline };
  }
}
