import type { Role, Run } from '../shared/types';
import { RepositoryConfigurationChanged, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import { RuntimeSupervisor } from './runtime/supervisor';

export async function prepareCodingRepository(work: RepositoryWork, evidence: Evidence, runId: string, request: unknown) {
  const prepared = await work.prepare(runId, request);
  evidence.configure(prepared.runId, prepared.repository.checks ?? []);
  return prepared;
}

/** The slot is stored by Workbench, not by this in-memory dispatcher. Signals only wake reconciliation. */
export class CodingScheduler {
  private pending = false;
  private worker?: Promise<void>;
  private closed = false;
  private launching = new Map<string, Promise<unknown>>();
  constructor(private store: Workbench, private work: RepositoryWork, private evidence: Evidence,
    private runtime: RuntimeSupervisor, private roles: () => Role[], private changed: () => void = () => {}) {}

  kick() {
    if (this.closed) return;
    this.pending = true;
    if (!this.worker) {
      this.worker = this.drain().finally(() => { this.worker = undefined; if (this.pending && !this.closed) this.kick(); });
    }
  }

  async idle() { await this.worker; }
  async close() { this.closed = true; await this.worker; }

  private async drain() {
    while (this.pending && !this.closed) {
      this.pending = false;
      try { await this.reconcile(); } catch (error) {
        // A failed launch is recorded on its run; a failed observation is retried on the next signal.
        console.error('Coding schedule reconciliation failed:', error);
      }
      this.changed();
    }
  }

  private async reconcile() {
    const holder = this.store.codingSlot();
    if (holder) {
      const publication = this.store.publication(holder.id);
      if (publication?.merged) { this.store.releaseCodingSlot(holder.id, 'merged'); this.pending = true; return; }
      if (holder.resumeRequest && !this.runtime.busy(holder.id)) {
        await this.runtime.resume(holder.id);
        return;
      }
      if (publication?.ready && !holder.sourceConflict && holder.slotRelease?.candidateId !== publication.candidateId) {
        this.store.releaseCodingSlot(holder.id, 'review', publication.candidateId);
        await this.runtime.pause(holder.id, true, 'review');
        this.pending = true;
        return;
      }
      if (publication?.ready && !holder.sourceConflict && holder.slotRelease?.reason === 'review' && holder.status === 'paused'
        && holder.desiredState === 'paused' && (holder.pauseReason === 'review' || holder.pauseReason === 'validation')
        && holder.slotRelease.candidateId === publication.candidateId) {
        this.store.releaseCodingSlot(holder.id, 'review', publication.candidateId);
        this.pending = true;
        return;
      }
      // A queued or active continuation owns this run until its FIFO lane becomes idle again.
      if (holder.schedule && !this.runtime.busy(holder.id) && !this.launching.has(holder.id)
        && this.store.canReplayInitialLaunch(holder.id, holder.schedule.id, holder.controlRevision ?? 0)) await this.launch(holder);
      return;
    }
    const resume = this.store.snapshot().runs.filter(run => run.resumeRequest && (run.status === 'paused' || run.status === 'attention_required')
      && run.desiredState !== 'cancelled').sort((left, right) => left.resumeRequest!.requestedAt.localeCompare(right.resumeRequest!.requestedAt))[0];
    if (resume) {
      if (this.store.unstoppedCodingRun(resume.id)) return;
      await this.runtime.resume(resume.id);
      return;
    }
    if (this.store.unstoppedCodingRun()) return;

    const rehold = this.store.snapshot().runs.filter(run => {
      const publication = this.store.publication(run.id);
      return run.slotRelease?.reason === 'review' && (run.status === 'paused' || run.status === 'attention_required')
        && run.desiredState === 'paused' && !publication?.merged && !run.feedbackResumeHold?.terminal
        && run.feedbackContinuations?.at(-1)?.state !== 'blocked' && (run.sourceConflict || publication?.reviewInvalidated);
    }).sort((left, right) => Date.parse(left.slotRelease!.at) - Date.parse(right.slotRelease!.at))[0];
    if (rehold) { this.store.reclaimCodingSlot(rehold.id, true); return; }

    // Configuration is checked before a plan owns the slot or creates a worktree. Holds are retried on signals.
    for (const plan of this.store.codingQueue().plans) {
      if (!plan.launch || (plan.launchHold && plan.launchHold.source !== 'preflight')
        || this.store.eligibilityHold({ ...plan, launchHold: undefined })) continue;
      try {
        this.runtime.preflightStart(plan.id, { team: plan.launch.team }, this.roles());
        this.store.setLaunchHold(plan.id, plan.launch);
      } catch (error) {
        this.store.setLaunchHold(plan.id, plan.launch, error instanceof Error ? error.message : 'Selected team is unavailable.');
      }
    }
    // Verify ancestry before claiming the slot, so a held plan never prepares a worktree.
    for (const plan of this.store.codingQueue().plans) {
      const merges = JSON.stringify(this.store.dependencyMerges(plan));
      if (merges === '[]' || (!plan.launch && !plan.dependencyBaseHold)) continue;
      if (this.store.eligibilityHold(plan) && this.store.eligibilityHold(plan) !== plan.dependencyBaseHold?.reason) continue;
      const reason = await this.work.dependencyBaseHold(plan.id);
      if (plan.dependencyBaseHold?.merges !== merges || plan.dependencyBaseHold.reason !== reason)
        this.store.saveDependencyBaseHold(plan.id, merges, reason);
    }
    // The existing attempt-claim executionConflict remains the final backstop after this admission gate.
    const decided = this.store.claimNextCodingSlot();
    if (decided) await this.launch(decided);
  }

  /** Share one launch, including its worktree preparation, with an operator start in flight. */
  launch(run: Run, manualInput?: unknown): Promise<unknown> {
    const existing = this.launching.get(run.id);
    if (existing) return existing;
    const next = this.performLaunch(run, manualInput);
    this.launching.set(run.id, next);
    return next.finally(() => { if (this.launching.get(run.id) === next) this.launching.delete(run.id); });
  }

  private async performLaunch(run: Run, manualInput?: unknown): Promise<unknown> {
    const scheduled = manualInput === undefined;
    const decisionId = run.schedule?.id;
    const controlRevision = run.controlRevision ?? 0;
    let dispatched = false;
    try {
      if (scheduled && (!decisionId || !this.store.canReplayInitialLaunch(run.id, decisionId, controlRevision) || this.runtime.busy(run.id))) return;
      if (run.launch) {
        try { await prepareCodingRepository(this.work, this.evidence, run.id, run.launch.repository); }
        catch (error) {
          if (!(error instanceof RepositoryConfigurationChanged)) throw error;
          // The first preflight used a superseded configuration; re-read and rerun Git once.
          await prepareCodingRepository(this.work, this.evidence, run.id, run.launch.repository);
        }
      }
      if (scheduled) {
        if (!this.store.canReplayInitialLaunch(run.id, decisionId!, controlRevision) || this.runtime.busy(run.id)) return;
        dispatched = true;
        return await this.runtime.startScheduled(run.id, decisionId!, controlRevision, { team: run.launch!.team }, this.roles());
      }
      const current = this.store.getRun(run.id);
      if (current.desiredState === 'cancelled' || current.desiredState === 'paused' || current.status !== 'awaiting_runner'
        || this.store.codingSlot()?.id !== run.id) return;
      dispatched = true;
      return await this.runtime.start(run.id, manualInput, this.roles());
    } catch (error) {
      if (scheduled && !dispatched && !this.store.canReplayInitialLaunch(run.id, decisionId!, controlRevision)) return;
      if (!this.store.repositoryWork(run.id) && this.store.dependencyMerges(this.store.getRun(run.id)).length) {
        const reason = await this.work.dependencyBaseHold(run.id);
        if (reason && this.store.codingSlot()?.id === run.id) {
          this.store.deferDependencyLaunch(run.id, reason);
          return;
        }
      }
      const current = this.store.getRun(run.id);
      const reason = error instanceof RepositoryConfigurationChanged
        ? 'Repository configuration changed again during preparation. Inspect the current configuration and Save launch details to retry.'
        : !dispatched && run.launch ? this.work.preparationHoldReason(error)
          : error instanceof Error ? error.message : 'Coding launch failed.';
      if (current.desiredState !== 'cancelled' && current.desiredState !== 'paused' && !this.store.repositoryWork(run.id)
        && !this.store.runtime(run.id).assignments.length && this.store.codingSlot()?.id === run.id) {
        this.store.deferUnadmittedLaunch(run.id, reason);
        if (manualInput === undefined) return;
        throw error;
      }
      if (current.desiredState !== 'cancelled' && (current.desiredState !== 'paused' || current.pauseReason !== 'operator'))
        this.store.requireAttention(run.id, reason);
      throw error;
    }
  }
}
