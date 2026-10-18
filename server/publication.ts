import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { realpathSync } from 'node:fs';
import { z } from 'zod';
import { Conflict, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import { BitbucketClient, BitbucketUnavailable, type BitbucketConnection, type BitbucketPullRequest } from './bitbucket-client';
import { PublicationGit } from './publication-git';
import type { HostedPolicy, PublicationRecord } from '../shared/publication';
import type { EvidenceAssessment } from './evidence';
import type { RepositoryWorkRecord } from '../shared/repository';

const names = z.array(z.string().min(1).max(200)).max(100).refine(items => new Set(items).size === items.length);
const policySchema = z.object({ statusKeys: names.min(1), pipelineSteps: names.min(1), optionalSteps: names.default([]) }).strict();
export interface PublicationConnection extends BitbucketConnection { repositoryPath: string; policy: HostedPolicy; fixtureGitPath?: string }
const marker = (record: PublicationRecord) => `Crewroom publication ${record.id}`;
const digest = (text: string) => createHash('sha256').update(text).digest('hex');

/** Owns push/PR intents and revision-bound observations. No merge operation exists. */
export class Publication {
  private flushing?: Promise<void>;
  private closing = false;
  private active = new Map<string, PublicationGit>();
  constructor(private store: Workbench, private work: RepositoryWork, private evidence: Evidence, private root: string, private connections: PublicationConnection[]) {
    this.connections = connections.map(connection => ({ ...connection, repositoryPath: realpathSync(connection.repositoryPath) }));
    for (const connection of this.connections) { new BitbucketClient(connection); policySchema.parse(connection.policy); }
    this.work.setTargetObserver((work) => this.observeTarget(work));
    this.work.setContinuationObserver(runId => this.verifyCancelledContinuation(runId));
  }
  /** Read-only fresh PR identity check at request and again at queued admission. */
  async verifyCancelledContinuation(runId: string) {
    const record = this.store.continuationPublication(runId); if (!record) return;
    if (this.closing) throw new Conflict('Continuation held: publication transport is stopping.');
    const connection = this.connections.find(item => item.repositoryPath === record.repositoryPath && JSON.stringify(item.repository) === JSON.stringify(record.repository));
    if (!connection || !record.pr.id) throw new Conflict('Continuation held: the recorded PR cannot be verified.');
    try {
      const client = new BitbucketClient(connection); await client.verifyRepository();
      const pr = await client.pullRequest(record.pr.id);
      if (pr.state !== 'OPEN' || !pr.description.includes(marker(record)) || pr.source.branch.name !== record.branch
        || pr.destination.branch.name !== record.target || pr.source.repository.uuid !== record.repository.uuid
        || pr.destination.repository.uuid !== record.repository.uuid || pr.source.commit.hash !== record.revision.head)
        throw new Conflict('Continuation held: the PR is closed, merged or its branch identity changed.');
    } catch (error) {
      if (error instanceof Conflict) throw error;
      throw new Conflict('Continuation held: live PR verification is unavailable. Retry when the provider is available.');
    }
  }
  available(runId: string) {
    const owned = this.store.repositoryWork(runId); return !!owned && this.connections.some(item => item.repositoryPath === owned.repository.path);
  }
  async view(runId: string, local?: EvidenceAssessment) {
    const record = await this.read(runId, local); const available = this.available(runId);
    return { available, record, reason: record?.merged ? `Merged into ${record.target} as ${record.merged.commit.slice(0, 12)}. Delivery is complete.`
      : record?.ready ? 'PR ready for human review. Merge remains your decision.'
      : record?.pr.state === 'confirmed' ? 'PR created. Validation is pending or blocked; inspect the recorded evidence.'
      : record ? 'Publication requested. Push and PR receipts are tracked separately.'
      : this.store.getRun(runId).continuedFrom?.publicationId ? 'An existing PR is retained in predecessor history. This run requires fresh validation before updating it.'
      : available ? 'Publication is available after local validation. The coordinator can request it.'
      : 'PR publication is unavailable. Local validation can complete; no PR has been created.' };
  }
  async read(runId: string, local?: EvidenceAssessment) {
    const record = this.store.publication(runId); return record ? { ...record, ...this.assess(record, local ?? await this.localAssessment(runId)) } : undefined;
  }
  /** Comment failures never affect the PR/check observation, publication receipts or readiness latch. */
  async observeFeedback(runId: string) {
    const record = this.store.publication(runId);
    if (!record?.pr.id || record.pr.state !== 'confirmed') return record?.feedback;
    const startedAt = new Date().toISOString();
    const connection = this.connections.find(item => item.repositoryPath === record.repositoryPath && JSON.stringify(item.repository) === JSON.stringify(record.repository));
    const read = connection ? await new BitbucketClient(connection).comments(record.pr.id)
      : { state: 'unavailable' as const, comments: [], reason: 'The configured Bitbucket repository no longer matches this publication.' };
    return this.store.savePublicationFeedback(runId, record.pr.id, read, startedAt);
  }
  private async localAssessment(runId: string) {
    try { return await this.evidence.assess(runId); } catch { return undefined; }
  }
  /** Pin a fresh request's remote target without changing a previously admitted ref. */
  async fetchTarget(runId: string, requestId: string) {
    z.string().uuid().parse(runId); z.string().uuid().parse(requestId);
    const owned = this.store.repositoryWork(runId); const connection = owned && this.connections.find(item => item.repositoryPath === owned.repository.path);
    if (!owned) return undefined;
    if (!connection) {
      if ((owned.continuationTargetRef ?? owned.targetRef).startsWith('refs/crewroom/') || owned.refreshes?.some(item => item.targetRef.startsWith('refs/crewroom/')))
        throw new Conflict('The configured remote target is unavailable. Do not verify a retained snapshot as the live target.');
      return undefined;
    }
    if (owned.refreshes?.some(item => item.requestId === requestId)) throw new Conflict('A recorded target request cannot be fetched again. Replay its pinned receipt.');
    const git = new PublicationGit(join(this.root, `${runId}.target`), connection.fixtureGitPath ? { fixturePath: connection.fixtureGitPath } : connection.repository, [owned.repository.targetBranch, 'main', 'master']);
    const key = `${runId}:target`; this.active.set(key, git);
    const destination = `refs/crewroom/${runId}/refresh/${requestId}/target`;
    try { await git.fetchTarget({ directory: owned.repository.path, branch: owned.repository.targetBranch, destination }); }
    catch (error) { throw new Conflict(error instanceof Error ? error.message : 'The remote target could not be fetched.'); }
    finally { await git.close(); this.active.delete(key); }
    return destination;
  }
  /** Read the configured remote branch itself; no retained snapshot ref is mutated. */
  private async observeTarget(work: RepositoryWorkRecord) {
    const connection = this.connections.find(item => item.repositoryPath === work.repository.path);
    if (!connection) throw new Conflict('The configured remote target is unavailable. Retain the branch for inspection.');
    if (this.closing) throw new Conflict('The remote target observer is stopping.');
    const git = new PublicationGit(join(this.root, `${work.runId}.observe`), connection.fixtureGitPath ? { fixturePath: connection.fixtureGitPath } : connection.repository, [work.repository.targetBranch, 'main', 'master']);
    const key = `${work.runId}:observe:${randomUUID()}`; this.active.set(key, git);
    try { return await git.ref(work.repository.targetBranch); }
    catch { throw new Conflict('The remote target observation is unavailable. Retain the branch for inspection.'); }
    finally { await git.close(); this.active.delete(key); }
  }
  /** Only a recorded target refresh or an operator-pinned feedback continuation links published history. */
  private continuesFrom(runId: string, candidateId: string, published: PublicationRecord) {
    const candidates = this.store.codingRecord(runId)?.candidates ?? [];
    const continuations = this.store.getRun(runId).feedbackContinuations ?? [];
    const visited = new Set<string>();
    for (let current = candidates.find(item => item.id === candidateId); current;) {
      if (visited.has(current.id)) return false;
      visited.add(current.id);
      const feedbackId = current.feedbackFrom?.continuationId;
      const feedback = feedbackId && continuations.find(item => item.id === feedbackId);
      if (current.feedbackFrom && (!feedback || feedback.state !== 'candidate' || !feedback.candidateId || !visited.has(feedback.candidateId)
        || feedback.publicationId !== published.id || feedback.prId !== published.pr.id
        || feedback.repository.uuid !== published.repository.uuid || feedback.branch !== published.branch)) return false;
      const parent = current.refreshedFrom ?? current.feedbackFrom?.sourceCandidateId;
      if (!parent) return false;
      if (feedback && (parent === feedback.sourceCandidateId
        ? candidates.find(item => item.id === parent)?.revision.head !== feedback.sourceHead
        : candidates.find(item => item.id === parent)?.feedbackFrom?.continuationId !== feedback.id)) return false;
      if (parent === published.candidateId) return true;
      current = candidates.find(item => item.id === parent);
    }
    return false;
  }
  async prepare(attemptId: string, input: unknown) {
    const { candidateId, summary } = z.object({ candidateId: z.string().uuid(), summary: z.string().trim().min(40).max(20000) }).strict().parse(input);
    const authorized = () => {
      const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
      const coordinator = this.store.runtime(run.id).assignments.find(item => item.coordinator && !item.retiredAt && item.currentAttemptId === attemptId);
      if (!coordinator || attempt.ownership !== 'held' || run.status !== 'running' || run.desiredState !== 'running' || run.sourceConflict || this.closing) throw new Conflict('Only the active coordinator may request publication.');
      const ownPublication = this.store.publication(run.id);
      const inherited = !ownPublication ? this.store.continuationPublication(run.id) : undefined;
      const previous = ownPublication ?? inherited;
      if (previous?.merged) throw new Conflict('This PR is merged; delivery is complete. New work needs a new run.');
      if (previous && previous.candidateId !== candidateId) {
        if (!inherited && !this.continuesFrom(run.id, candidateId, previous)) throw new Conflict('Publication already belongs to another candidate. Explicit review continuation is required.');
        if (previous.pr.state === 'confirmed' && previous.pr.providerState !== 'OPEN') throw new Conflict('The existing PR is not open. No successor can be published.');
        if (!['pending', 'confirmed'].includes(previous.push.state) || !['pending', 'confirmed'].includes(previous.pr.state)) throw new Conflict('Reconcile the publication receipts before publishing the successor candidate.');
      }
      return { run, previous };
    };
    const first = authorized();
    if (first.previous?.candidateId === candidateId) {
      if (first.previous.summary === summary) return (await this.read(first.run.id))!;
      // A revised summary rewrites the existing PR description once. A flush saves the record it loaded, so wait for it first.
      while (this.flushing) await this.flushing.catch(() => {});
      const { run, previous } = authorized();
      if (previous?.candidateId !== candidateId) throw new Conflict('Publication changed while revising its description. Inspect publication_status.');
      if (['sending', 'uncertain'].includes(previous.pr.state)) throw new Conflict('Reconcile the PR receipt before revising its description.');
      const record: PublicationRecord = { ...previous, summary }; record.description = this.describe(record);
      this.store.savePublication(record); return (await this.read(run.id))!;
    }
    const owned = await this.work.status(first.run.id); const assessment = await this.evidence.assess(first.run.id, owned);
    // A flush saves the record it loaded, so a successor waits for any flush in progress. No await follows before the save.
    if (first.previous) while (this.flushing) await this.flushing.catch(() => {});
    // Coordinator ownership, run state and any concurrent request are rechecked after Git, before the durable intent.
    const { run, previous } = authorized(); if (previous?.candidateId === candidateId) return (await this.read(run.id))!;
    if (!assessment.ready || assessment.candidate?.id !== candidateId || owned.writer || owned.revision.head === owned.revision.base) throw new Conflict('Publication requires a changed, clean and locally validated current candidate.');
    if (previous && (owned.branch !== previous.branch || owned.repository.path !== previous.repositoryPath || owned.repository.targetBranch !== previous.target))
      throw new Conflict('The owned branch or target repository changed. The existing PR cannot be replaced.');
    const connection = this.connections.find(item => item.repositoryPath === owned.repository.path);
    if (!connection) throw new Conflict('Bitbucket publication is not configured for this repository.');
    if (owned.branch === owned.repository.targetBranch) throw new Conflict('Publication cannot push the target branch.');
    if (previous) {
      // The refreshed candidate replaces the published one on the same branch and PR. The lease expects what was pushed.
      const pushedHead = previous.push.state === 'confirmed' ? previous.revision.head : undefined;
      const record: PublicationRecord = { ...previous, runId: run.id, attemptId, candidateId, summary, revision: assessment.candidate.revision,
        history: [...(previous.history ?? []), { candidateId: previous.candidateId, revision: previous.revision, pushedHead, supersededAt: new Date().toISOString() }],
        push: { state: 'pending', expectedHead: pushedHead ?? previous.push.expectedHead }, observation: undefined, error: undefined, ready: false, blockers: ['Publication has not been confirmed.'] };
      record.description = this.describe(record);
      return this.store.savePublication(record);
    }
    const record: PublicationRecord = { id: randomUUID(), runId: run.id, attemptId, candidateId, repositoryPath: owned.repository.path, repository: structuredClone(connection.repository),
      branch: owned.branch, target: owned.repository.targetBranch, revision: assessment.candidate.revision, title: `${run.taskSnapshot.key}: ${run.taskSnapshot.title}`, summary,
      description: '', policy: policySchema.parse(connection.policy), createdAt: new Date().toISOString(), push: { state: 'pending', expectedHead: null }, pr: { state: 'pending' }, ready: false, blockers: ['Publication has not been confirmed.'] };
    record.description = this.describe(record);
    return this.store.savePublication(record);
  }
  /** The coordinator's reviewer-facing summary, then Crewroom's evidence for the exact revision. */
  private describe(record: PublicationRecord) {
    const run = this.store.getRun(record.runId); const coding = this.store.codingRecord(record.runId)!;
    const lineage: string[] = [];
    for (let current = coding.candidates.find(item => item.id === record.candidateId); current; current = current.refreshedFrom ? coding.candidates.find(item => item.id === current!.refreshedFrom) : undefined) lineage.unshift(current.id);
    const refreshes = (this.store.repositoryWork(record.runId)?.refreshes ?? []).filter(item => item.state === 'completed' && item.candidateId && lineage.includes(item.candidateId));
    return [record.summary ?? run.taskSnapshot.brief, '', '---', '', '### Crewroom evidence', '',
      ...(run.taskSnapshot.sourceUrl ? [`Jira: ${run.taskSnapshot.sourceUrl}`, ''] : []),
      'Acceptance criteria:', ...run.taskSnapshot.criteria.map(item => `- ${item}`), '',
      `- Candidate commit \`${record.revision.head}\` on base \`${record.revision.base}\` (candidate ${record.candidateId})`,
      ...refreshes.map(item => `- Rebased onto the target: \`${item.fromBase.slice(0, 12)}\` → \`${item.toBase.slice(0, 12)}\``),
      '- Required local checks, run in isolated containers against this exact commit:',
      ...coding.requiredChecks.filter(check => check.required).map(check => {
        const receipt = coding.checks.findLast(item => item.candidateId === record.candidateId && item.checkId === check.id)!;
        return `  - ${check.id}: passed (output SHA-256 \`${receipt.output.sha256}\`)`;
      }),
      '- Independent review passed for this commit.',
      "- Hosted checks are this PR's build statuses and pipeline. Crewroom reports the PR ready only when they pass for this head and base.",
      "- Merge is the operator's decision.", '', marker(record)].join('\n');
  }
  private assess(record: PublicationRecord, local: EvidenceAssessment | undefined) {
    const blockers: string[] = [];
    if (record.merged) return { ready: false, blockers: [], reviewInvalidated: false };
    if (!local) blockers.push('Local evidence is unavailable.');
    else if (!local.ready || local.candidate?.id !== record.candidateId) blockers.push('Local evidence does not cover the current publication revision.');
    if (this.store.getRun(record.runId).sourceConflict) blockers.push('Jira requirements changed.');
    if (record.error) blockers.push(record.error);
    if (record.push.state !== 'confirmed' || record.pr.state !== 'confirmed') blockers.push('Push and PR creation both require confirmed receipts.');
    else if (record.pr.describedSha256 !== digest(record.description)) blockers.push('The PR description update has not been confirmed.');
    if (record.remoteSourceHead !== record.revision.head) blockers.push('The remote source branch no longer matches the published candidate.');
    if (record.remoteTargetHead !== record.revision.base) blockers.push('The remote target does not match the validated base.');
    const observed = record.observation;
    if (!observed) blockers.push('Hosted validation has not been observed.');
    else {
      const pr = observed.pr;
      if (pr.state !== 'OPEN' || pr.draft || (record.pr.providerState && record.pr.providerState !== 'OPEN')) blockers.push('The pull request is not open for human review.');
      if (pr.source.commit.hash !== record.revision.head || pr.destination.commit.hash !== record.revision.base || pr.source.branch.name !== record.branch || pr.destination.branch.name !== record.target) blockers.push('The PR head, base or branch no longer matches local evidence.');
      for (const key of record.policy.statusKeys) {
        const status = observed.statuses.find(item => item.key === key);
        if (status?.state !== 'SUCCESSFUL' || (status.refname && status.refname !== record.branch)) blockers.push(`Hosted status ${key} does not confirm success for this branch and revision.`);
      }
      const pipeline = observed.pipelines[0];
      if (!pipeline || pipeline.target.commit.hash !== record.revision.head || pipeline.target.destination_commit?.hash !== record.revision.base || pipeline.target.pullrequest?.id !== record.pr.id) blockers.push('The required PR pipeline has not validated this head and base.');
      else {
        if (pipeline.state.name !== 'COMPLETED' || pipeline.state.result?.name !== 'SUCCESSFUL') blockers.push('The required PR pipeline is incomplete or unsuccessful.');
        for (const name of record.policy.pipelineSteps) {
          const matching = pipeline.steps.filter(step => step.name === name);
          if (matching.length !== 1 || matching[0].state.name !== 'COMPLETED' || matching[0].state.result?.name !== 'SUCCESSFUL') blockers.push(`Pipeline step ${name} is missing, ambiguous, skipped or unsuccessful.`);
        }
        if (pipeline.steps.some(step => ![...record.policy.pipelineSteps, ...record.policy.optionalSteps].includes(step.name))) blockers.push('The pipeline contains steps outside the accepted check policy.');
      }
    }
    const ready = blockers.length === 0;
    const failedHostedCheck = !!observed && observed.pr.source.commit.hash === record.revision.head
      && (record.policy.statusKeys.some(key => observed.statuses.find(item => item.key === key)?.state === 'FAILED')
        || observed.pipelines.some(pipeline => pipeline.target.commit.hash === record.revision.head
          && pipeline.target.destination_commit?.hash === record.revision.base && pipeline.target.pullrequest?.id === record.pr.id
          && (pipeline.state.result?.name === 'FAILED' || pipeline.steps.some(step =>
            record.policy.pipelineSteps.includes(step.name) && step.state.result?.name === 'FAILED'))));
    const observedRevisionChanged = !!observed && (observed.pr.source.commit.hash !== record.revision.head
      || observed.pr.destination.commit.hash !== record.revision.base || observed.pr.source.branch.name !== record.branch
      || observed.pr.destination.branch.name !== record.target);
    const confirmedRegression = !!this.store.getRun(record.runId).sourceConflict || local?.confirmedInvalidation === true
      || (!!local?.candidate && local.candidate.id !== record.candidateId)
      || (!!record.remoteSourceHead && record.remoteSourceHead !== record.revision.head)
      || (!!record.remoteTargetHead && record.remoteTargetHead !== record.revision.base)
      || observedRevisionChanged || failedHostedCheck;
    const reviewInvalidated = !ready && (record.reviewInvalidated === true || confirmedRegression);
    return { ready, blockers, reviewInvalidated };
  }
  /** Run state is rechecked after the evidence gate, with no await before the caller dispatches. */
  private async mayWrite(record: PublicationRecord) {
    const writable = () => {
      if (this.closing) throw new Conflict('Publication transport is stopping.');
      const run = this.store.getRun(record.runId);
      if (run.status !== 'running' || run.desiredState !== 'running' || run.sourceConflict) throw new Conflict('Publication is waiting for an active run with accepted requirements.');
    };
    writable(); const local = await this.evidence.assess(record.runId); writable();
    if (!local.ready || local.candidate?.id !== record.candidateId) throw new Conflict('Publication evidence is stale. Revalidate before writing.');
  }
  private confirmPR(record: PublicationRecord, pr: BitbucketPullRequest) {
    if (!pr.description.includes(marker(record)) || pr.source.branch.name !== record.branch || pr.destination.branch.name !== record.target) throw new Conflict('The PR does not match the durable publication identity.');
    record.pr = { state: 'confirmed', id: pr.id, url: pr.links.html.href, describedSha256: digest(pr.description === record.description ? record.description : pr.description), providerState: pr.state }; this.store.savePublication(record);
  }
  async flush() {
    if (this.closing) return;
    if (!this.flushing) this.flushing = this.performFlush();
    try { await this.flushing; } finally { this.flushing = undefined; }
  }
  private async performFlush() {
    for (const record of this.store.publications()) {
      if (this.closing) break;
      if (this.store.publicationSuperseded(record.runId, record.id)) continue;
      if (record.merged) continue;
      const connection = this.connections.find(item => item.repositoryPath === record.repositoryPath && JSON.stringify(item.repository) === JSON.stringify(record.repository));
      if (!connection) { record.error = 'The configured Bitbucket repository no longer matches this publication.'; record.ready = false; this.store.savePublication(record); continue; }
      const client = new BitbucketClient(connection);
      const git = new PublicationGit(join(this.root, record.runId), connection.fixtureGitPath ? { fixturePath: connection.fixtureGitPath } : record.repository, [record.target, 'main', 'master']);
      this.active.set(record.runId, git); record.lastAttemptAt = new Date().toISOString();
      if (record.push.state !== 'blocked' && record.pr.state !== 'blocked') record.error = undefined;
      if (record.push.state === 'sending') record.push.state = 'uncertain';
      if (record.pr.state === 'sending') record.pr.state = 'uncertain';
      try {
        await client.verifyRepository();
        record.remoteTargetHead = await git.ref(record.target);
        record.remoteSourceHead = await git.ref(record.branch);
        if (record.push.state !== 'confirmed' && record.push.state !== 'blocked') {
          const remote = record.remoteSourceHead;
          if (remote === record.revision.head) { record.push.state = 'confirmed'; this.store.savePublication(record); }
          else if (remote !== record.push.expectedHead && !(remote === undefined && record.push.expectedHead === null)) { record.push.state = 'blocked'; throw new Conflict('Remote branch changed outside this publication. No overwrite was attempted.'); }
          else {
            if (record.pr.state === 'confirmed' && record.pr.id) {
              const current = await client.pullRequest(record.pr.id);
              record.pr.providerState = current.state; this.store.savePublication(record);
              if (current.state !== 'OPEN' || !current.description.includes(marker(record))
                || current.source.branch.name !== record.branch || current.destination.branch.name !== record.target
                || current.source.commit.hash !== record.push.expectedHead || current.destination.commit.hash !== record.revision.base) {
                record.push.state = 'blocked'; throw new Conflict('The existing PR closed, changed head/base, or lost its publication identity. No push was attempted.');
              }
            }
            // The owned path is read first: mayWrite's final state check must be the last await before the push starts.
            const owned = await this.work.status(record.runId);
            await this.mayWrite(record);
            if (record.remoteTargetHead !== record.revision.base) throw new Conflict('Remote target changed. Refresh and validate the base before publication.');
            record.push.state = 'sending'; this.store.savePublication(record);
            await git.push({ directory: owned.path, branch: record.branch, head: record.revision.head, expectedHead: record.push.expectedHead });
            record.remoteSourceHead = await git.ref(record.branch);
            if (record.remoteSourceHead !== record.revision.head) throw new Conflict('Remote branch did not confirm the publication revision.');
            record.push.state = 'confirmed'; this.store.savePublication(record);
          }
        }
        if (record.push.state !== 'confirmed') throw new Conflict(record.error ?? 'Git publication remains blocked.');
        if (record.pr.state !== 'confirmed' && record.pr.state !== 'blocked') {
          const existing = await client.pullRequests(record.branch, record.target);
          const matches = existing.filter(pr => pr.description.includes(marker(record)));
          if (matches.length > 1) throw new Conflict('Multiple PRs claim this publication identity. Inspect the provider before continuing.');
          if (matches.length === 1) this.confirmPR(record, matches[0]);
          else if (record.pr.state === 'uncertain') throw new Conflict('PR creation outcome is unknown. No matching receipt was found; no duplicate will be created.');
          else if (existing.some(pr => pr.state === 'OPEN')) { record.pr.state = 'blocked'; throw new Conflict('An existing PR uses this branch without the recorded publication identity.'); }
          else {
            await this.mayWrite(record);
            if (record.remoteSourceHead !== record.revision.head) throw new Conflict('Remote source changed after publication. Reconcile the branch before PR creation.');
            if (record.remoteTargetHead !== record.revision.base) throw new Conflict('Remote target changed. Refresh and validate before PR creation.');
            record.pr.state = 'sending'; this.store.savePublication(record);
            this.confirmPR(record, await client.createPullRequest({ title: record.title, description: record.description, branch: record.branch, target: record.target, head: record.revision.head }, () => this.mayWrite(record)));
          }
        }
        if (record.pr.state === 'confirmed' && record.pr.describedSha256 !== digest(record.description)) {
          const current = await client.pullRequest(record.pr.id!);
          record.pr.providerState = current.state; this.store.savePublication(record);
          if (current.description === record.description && current.title === record.title) {
            record.pr.describedSha256 = digest(record.description); this.store.savePublication(record);
          } else {
            if (current.state !== 'OPEN' || !current.description.includes(marker(record)) || current.source.commit.hash !== record.revision.head
              || current.destination.commit.hash !== record.revision.base || current.source.branch.name !== record.branch
              || current.destination.branch.name !== record.target) throw new Conflict('The existing PR closed or changed identity before its description update. No PR write was attempted.');
            await this.mayWrite(record);
            const pr = await client.updatePullRequest(record.pr.id!, { title: record.title, description: record.description }, () => this.mayWrite(record));
            if (pr.state !== 'OPEN' || pr.description !== record.description || pr.source.commit.hash !== record.revision.head)
              throw new Conflict('The PR description update was not confirmed for this revision.');
            record.pr = { ...record.pr, describedSha256: digest(record.description), providerState: pr.state }; this.store.savePublication(record);
          }
        }
        if (record.pr.state === 'confirmed') {
          // The PR's own state is saved first, so a hosted-check outage cannot leave a stale state for Jira's review rule.
          const current = await client.pullRequest(record.pr.id!); record.pr = { ...record.pr, providerState: current.state };
          // The operator's merge, read from the PR itself at the exact published head and target, completes delivery even while hosted checks are unavailable.
          if (current.state === 'MERGED' && current.merge_commit && current.source.commit.hash === record.revision.head
            && current.source.branch.name === record.branch && current.destination.branch.name === record.target) record.merged = { commit: current.merge_commit.hash, observedAt: new Date().toISOString() };
          this.store.savePublication(record);
          if (!record.merged) {
            const observation = await client.observe(record.pr.id!); const previous = record.observation;
            if (previous && (Date.parse(observation.pr.updated_on) < Date.parse(previous.pr.updated_on)
              || observation.statuses.some(status => previous.statuses.some(old => old.key === status.key && Date.parse(status.updated_on) < Date.parse(old.updated_on)))
              || (observation.pipelines[0] && previous.pipelines[0] && observation.pr.source.commit.hash === previous.pr.source.commit.hash
                && (observation.pipelines[0].build_number < previous.pipelines[0].build_number
                  || (observation.pipelines[0].build_number === previous.pipelines[0].build_number && (observation.pipelines[0].run_number ?? 0) < (previous.pipelines[0].run_number ?? 0)))))) throw new Conflict('Bitbucket returned older observations. Retained evidence was not replaced.');
            record.observation = observation; record.pr = { ...record.pr, providerState: observation.pr.state };
          }
        }
      } catch (error) {
        if (record.push.state === 'sending') record.push.state = 'uncertain';
        if (record.pr.state === 'sending') record.pr.state = error instanceof BitbucketUnavailable && !error.requestSent ? 'pending'
          : error instanceof BitbucketUnavailable && error.status && error.status >= 400 && error.status < 500 ? error.status === 429 ? 'pending' : 'blocked' : 'uncertain';
        record.error = error instanceof Conflict || error instanceof BitbucketUnavailable ? error.message : 'Publication is unavailable. Reconcile retained intents before another write.';
      } finally {
        await git.close(); this.active.delete(record.runId);
        Object.assign(record, this.assess(record, await this.localAssessment(record.runId))); this.store.savePublication(record);
        if (record.pr.state !== 'pending' || ['blocked', 'uncertain'].includes(record.push.state)) this.store.noticeDelivery(record.runId, {
          kind: 'publication', subjectId: record.id, candidateId: record.candidateId, pushState: record.push.state, prState: record.pr.state,
          prUrl: record.pr.url, ready: record.ready, blockers: record.blockers,
        });
      }
    }
  }
  async stopRun(runId: string) { await Promise.all([...this.active].filter(([key]) => key === runId || key.startsWith(`${runId}:`)).map(([, git]) => git.close())); }
  async close() { this.closing = true; await Promise.all([...this.active.values()].map(git => git.close())); await this.flushing; }
}
