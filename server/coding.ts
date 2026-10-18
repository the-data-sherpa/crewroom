import { createHash, randomUUID } from 'node:crypto';
import { z } from 'zod';
import { Conflict, NotFound, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import type { CompletionOperation, ImplementationClaim } from '../shared/evidence';

const editSchema = z.object({ path: z.string().min(1).max(1000), expectedHash: z.string().regex(/^[a-f0-9]{64}$/).nullable(), content: z.string().max(1024 * 1024) }).strict();
const patchSchema = z.object({ path: z.string().min(1).max(1000), expectedHash: z.string().regex(/^[a-f0-9]{64}$/),
  replacements: z.array(z.object({ oldText: z.string().min(1).max(100000), newText: z.string().max(100000) }).strict()).min(1).max(20) }).strict();
export class Coding {
  constructor(private store: Workbench, private work: RepositoryWork, private evidence?: Evidence) {}
  async claim(coordinatorAttemptId: string, toolCallId: string, input: unknown) {
    const value = z.object({ assignee: z.string().min(1), instruction: z.string().trim().min(1).max(40000) }).strict().parse(input);
    z.string().min(1).max(200).parse(toolCallId);
    const coordinator = this.store.getAttempt(coordinatorAttemptId); const run = this.store.getRun(coordinator.runId);
    const team = this.store.runtime(run.id).assignments;
    if (coordinator.ownership !== 'held' || run.status !== 'running' || run.desiredState !== 'running' || !team.some(item => item.coordinator && item.currentAttemptId === coordinatorAttemptId)) throw new Conflict('Only the active coordinator may assign implementation.');
    const writer = team.find(item => item.name === value.assignee && !item.coordinator && !item.retiredAt);
    const record = this.store.codingRecord(run.id);
    if (!writer || !record) throw new Conflict('Specialist or coding policy is unavailable.');
    const unresolved = this.work.unresolvedConflict(run.id);
    const conflict = this.work.activeConflict(run.id);
    if (unresolved && !conflict) throw new Conflict('Target conflict requires an operator decision before implementation.');
    if (conflict?.resolutionWorkspace?.state === 'ready') throw new Conflict('Conflict rebase is ready; submit completion instead of assigning another writer.');
    const candidate = record.candidates.at(-1);
    const adoptedScope = !!candidate && candidate.revision.requirementsHash !== this.store.repositoryWork(run.id)?.requirementsHash;
    const publication = this.store.publication(run.id);
    const feedback = run.feedbackContinuations?.findLast(item => item.publicationId === publication?.id && item.prId === publication?.pr.id
      && item.branch === publication?.branch && (['starting', 'handed_off'].includes(item.state)
        && item.sourceCandidateId === candidate?.id && item.sourceHead === candidate?.revision.head
        || item.state === 'candidate' && item.candidateId === candidate?.id && candidate?.feedbackFrom?.continuationId === item.id
          && item.sourceCandidateId === publication?.candidateId));
    const feedbackAllowed = !!feedback && !conflict && !!publication && !publication.merged && publication.pr.providerState === 'OPEN'
      && publication.pr.state === 'confirmed' && publication.push.state === 'confirmed';
    const firstFeedback = feedbackAllowed && candidate?.id === publication?.candidateId;
    const previous = record.claims?.find(item => item.coordinatorAttemptId === coordinatorAttemptId && item.toolCallId === toolCallId);
    if (previous) {
      const priorRefresh = previous.targetRefreshId ? this.store.repositoryWork(run.id)?.refreshes?.find(item => item.requestId === previous.targetRefreshId) : undefined;
      const sameScope = previous.targetRefreshId === conflict?.requestId || (!conflict && !!previous.targetRefreshId && priorRefresh?.state === 'completed');
      if (this.store.getAttempt(previous.writerAttemptId).assignmentId !== writer.id || previous.instruction !== value.instruction || !sameScope
        || previous.feedbackContinuationId !== (feedbackAllowed ? feedback?.id : undefined)) throw new Conflict('Implementation claim has different content or workspace scope.');
      return previous;
    }
    if (this.store.repositoryWork(run.id)?.writer) throw new Conflict('Another implementation claim still owns repository writes.');
    if (record.completions?.some(item => item.state === 'requested')) throw new Conflict('Reconcile pending implementation completion first.');
    if (candidate) {
      const failedCheck = record.requiredChecks.filter(item => item.required).some(check => {
        const result = record.checks.findLast(item => item.candidateId === candidate.id && item.checkId === check.id);
        return result && (!result.started || !result.completed || !result.sourceIntact || result.exitCode !== 0 || result.timedOut);
      });
      const reviews = new Map(record.reviews.filter(item => item.candidateId === candidate.id).map(item => [item.assignmentId, item]));
      if (!conflict && !adoptedScope && !failedCheck && ![...reviews.values()].some(review => review.verdict === 'changes_requested') && !firstFeedback)
        throw new Conflict('Complete validation of the current revision before assigning automatic rework.');
    }
    const unfinished = record.claims?.findLast(item => item.candidateId === candidate?.id && this.store.getAttempt(item.writerAttemptId).assignmentId === writer.id && !record.completions?.some(completion => completion.attemptId === item.writerAttemptId && completion.state !== 'requested'));
    const claim: ImplementationClaim = { id: randomUUID(), coordinatorAttemptId, toolCallId, writerAttemptId: writer.currentAttemptId, instruction: value.instruction,
      candidateId: candidate?.id, automatic: !firstFeedback && !conflict && !!candidate && !unfinished && !adoptedScope,
      ...(feedbackAllowed ? { feedbackContinuationId: feedback.id } : {}), ...(conflict ? { targetRefreshId: conflict.requestId } : {}) };
    await this.work.claimWriter(writer.currentAttemptId, claim);
    return claim;
  }

  async reconcileStoppedEdits(runId: string) {
    const edits = this.store.codingRecord(runId)?.edits?.filter(item => item.state === 'pending') ?? [];
    if (edits.some(item => this.store.getAttempt(item.attemptId).ownership !== 'stopped')) throw new Conflict('Confirm stopped writer ownership before reconciling an edit.');
    for (const edit of edits) {
      let current: string | null;
      try { current = (await this.work.readFile(runId, edit.input.path)).sha256; } catch (error) { if (!(error instanceof NotFound)) throw error; current = null; }
      this.store.updateCodingRecord(runId, record => ({ ...record!, edits: record!.edits!.map(item => item.id !== edit.id ? item : current === edit.afterHash
        ? { ...item, state: 'applied' }
        : { ...item, state: 'rejected', reason: 'Interrupted edit was not confirmed in the retained content. Read the current file before retrying.' }) }));
    }
  }

  async restoreWriter(runId: string) {
    const record = this.store.codingRecord(runId);
    const claim = record?.claims?.at(-1);
    if (!record || !claim) return;
    const unresolved = this.work.unresolvedConflict(runId);
    if ((unresolved && (!this.work.activeConflict(runId) || claim.targetRefreshId !== unresolved.requestId)) || (!unresolved && claim.targetRefreshId)) return;
    if (claim.candidateId !== record.candidates.at(-1)?.id || record.completions?.some(item => item.attemptId === claim.writerAttemptId)) return;
    const prior = this.store.getAttempt(claim.writerAttemptId);
    const team = this.store.runtime(runId).assignments;
    const writer = team.find(item => item.id === prior.assignmentId && !item.retiredAt);
    if (writer?.rebriefRequired) return; // No old writer grant is restored before a newly briefed assignment exists.
    const coordinator = team.find(item => item.coordinator)!;
    if (!writer || prior.ownership !== 'stopped') throw new Conflict('Interrupted implementation ownership is unresolved.');
    if (writer.currentAttemptId === prior.id) return;
    // Continue the retained grant before delivering unfinished input. Recovery already charged its cycle.
    await this.work.claimWriter(writer.currentAttemptId, { ...claim, id: randomUUID(), writerAttemptId: writer.currentAttemptId,
      coordinatorAttemptId: coordinator.currentAttemptId, toolCallId: `restore:${writer.currentAttemptId}`, automatic: false, restoredFrom: prior.id });
  }

  async continueTargetConflict(attemptId: string, toolCallId: string, input: unknown) {
    const { requestId } = z.object({ requestId: z.string().uuid() }).strict().parse(input);
    z.string().min(1).max(200).parse(toolCallId);
    return this.work.continueConflict(attemptId, requestId, toolCallId);
  }

  async requestCompletion(attemptId: string, toolCallId: string, input: unknown) {
    const { summary } = z.object({ summary: z.string().trim().min(1).max(2000) }).strict().parse(input);
    z.string().min(1).max(200).parse(toolCallId);
    const attempt = this.store.getAttempt(attemptId);
    const existing = () => {
      const record = this.store.codingRecord(attempt.runId); if (!record) throw new NotFound('Coding evidence is not configured.');
      const previous = record.completions?.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
      if (previous && previous.summary !== summary) throw new Conflict('Completion identity has different content.');
      return { record, previous };
    };
    const before = existing().previous; if (before) return before;
    const unresolved = this.work.unresolvedConflict(attempt.runId);
    const conflict = this.work.activeConflict(attempt.runId);
    if (unresolved && !conflict) throw new Conflict('Target conflict requires an operator decision before completion.');
    if (conflict && conflict.resolutionWorkspace?.state !== 'ready') throw new Conflict('Continue the isolated target rebase until it reports ready before submitting completion.');
    const conflictHash = conflict ? await this.work.conflictFilesHash(attempt.runId) : undefined;
    const work = await this.work.status(attempt.runId);
    // Ownership and coding state are reread after Git; the checks and the durable request are one synchronous step.
    const { record, previous } = existing(); if (previous) return previous;
    const current = this.store.getAttempt(attemptId); const owned = this.store.repositoryWork(attempt.runId); const run = this.store.getRun(attempt.runId);
    if (owned?.writer?.attemptId !== attemptId || work.writer?.attemptId !== attemptId || current.ownership !== 'held' || run.status !== 'running' || run.desiredState !== 'running') throw new Conflict('Only the current writer may request completion.');
    if (record.edits?.some(item => item.state === 'pending') || record.completions?.some(item => item.state === 'requested')) throw new Conflict('Reconcile pending edits or completion first.');
    const completion: CompletionOperation = { id: randomUUID(), attemptId, toolCallId, summary, head: work.revision.head, worktreeHash: conflictHash ?? work.revision.worktreeHash,
      state: 'requested', ...(conflict ? { conflictRefreshId: conflict.requestId } : {}) };
    this.store.updateCodingRecord(attempt.runId, record => ({ ...record!, completions: [...(record!.completions ?? []), completion] })); return completion;
  }
  async complete(runId: string, id: string) {
    const record = this.store.codingRecord(runId); const completion = record?.completions?.find(item => item.id === id);
    if (!record || !completion || !this.evidence) throw new NotFound('Completion is unavailable.');
    if (completion.state === 'completed') return record.candidates.find(item => item.id === completion.candidateId)!;
    if (completion.state === 'rejected') throw new Conflict(`Completion was rejected: ${completion.rejection?.reason ?? 'the target changed'}. Request an operator retry; this completion cannot be replayed.`);
    await this.work.commit(runId, completion);
    const currentCompletion = this.store.codingRecord(runId)?.completions?.find(item => item.id === id);
    if (currentCompletion?.state === 'rejected') throw new Conflict(`Completion was rejected: ${currentCompletion.rejection?.reason}. Request an operator retry; this completion cannot be replayed.`);
    if (currentCompletion?.state !== 'requested') throw new Conflict('Completion changed while committing.');
    const refresh = completion.conflictRefreshId ? this.store.repositoryWork(runId)?.refreshes?.find(item => item.requestId === completion.conflictRefreshId) : undefined;
    if (completion.conflictRefreshId && (!refresh || refresh.state !== 'completed')) throw new Conflict('The target conflict was not promoted.');
    const sources = refresh ? refresh.sourceCandidateId ? record.candidates.filter(item => item.id === refresh.sourceCandidateId)
      : record.candidates.filter(item => item.revision.head === refresh.fromHead && item.revision.base === refresh.fromBase) : [];
    if (refresh && (sources.length !== 1 || sources[0].revision.head !== refresh.fromHead || sources[0].revision.base !== refresh.fromBase))
      throw new Conflict('The source candidate for this target refresh is missing or ambiguous; operator recovery is required.');
    const feedbackClaim = record.claims?.findLast(item => item.writerAttemptId === completion.attemptId && !!item.feedbackContinuationId);
    const feedback = this.store.getRun(runId).feedbackContinuations?.find(item => item.id === feedbackClaim?.feedbackContinuationId);
    const feedbackSource = feedback && record.candidates.find(item => item.id === feedback.sourceCandidateId);
    const latest = record.candidates.at(-1);
    if (feedbackClaim && (!feedback || !feedbackSource || feedbackSource.revision.head !== feedback.sourceHead
      || latest?.id !== feedbackSource.id && latest?.feedbackFrom?.continuationId !== feedback.id))
      throw new Conflict('The feedback source candidate changed before completion.');
    const prior = sources[0] ?? (feedback ? latest : undefined);
    const authors = prior ? [...new Set([...prior.implementationAttempts, completion.attemptId])]
      : [...new Set([...(record.edits ?? []).filter(edit => edit.state === 'applied').map(edit => edit.attemptId), completion.attemptId])];
    const current = await this.work.status(runId);
    if (this.store.codingRecord(runId)?.completions?.find(item => item.id === id)?.state !== 'requested') throw new Conflict('Completion changed before candidate registration.');
    const existing = record.candidates.find(item => !!feedback && item.feedbackFrom?.continuationId === feedback.id
      && item.implementationAttempts.includes(completion.attemptId) && JSON.stringify(item.revision) === JSON.stringify(current.revision))
      ?? (prior && record.candidates.find(item => item.refreshedFrom === prior.id
        && JSON.stringify(item.revision) === JSON.stringify(current.revision) && JSON.stringify(item.implementationAttempts) === JSON.stringify(authors)));
    const unchangedFeedback = !!feedback && current.revision.head === prior?.revision.head && !existing;
    const candidate = existing ?? (unchangedFeedback ? prior! : await this.evidence.registerCandidate(runId, authors, prior && refresh ? prior.id : undefined,
      feedback && prior ? { continuationId: feedback.id, sourceCandidateId: prior.id } : undefined));
    if (feedback) this.store.finishFeedbackContinuation(runId, feedback.id,
      unchangedFeedback && prior?.id === feedback.sourceCandidateId ? { noChange: true } : { candidateId: candidate.id });
    if (refresh) {
      const saved = this.store.finishTargetRefresh(runId, refresh.requestId, { state: 'completed', candidateId: candidate.id }).refreshes!.find(item => item.requestId === refresh.requestId)!;
      this.store.noticeDelivery(runId, { kind: 'target_refresh', subjectId: saved.requestId, fromBase: saved.fromBase, toBase: saved.toBase, candidateId: candidate.id,
        head: candidate.revision.head, instruction: `The target conflict was resolved on the owned branch as candidate ${candidate.id}. Run its required checks and independent review before publishing it.` });
    }
    this.store.updateCodingRecord(runId, record => ({ ...record!, completions: record!.completions!.map(item => item.id === id ? { ...item, state: 'completed', candidateId: candidate.id } : item) }));
    return candidate;
  }

  /**
   * Replace exact, unique text spans in one file read at `expectedHash`. The result enters the same
   * writer-owned, hash-checked edit ledger as a whole-file write; a replay returns the recorded edit.
   */
  async patch(attemptId: string, toolCallId: string, input: unknown) {
    const value = patchSchema.parse(input); z.string().min(1).max(200).parse(toolCallId);
    const request = createHash('sha256').update(JSON.stringify(value)).digest('hex');
    const attempt = this.store.getAttempt(attemptId);
    const prior = this.store.codingRecord(attempt.runId)?.edits?.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (prior) {
      if (prior.request !== request) throw new Conflict('Edit operation has different content.');
      return this.edit(attemptId, toolCallId, prior.input, request);
    }
    const current = await this.work.readFile(attempt.runId, value.path);
    if (current.sha256 !== value.expectedHash) throw new Conflict('File changed since it was read.');
    let content = current.content;
    for (const [index, { oldText, newText }] of value.replacements.entries()) {
      const first = content.indexOf(oldText);
      if (first < 0 || content.indexOf(oldText, first + 1) >= 0) throw new Conflict(`Replacement ${index + 1} must match exactly one span of the current text; it matched ${first < 0 ? 'none' : 'more than one'}.`);
      content = content.slice(0, first) + newText + content.slice(first + oldText.length);
    }
    return this.edit(attemptId, toolCallId, { path: value.path, expectedHash: value.expectedHash, content }, request);
  }

  async edit(attemptId: string, toolCallId: string, input: unknown, request?: string) {
    const value = editSchema.parse(input); z.string().min(1).max(200).parse(toolCallId);
    const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
    const owned = this.store.repositoryWork(run.id);
    if (owned?.writer?.attemptId !== attemptId || attempt.ownership !== 'held' || !['idle', 'executing'].includes(attempt.state) || run.status !== 'running' || run.desiredState !== 'running'
      || !this.store.runtime(run.id).assignments.some(item => item.currentAttemptId === attemptId)) throw new Conflict('Only the current writer may edit repository files.');
    const record = this.store.codingRecord(run.id);
    if (record?.completions?.some(item => item.state === 'requested' && item.attemptId === attemptId)) throw new Conflict('Completion is pending; further edits are closed.');
    if (!record) throw new Conflict('Configure coding evidence before editing.');
    let edit = record.edits?.find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (edit && (JSON.stringify(edit.input) !== JSON.stringify(value) || edit.request !== request)) throw new Conflict('Edit operation has different content.');
    if (!edit) {
      edit = { id: randomUUID(), attemptId, toolCallId, input: value, afterHash: createHash('sha256').update(value.content).digest('hex'), state: 'pending', ...(request ? { request } : {}) };
      this.store.updateCodingRecord(run.id, record => ({ ...record!, edits: [...(record!.edits ?? []), edit!] }));
    }
    if (edit.state === 'rejected') throw new Conflict(edit.reason ?? 'Edit was rejected.');
    try {
    if (edit.state !== 'applied') {
      let current: string | null;
      try { current = (await this.work.readFile(run.id, value.path)).sha256; } catch (error) { if (!(error instanceof NotFound)) throw error; current = null; }
      if (current !== edit.afterHash) await this.work.writeFile(attemptId, value);
      const id = edit.id;
      this.store.updateCodingRecord(run.id, record => ({ ...record!, edits: record!.edits!.map(item => item.id === id ? { ...item, state: 'applied' } : item) }));
    }
    } catch (error) {
      if (error instanceof Conflict || error instanceof NotFound) {
        const id = edit.id;
        this.store.updateCodingRecord(run.id, record => ({ ...record!, edits: record!.edits!.map(item => item.id === id ? { ...item, state: 'rejected', reason: error.message } : item) }));
      }
      throw error;
    }
    return { operationId: edit.id, path: value.path, sha256: edit.afterHash };
  }
}
