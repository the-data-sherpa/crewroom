import { z } from 'zod';
import { Conflict, NotFound, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import type { Publication } from './publication';
import type { TargetRefresh as RefreshRecord } from '../shared/repository';

/**
 * Operator-initiated verification of the current target for a stopped run. A moved target rebases the owned
 * branch and requires a new candidate's checks and review; an unchanged target can complete a reasoned hold
 * retry without changing Git, the existing candidate, ownership or publication readiness.
 */
export class TargetRefresh {
  constructor(private store: Workbench, private work: RepositoryWork, private evidence: Evidence, private publication?: Publication) {}
  private static lanes = new WeakMap<Workbench, Map<string, Promise<void>>>();

  async refresh(runId: string, input: unknown) {
    let lanes = TargetRefresh.lanes.get(this.store);
    if (!lanes) { lanes = new Map(); TargetRefresh.lanes.set(this.store, lanes); }
    const previous = lanes.get(runId);
    let release!: () => void;
    const lane = new Promise<void>(resolve => { release = resolve; });
    lanes.set(runId, lane);
    try { if (previous) await previous; return await this.refreshSerialized(runId, input); }
    finally { if (lanes.get(runId) === lane) lanes.delete(runId); release(); }
  }

  private async refreshSerialized(runId: string, input: unknown) {
    const { requestId, operatorDecision } = z.object({ requestId: z.string().uuid(),
      operatorDecision: z.object({ conflictRequestId: z.string().uuid(), reason: z.string().trim().min(1).max(4000) }).strict().optional() }).strict().parse(input);
    const owned = this.store.repositoryWork(runId); if (!owned) throw new NotFound('Repository work has not been prepared.');
    const record = this.store.codingRecord(runId);
    if (!record?.candidates.length) throw new Conflict('Refreshing from the target needs a recorded candidate.');
    if (owned.refreshes?.some(item => item.state === 'conflicted' && ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')))
      throw new Conflict('Retained Git mutation ownership is unverified; inspect every workspace before retrying the target.');
    // A recorded request is historical: replay never admits it against a newer hold or fetches the target again.
    let replay = owned.refreshes?.find(item => item.requestId === requestId);
    if (replay?.state === 'rebasing' || replay?.resolutionWorkspace?.state === 'promoting'
      || (replay?.resolutionWorkspace?.state === 'ready' && this.store.codingRecord(runId)?.completions?.some(item => item.state === 'requested' && item.conflictRefreshId === requestId))) {
      await this.work.reconcileRefresh(runId);
      replay = this.store.repositoryWork(runId)!.refreshes?.find(item => item.requestId === requestId);
    }
    if (replay) {
      const decision = operatorDecision ? { requestId: operatorDecision.conflictRequestId, reason: operatorDecision.reason } : undefined;
      if (JSON.stringify(replay.supersedes) !== JSON.stringify(decision)) throw new Conflict('This refresh request has a different operator decision.');
      if (['preparing', 'continuing'].includes(replay.resolutionWorkspace?.state ?? ''))
        throw new Conflict('Git mutation ownership is unverified; retain the target workspace for operator inspection.');
      if (replay.state === 'conflicted') return { changed: false as const, refresh: replay };
      if (replay.state !== 'completed') throw new Conflict('This refresh request was interrupted. Start a new refresh.');
      return this.completeCandidate(runId, replay);
    }
    await this.work.reconcileRefresh(runId);
    const refreshes = this.store.repositoryWork(runId)!.refreshes ?? [];
    if (refreshes.some(item => item.state === 'rebasing')) throw new Conflict('Reconcile the existing target refresh before starting another.');
    const unresolved = refreshes.findLast(item => item.state === 'conflicted' && !item.supersededBy);
    if (unresolved) {
      if ((unresolved.resolutionWorkspace && unresolved.resolutionWorkspace.state !== 'operator_required')
        || operatorDecision?.conflictRequestId !== unresolved.requestId) throw new Conflict('An operator reason is required to retry this held target conflict.');
      if (this.store.codingRecord(runId)?.completions?.some(item => item.conflictRefreshId === unresolved.requestId && item.state === 'requested')) throw new Conflict('Reconcile the held conflict completion before retrying the target.');
    } else if (operatorDecision) throw new Conflict('The operator retry no longer matches an active target hold.');
    // Finish an older recorded Git transition as recovery, then process this distinct operator request.
    const pending = refreshes.find(item => item.state === 'completed' && !item.candidateId);
    if (pending) await this.completeCandidate(runId, pending);
    const previous = this.store.codingRecord(runId)!.candidates.at(-1)!;
    const current = this.store.preflightTargetRefresh(runId, operatorDecision ? { requestId: operatorDecision.conflictRequestId, reason: operatorDecision.reason } : undefined);
    // Every fresh request stages into its own ref. Replay above never reaches the transport.
    if (!this.publication && ((current.continuationTargetRef ?? current.targetRef).startsWith('refs/crewroom/') || current.refreshes?.some(item => item.targetRef.startsWith('refs/crewroom/'))))
      throw new Conflict('The configured remote target is unavailable; a retained snapshot is not the live target.');
    const targetRef = await this.publication?.fetchTarget(runId, requestId) ?? current.continuationTargetRef ?? current.targetRef;
    const refresh = (await this.work.refreshBase(runId, requestId, targetRef, previous.id,
      operatorDecision ? { requestId: operatorDecision.conflictRequestId, reason: operatorDecision.reason } : undefined)).refresh;
    if (refresh.state === 'conflicted') return { changed: false as const, refresh };
    return this.completeCandidate(runId, refresh);
  }

  private async completeCandidate(runId: string, refresh: RefreshRecord) {
    const unchanged = refresh.fromBase === refresh.toBase && refresh.fromHead === refresh.toHead;
    if (refresh.candidateId) return { changed: !unchanged, refresh, candidateId: refresh.candidateId };
    const record = this.store.codingRecord(runId)!;
    if (refresh.resolutionWorkspace && record.completions?.some(item => item.conflictRefreshId === refresh.requestId && item.state === 'requested'))
      throw new Conflict('Finish the retained conflict completion before refreshing again.');
    const status = await this.work.status(runId);
    if (status.base !== refresh.toBase || status.revision.head !== refresh.toHead || status.writer || status.revision.dirty
      || status.drift.head || status.drift.requirements || status.drift.instructions)
      throw new Conflict('The recorded refresh no longer matches the owned candidate; reconcile recovery before retrying.');
    const matchingSources = refresh.sourceCandidateId ? record.candidates.filter(item => item.id === refresh.sourceCandidateId)
      : record.candidates.filter(item => item.revision.head === refresh.fromHead && item.revision.base === refresh.fromBase);
    if (matchingSources.length !== 1 || matchingSources[0].revision.head !== refresh.fromHead
      || matchingSources[0].revision.base !== refresh.fromBase)
      throw new Conflict('The recorded refresh source candidate is missing or ambiguous; operator recovery is required.');
    const source = matchingSources[0];
    const completion = refresh.resolutionWorkspace?.operationId && record.completions?.find(item => item.id === refresh.resolutionWorkspace!.operationId);
    const authors = completion ? [...new Set([...source.implementationAttempts, completion.attemptId])] : source.implementationAttempts;
    // A crash after candidate registration can leave only the receipt association missing. Historical
    // shared-ref records may observe later drift; recover only the matching candidate, never create another.
    const existing = record.candidates.at(-1);
    const recordedCandidate = existing && JSON.stringify(existing.revision) === JSON.stringify({ ...status.revision, base: refresh.toBase })
      && JSON.stringify(existing.implementationAttempts) === JSON.stringify(authors)
      && (unchanged ? existing.id === source.id : existing.refreshedFrom === source.id) ? existing : undefined;
    if (unchanged && !recordedCandidate) throw new Conflict('The unchanged target no longer matches its recorded candidate; reconcile recovery before retrying.');
    const candidate = recordedCandidate ?? await this.evidence.registerCandidate(runId, authors, source.id);
    const saved = this.store.finishTargetRefresh(runId, refresh.requestId, { state: 'completed', candidateId: candidate.id }).refreshes!.find(item => item.requestId === refresh.requestId)!;
    if (!unchanged) this.store.noticeDelivery(runId, { kind: 'target_refresh', subjectId: saved.requestId, fromBase: saved.fromBase, toBase: saved.toBase, candidateId: candidate.id,
      head: candidate.revision.head, instruction: `The target moved. The owned branch was rebased onto it as candidate ${candidate.id}. Run its required checks and independent review before publishing it; earlier evidence does not cover it.` });
    return { changed: !unchanged, refresh: saved, candidateId: candidate.id };
  }

  /** Startup: undo or finish refreshes interrupted by a crash. An unreconciled refresh keeps blocking execution. */
  async reconcile() {
    for (const run of this.store.snapshot().runs) {
      if (!this.store.repositoryWork(run.id)?.refreshes?.some(item => item.state === 'rebasing'
        || (item.state === 'conflicted' && (['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')
          || (!item.supersededBy && (item.resolutionWorkspace?.state === 'promoting'
            || (item.resolutionWorkspace?.state === 'ready' && this.store.codingRecord(run.id)?.completions?.some(completion => completion.state === 'requested' && completion.conflictRefreshId === item.requestId))))))
        || (item.state === 'completed' && item.resolutionWorkspace?.state === 'promoted'))) continue;
      try { await this.work.reconcileRefresh(run.id); }
      catch {
        const current = this.store.getRun(run.id);
        if (!['cancelled', 'completed', 'closed'].includes(current.status) && current.desiredState !== 'cancelled')
          this.store.requireAttention(run.id, 'Git mutation ownership or target recovery is unverified; retain source and workspace for operator inspection before any retry.');
      }
    }
  }
}
