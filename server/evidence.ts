import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync, lstatSync } from 'node:fs';
import { join } from 'node:path';
import { z } from 'zod';
import { Conflict, NotFound, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import type { Candidate, CheckDefinition, CheckEvidence, CodingRecord, EvidenceArtifact, ReviewEvidence, ReviewAssignment } from '../shared/evidence';
import type { RepositoryStatus } from '../shared/repository';

const digest = (value: Buffer | string) => createHash('sha256').update(value).digest('hex');
import { checkSchema } from './check-policy';
export { checkSchema } from './check-policy';
const reviewSchema = z.object({ candidateId: z.string().uuid(), verdict: z.enum(['pass', 'changes_requested']), summary: z.string().trim().min(1).max(12000),
  findings: z.array(z.object({ path: z.string().max(1000).optional(), line: z.number().int().positive().optional(), severity: z.enum(['blocker', 'major', 'minor']), body: z.string().trim().min(1).max(4000) }).strict()).max(200),
  coverage: z.array(z.object({ criterion: z.number().int().nonnegative(), assessment: z.string().trim().min(1).max(4000), checkIds: z.array(z.string()).max(100) }).strict()).max(100) }).strict();

export interface EvidenceAssessment { ready: boolean; candidate: Candidate | undefined; blockers: string[]; confirmedInvalidation: boolean }

/** Only the controller records command results; agent review is a separate, attributed claim. */
export class Evidence {
  constructor(private store: Workbench, private work: RepositoryWork, private root: string) {}
  private record(runId: string) { const record = this.store.codingRecord(runId); if (!record) throw new NotFound('Coding evidence is not configured.'); return record; }
  checkOutput(runId: string, checkEvidenceId: string) {
    const record = this.record(runId); const check = record.checks.find(item => item.id === checkEvidenceId);
    const candidate = check && record.candidates.find(item => item.id === check.candidateId);
    if (!check || !candidate) throw new NotFound('Check evidence is unavailable for this run.');
    const output = this.readArtifact(check.output);
    return { checkEvidenceId: check.id, checkId: check.checkId, candidateId: candidate.id, revision: candidate.revision,
      exitCode: check.exitCode, output: check.output, outputPreview: output.slice(-16000), truncated: output.length > 16000 };
  }
  configure(runId: string, input: unknown) {
    const checks = z.array(checkSchema).max(100).refine(items => new Set(items.map(item => item.id)).size === items.length, 'Check IDs must be distinct.').parse(input);
    return this.store.updateCodingRecord(runId, previous => {
      if (previous) { if (JSON.stringify(previous.requiredChecks) !== JSON.stringify(checks)) throw new Conflict('Check policy was already accepted for this run.'); return previous; }
      return { runId, requiredChecks: checks, candidates: [], checks: [], reviews: [] };
    });
  }
  /** Operator correction of a check's pinned image or memory limit. Earlier receipts and reviews stay; they no longer satisfy the check. */
  correctCheckEnvironment(runId: string, input: unknown) {
    const image = z.string().regex(/^sha256:[a-f0-9]{64}$/, 'Use a pinned image SHA-256.');
    // The same bounds as an admitted check definition, so a correction always yields a check that can start.
    const memory = z.number().int().min(128).max(8192);
    const value = z.object({ requestId: z.string().trim().min(1).max(200), checkId: z.string().min(1).max(200),
      expectedImage: image, image, expectedMemoryMiB: memory.optional(), memoryMiB: memory.optional(), reason: z.string().trim().min(1).max(4000) }).strict()
      .refine(item => (item.memoryMiB === undefined) === (item.expectedMemoryMiB === undefined), 'A memory change names both the expected and the new limit.').parse(input);
    const result = this.store.updateCodingRecord(runId, previous => {
      if (!previous) throw new NotFound('Coding evidence is not configured.');
      const existing = previous.checkPolicyChanges?.find(item => item.requestId === value.requestId);
      if (existing) {
        if (Object.entries(value).some(([key, entry]) => existing[key as keyof typeof value] !== entry)) throw new Conflict('Environment correction request has different content.');
        return previous;
      }
      const run = this.store.getRun(runId);
      if (run.status !== 'paused' || run.desiredState !== 'paused'
        || this.store.runtime(runId).attempts.some(item => item.state !== 'admitted' && item.ownership !== 'stopped')
        || this.store.repositoryWork(runId)?.writer) throw new Conflict('A confirmed paused run is required before correcting a check environment.');
      if (this.store.publication(runId)) throw new Conflict('A publication already exists; environment correction requires the feedback-continuation workflow.');
      if (previous.checkOperations?.some(item => !['recorded', 'rejected'].includes(item.state))) throw new Conflict('Reconcile outstanding check operations before correcting the environment.');
      const check = previous.requiredChecks.find(item => item.id === value.checkId);
      if (!check || check.image !== value.expectedImage || (value.expectedMemoryMiB !== undefined && (check.memoryMiB ?? 1024) !== value.expectedMemoryMiB)) throw new Conflict('The selected check environment changed. Reload its current image and memory limit.');
      if (check.image === value.image && (value.memoryMiB === undefined || value.memoryMiB === (check.memoryMiB ?? 1024))) throw new Conflict('The correction leaves the check environment unchanged.');
      const correction = { ...value, revision: (previous.checkPolicyChanges?.length ?? 0) + 1, createdAt: new Date().toISOString() };
      return { ...previous, requiredChecks: previous.requiredChecks.map(item => item.id === check.id ? { ...item, image: value.image, ...(value.memoryMiB === undefined ? {} : { memoryMiB: value.memoryMiB }) } : item),
        checkPolicyChanges: [...(previous.checkPolicyChanges ?? []), correction] };
    });
    return result.checkPolicyChanges!.find(item => item.requestId === value.requestId)!;
  }
  artifact(text: string): EvidenceArtifact {
    const bytes = Buffer.from(text); if (bytes.length > 4 * 1024 * 1024) throw new Conflict('Evidence artifact exceeds its limit.');
    mkdirSync(this.root, { recursive: true, mode: 0o700 });
    const artifact = { id: randomUUID(), sha256: digest(bytes), bytes: bytes.length };
    writeFileSync(join(this.root, artifact.id), bytes, { flag: 'wx', mode: 0o600, flush: true }); return artifact;
  }
  readArtifact(artifact: EvidenceArtifact) {
    if (!z.string().uuid().safeParse(artifact.id).success) throw new Conflict('Invalid evidence artifact identity.');
    const path = join(this.root, artifact.id); let bytes: Buffer;
    try { const stat = lstatSync(path); if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 4 * 1024 * 1024) throw new Error('Invalid artifact'); bytes = readFileSync(path); }
    catch { throw new Conflict('Evidence artifact is missing or unavailable.'); }
    if (bytes.length !== artifact.bytes || digest(bytes) !== artifact.sha256) throw new Conflict('Evidence artifact hash changed.');
    return bytes.toString('utf8');
  }
  async registerCandidate(runId: string, implementationAttempts: string[], refreshedFrom?: string,
    feedbackFrom?: { continuationId: string; sourceCandidateId: string }): Promise<Candidate> {
    return this.saveCandidate(runId, implementationAttempts, await this.work.status(runId), refreshedFrom, feedbackFrom);
  }
  private saveCandidate(runId: string, implementationAttempts: string[], current: RepositoryStatus, refreshedFrom?: string,
    feedbackFrom?: { continuationId: string; sourceCandidateId: string }): Candidate {
    const record = this.record(runId);
    if (current.writer || current.revision.dirty || current.drift.base || current.drift.requirements || current.drift.instructions) throw new Conflict('A clean candidate with stable base, requirements and project instructions is required.');
    if (!implementationAttempts.length || new Set(implementationAttempts).size !== implementationAttempts.length) throw new Conflict('Candidate requires its implementation attempt identities.');
    for (const id of implementationAttempts) {
      const attempt = this.store.getAttempt(id);
      if (attempt.runId !== runId || !attempt.sessionId) throw new Conflict('Implementation attempt is outside this run or has no verified session.');
      if (attempt.ownership !== 'stopped') throw new Conflict('Implementation ownership must be stopped before recording a candidate.');
    }
    const previous = record.candidates.at(-1);
    if (previous && JSON.stringify(previous.revision) === JSON.stringify(current.revision) && JSON.stringify(previous.implementationAttempts) === JSON.stringify(implementationAttempts)) return previous;
    if (refreshedFrom && previous?.id !== refreshedFrom) throw new Conflict('A refreshed candidate must follow the candidate it was rebased from.');
    if (feedbackFrom && (refreshedFrom || previous?.id !== feedbackFrom.sourceCandidateId || previous.revision.head === current.revision.head))
      throw new Conflict('A feedback candidate requires a changed head from its recorded source candidate.');
    const candidate: Candidate = { id: randomUUID(), runId, revision: current.revision, implementationAttempts, createdAt: new Date().toISOString(),
      ...(refreshedFrom ? { refreshedFrom } : {}), ...(feedbackFrom ? { feedbackFrom } : {}) };
    this.store.updateCodingRecord(runId, record => ({ ...record!, candidates: [...record!.candidates, candidate] })); return candidate;
  }
  recordCheck(runId: string, value: Omit<CheckEvidence, 'id' | 'completedAt'>, operationId?: string) {
    const record = this.record(runId); const definition = record.requiredChecks.find(item => item.id === value.checkId);
    const candidate = record.candidates.find(item => item.id === value.candidateId);
    const attempt = this.store.getAttempt(value.attemptId);
    if (!candidate || attempt.runId !== runId || !attempt.sessionId || !definition || JSON.stringify(value.definition) !== JSON.stringify(definition)
      || value.environment.image !== definition.image || value.environment.network !== 'none' || !value.environment.platform
      || (value.environment.memoryMiB ?? 1024) !== (definition.memoryMiB ?? 1024)
      || (value.environment.tmpExecutable ?? false) !== (definition.tmpExecutable ?? false) || !Number.isInteger(value.exitCode)) throw new Conflict('Executed check does not match its admitted candidate, environment or command.');
    const operation = operationId ? record.checkOperations?.find(item => item.id === operationId) : undefined;
    if (operationId && (!operation || operation.attemptId !== value.attemptId || operation.candidateId !== value.candidateId || operation.definition.id !== value.checkId)) throw new Conflict('Check receipt does not belong to its prepared operation.');
    const previous = operationId ? record.checks.find(item => item.id === operationId) : undefined;
    if (previous) {
      this.store.updateCodingRecord(runId, record => ({ ...record!, checkOperations: record!.checkOperations!.map(item => item.id === operationId ? { ...item, state: 'recorded', evidenceId: previous.id } : item) }));
      return previous;
    }
    this.readArtifact(value.output); this.readArtifact(value.source);
    const receipt: CheckEvidence = { ...structuredClone(value), id: operationId ?? randomUUID(), completedAt: new Date().toISOString() };
    this.store.updateCodingRecord(runId, record => ({ ...record!, checks: [...record!.checks, receipt], checkOperations: record!.checkOperations?.map(item => item.id === operationId ? { ...item, state: 'recorded', evidenceId: receipt.id } : item) })); return receipt;
  }
  assignReview(coordinatorAttemptId: string, toolCallId: string, input: unknown) {
    const value = z.object({ assignee: z.string().min(1), candidateId: z.string().uuid(), instruction: z.string().trim().min(1).max(40000) }).strict().parse(input);
    z.string().min(1).max(200).parse(toolCallId);
    const attempt = this.store.getAttempt(coordinatorAttemptId); const run = this.store.getRun(attempt.runId); const record = this.record(run.id);
    const team = this.store.runtime(run.id).assignments;
    if (attempt.ownership !== 'held' || run.status !== 'running' || run.desiredState !== 'running' || !team.some(item => item.coordinator && item.currentAttemptId === coordinatorAttemptId)) throw new Conflict('Only the active coordinator may assign review.');
    const reviewer = team.find(item => item.name === value.assignee && !item.coordinator && !item.retiredAt);
    if (!reviewer || !record.candidates.some(candidate => candidate.id === value.candidateId)) throw new Conflict('Review requires a completed candidate and an admitted specialist.');
    if (record.candidates.some(candidate => candidate.implementationAttempts.some(id => this.store.getAttempt(id).assignmentId === reviewer.id))) throw new Conflict('Review must be independent of implementation.');
    const previous = record.reviewAssignments?.find(item => item.coordinatorAttemptId === coordinatorAttemptId && item.toolCallId === toolCallId);
    if (previous) {
      if (previous.assignmentId !== reviewer.id || previous.candidateId !== value.candidateId || previous.instruction !== value.instruction) throw new Conflict('Review assignment has different content.'); return previous;
    }
    const assignment: ReviewAssignment = { id: randomUUID(), coordinatorAttemptId, toolCallId, assignmentId: reviewer.id, candidateId: value.candidateId, instruction: value.instruction };
    this.store.updateCodingRecord(run.id, record => ({ ...record!, reviewAssignments: [...(record!.reviewAssignments ?? []), assignment] })); return assignment;
  }

  review(attemptId: string, input: unknown): ReviewEvidence {
    const value = reviewSchema.parse(input); const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
    const record = this.record(run.id); const candidate = record.candidates.find(item => item.id === value.candidateId);
    const assignment = this.store.runtime(run.id).assignments.find(item => item.currentAttemptId === attemptId);
    if (!candidate || !assignment || !attempt.sessionId) throw new Conflict('Only a current admitted review attempt may submit evidence.');
    const authors = record.candidates.flatMap(item => item.implementationAttempts).map(id => this.store.getAttempt(id));
    if (authors.some(author => author.assignmentId === assignment.id || author.sessionId === attempt.sessionId)) throw new Conflict('Review must be independent of every implementation assignment and session.');
    if (!record.reviewAssignments?.some(item => item.assignmentId === assignment.id && item.candidateId === candidate.id)) throw new Conflict('Review was not assigned for this candidate.');
    if (attempt.ownership !== 'held' || run.status !== 'running') throw new Conflict('Only a current admitted review attempt may submit evidence.');
    if (value.coverage.some(item => item.criterion >= run.taskSnapshot.criteria.length || item.checkIds.some(id => !record.requiredChecks.some(check => check.id === id)))
      || new Set(value.coverage.map(item => item.criterion)).size !== value.coverage.length) throw new Conflict('Review coverage must reference distinct accepted criteria and configured checks.');
    if (value.verdict === 'pass' && (value.coverage.length !== run.taskSnapshot.criteria.length || value.findings.some(item => item.severity !== 'minor'))) throw new Conflict('Passing review must cover every criterion and leave no blocking findings.');
    if (value.verdict === 'pass' && this.checkBlockers(record, candidate).blockers.length) throw new Conflict('Required checks must pass for this exact candidate before review can pass.');
    const body = { ...value, attemptId, assignmentId: assignment.id, sessionId: attempt.sessionId, checkPolicyRevision: record.checkPolicyChanges?.length ?? 0 };
    const receipt: ReviewEvidence = { ...body, id: randomUUID(), artifact: this.artifact(JSON.stringify(body)), createdAt: new Date().toISOString() };
    this.store.updateCodingRecord(run.id, record => ({ ...record!, reviews: [...record!.reviews, receipt] })); return receipt;
  }
  private checkBlockers(record: CodingRecord, candidate: Candidate): { blockers: string[]; confirmedInvalidation: boolean } {
    const blockers: string[] = []; let confirmedInvalidation = false;
    const required = record.requiredChecks.filter(check => check.required);
    if (!required.length) blockers.push('No required repository checks have been configured.');
    for (const check of required) {
      const result = record.checks.findLast(item => item.candidateId === candidate.id && item.checkId === check.id);
      if (!result || !result.started || !result.completed || !result.sourceIntact || result.exitCode !== 0 || result.timedOut) {
        blockers.push(`Required check ${check.id} is missing or failed.`);
        if (result?.started && (result.timedOut || !result.sourceIntact || result.exitCode !== 0)) confirmedInvalidation = true;
        continue;
      }
      if (JSON.stringify(result.definition) !== JSON.stringify(check)) {
        blockers.push(`Required check ${check.id} has a different command policy.`);
        confirmedInvalidation = true;
      }
      try { this.readArtifact(result.output); this.readArtifact(result.source); } catch (error) { blockers.push(`Required check ${check.id}: ${(error as Error).message}`); }
    }
    return { blockers, confirmedInvalidation };
  }

  /** Readiness of the latest candidate. Without an observed status this runs the uncached revision gate. */
  async assess(runId: string, observed?: RepositoryStatus | Error): Promise<EvidenceAssessment> {
    if (!this.record(runId).candidates.length) return { ready: false, candidate: undefined, blockers: ['No candidate revision has been recorded.'], confirmedInvalidation: false };
    let current: RepositoryStatus | Error;
    try { current = observed ?? await this.work.status(runId); } catch (error) { current = error instanceof Error ? error : new Error('Repository revision is unavailable.'); }
    // Checks and reviews can land while Git runs; the decision reads them after the await, with no further yield.
    const record = this.record(runId); const candidate = record.candidates.at(-1)!; const blockers: string[] = [];
    if (!(current instanceof Error) && current.continuationTargetRef) blockers.push('Refresh from the target after recording fresh implementation for this continuation.');
    let confirmedInvalidation = false;
    blockers.push(...this.store.insertionBlockers(runId));
    if (this.store.repositoryWork(runId)?.refreshes?.some(item => item.state === 'rebasing'
      || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '') || (item.state === 'conflicted' && !item.supersededBy))) {
      blockers.push('A target conflict is unresolved; the previous candidate cannot be published.');
      confirmedInvalidation = true;
    }
    if (current instanceof Error) blockers.push(current.message);
    else if (current.writer || current.revision.dirty || JSON.stringify(current.revision) !== JSON.stringify(candidate.revision)) {
      blockers.push('The repository revision or accepted requirements changed.');
      confirmedInvalidation = true;
    }
    const pendingBriefs = this.store.runtime(runId).assignments.filter(item => !item.retiredAt && item.rebriefRequired);
    if (pendingBriefs.length) {
      blockers.push(`Re-brief affected assignments: ${pendingBriefs.map(item => item.name).join(', ')}.`);
      confirmedInvalidation = true;
    }
    const checks = this.checkBlockers(record, candidate);
    blockers.push(...checks.blockers);
    confirmedInvalidation ||= checks.confirmedInvalidation;
    const latest = new Map<string, ReviewEvidence>();
    for (const review of record.reviews.filter(item => item.candidateId === candidate.id)) latest.set(review.assignmentId, review);
    if (!latest.size) blockers.push('Independent review is missing for this revision.');
    for (const review of latest.values()) {
      if ((review.checkPolicyRevision ?? 0) !== (record.checkPolicyChanges?.length ?? 0)) {
        blockers.push('Independent review predates the current check environment.');
        confirmedInvalidation = true;
      }
      if (review.verdict !== 'pass') {
        blockers.push('Independent review requests changes.');
        confirmedInvalidation = true;
      }
      try { this.readArtifact(review.artifact); } catch (error) { blockers.push(`Review: ${(error as Error).message}`); }
    }
    return { ready: blockers.length === 0, candidate, blockers, confirmedInvalidation };
  }
}
