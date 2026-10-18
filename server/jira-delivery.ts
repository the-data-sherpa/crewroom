import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import { Workbench, Conflict } from './workbench';
import { JiraClient, JiraUnavailable } from './jira-client';
import { descriptionText } from './jira';
import type { Evidence } from './evidence';
import type { Research } from './research';
import type { JiraUpdate } from '../shared/jira';

const eventKey = z.string().regex(/^[a-zA-Z0-9][a-zA-Z0-9:._-]{0,199}$/);
const reportSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.enum(['milestone', 'blocker']), eventKey, body: z.string().trim().min(1).max(12000) }).strict(),
  z.object({ kind: z.literal('in_progress'), eventKey: z.literal('execution-started') }).strict(),
  z.object({ kind: z.enum(['evidence', 'pr_link']), eventKey, candidateId: z.string().uuid() }).strict(),
  z.object({ kind: z.literal('research_outcome'), eventKey }).strict(),
]);
const marker = (update: JiraUpdate) => `Crewroom update ${update.id}`;
const controllerKinds = new Set<JiraUpdate['kind']>(['in_review', 'done']);
function commentText(update: JiraUpdate) {
  return `${['evidence', 'pr_link', 'research_outcome'].includes(update.kind) ? 'Crewroom verified evidence' : `Crewroom ${update.kind} — agent report`}\n\n${update.body}\n\nRun ${update.runId}\n${marker(update)}`;
}
function document(update: JiraUpdate) {
  return { type: 'doc', version: 1, content: commentText(update).split('\n').map(text => ({ type: 'paragraph', content: text ? [{ type: 'text', text }] : [] })) };
}

/** Durable provider intents. An unknown mutation is reconciled by reading, never blindly repeated. */
export class JiraDelivery {
  private flushing?: Promise<void>;
  private closing = false;
  constructor(private store: Workbench, private client: JiraClient, private evidence?: Evidence, private research?: Research) {}
  read(runId: string) { this.store.getRun(runId); return this.store.jiraUpdates(runId); }
  /** A linked research ticket is reported by its active reporter, or by its coordinator if no reporter is assigned. */
  needsResearchSummary(runId: string, revision: number) {
    const task = this.store.getTask(this.store.getRun(runId).taskId);
    return task.source === 'jira' && task.sourceSite === this.client.source.siteUrl && !this.read(runId).some(item => item.kind === 'research_outcome' && item.eventKey === `research-outcome-${revision}` && item.state !== 'blocked');
  }

  /** The reporter requests an outcome summary; the server supplies content only from verified artifacts. */
  private researchSummary(runId: string) {
    if (!this.research) throw new Conflict('Research evidence is unavailable.');
    const run = this.store.getRun(runId); const result = this.research.read(runId);
    if (run.purpose !== 'research' || !result.assessment.ready || !run.researchCompletion || run.researchCompletion.revision !== run.taskRevision) throw new Conflict('A requested, fully verified research outcome is required before reporting to Jira.');
    const artifacts = result.artifacts.filter(item => item.state === 'verified' && !item.supersededBy && item.requirementRevision === run.taskRevision);
    return `Research outcome completed for accepted revision ${run.taskRevision}.\n${artifacts.map(item => `- ${item.title} (${item.kind}), SHA-256 ${item.body.sha256}\n  Limitations: ${item.limitations}`).join('\n')}\nCrewroom reference: run ${run.id}, /api/runs/${run.id}/result\nNo code change or PR is claimed.`;
  }

  async prepare(attemptId: string, input: unknown) {
    const value = reportSchema.parse(input);
    const authorized = () => {
      const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
      const assignment = this.store.runtime(run.id).assignments.find(item => item.currentAttemptId === attemptId);
      const reporter = !!assignment && !assignment.coordinator && assignment.role.id === 'delivery-reporter';
      const coordinatorFallback = !!assignment?.coordinator && value.kind === 'research_outcome' && run.purpose === 'research' && !this.store.runtime(run.id).assignments.some(item => !item.retiredAt && item.role.id === 'delivery-reporter');
      if (!assignment || assignment.retiredAt || (!reporter && !coordinatorFallback) || attempt.ownership !== 'held'
        || !['idle', 'executing'].includes(attempt.state) || run.status !== 'running' || run.desiredState !== 'running' || run.sourceConflict) throw new Conflict('Only the active delivery reporter, or a research coordinator without one, may request Jira updates.');
      const task = this.store.getTask(run.taskId);
      if (task.source !== 'jira' || task.sourceSite !== this.client.source.siteUrl) throw new Conflict('This run does not belong to the configured Jira site.');
      const prior = this.read(run.id).find(item => item.eventKey === value.eventKey);
      if (prior && (prior.kind !== value.kind || ('candidateId' in value ? prior.candidateId !== value.candidateId : 'body' in value && prior.body !== value.body))) throw new Conflict('Jira event key has different content.');
      if (run.purpose === 'research' && !['milestone', 'blocker', 'research_outcome'].includes(value.kind)) throw new Conflict('Research does not report coding evidence or Jira transitions.');
      if (value.kind === 'research_outcome' && (run.purpose !== 'research' || value.eventKey !== `research-outcome-${run.taskRevision}`)) throw new Conflict('Research outcome event key must identify the accepted revision.');
      return { run, task, prior };
    };
    const first = authorized(); if (first.prior) return first.prior;
    const body = value.kind === 'pr_link' ? await this.publicationText(first.run.id, value.candidateId) : value.kind === 'evidence' ? await this.evidenceText(first.run.id, value.candidateId) : value.kind === 'research_outcome' ? this.researchSummary(first.run.id) : undefined;
    // Reporter ownership and any concurrent request are rechecked after the evidence gate.
    const { run, task, prior } = authorized(); if (prior) return prior;
    const update: JiraUpdate = { id: randomUUID(), runId: run.id, attemptId, siteUrl: task.sourceSite!, issueId: task.sourceId!, ...value,
      state: 'pending', createdAt: new Date().toISOString() };
    if (value.kind === 'in_progress') {
      if (!this.client.source.inProgressStatusId || !this.client.source.inProgressTransitionId) throw new Conflict('Jira progress transition is not configured.');
      update.statusId = this.client.source.inProgressStatusId; update.transitionId = this.client.source.inProgressTransitionId;
    }
    if (body !== undefined) update.body = body;
    return this.store.saveJiraUpdate(update);
  }
  private async evidenceText(runId: string, candidateId: string) {
    if (!this.evidence) throw new Conflict('Verified evidence is unavailable.');
    const assessment = await this.evidence.assess(runId);
    if (!assessment.ready || assessment.candidate?.id !== candidateId) throw new Conflict('Evidence must cover the current verified candidate.');
    const record = this.store.codingRecord(runId)!;
    return `Revision ${assessment.candidate.revision.head}\nCandidate ${candidateId}\nRequired checks passed: ${record.requiredChecks.filter(check => check.required).map(check => check.id).join(', ')}\nIndependent review passed for this revision.\nMerge remains an operator decision.`;
  }
  private async publicationText(runId: string, candidateId: string) {
    const body = await this.evidenceText(runId, candidateId); const record = this.store.publication(runId);
    if (!record || record.candidateId !== candidateId || record.pr.state !== 'confirmed' || record.remoteSourceHead !== record.revision.head
      || record.observation?.pr.source.commit.hash !== record.revision.head || record.observation.pr.state !== 'OPEN'
      || record.pr.url !== `https://bitbucket.org/${record.repository.workspace}/${record.repository.slug}/pull-requests/${record.pr.id}`) throw new Conflict('A confirmed PR receipt for this candidate is required.');
    return `${body}\n\nPull request: ${record.pr.url}\nHosted validation must be checked against the current PR revision.`;
  }
  async flush() {
    if (this.closing) return;
    if (!this.flushing) this.flushing = this.performFlush();
    try { await this.flushing; } finally { this.flushing = undefined; }
  }
  async close() { this.closing = true; await this.flushing; }
  /** Run state is rechecked after the evidence gate, with no await before the caller dispatches. */
  private async maySend(update: JiraUpdate) {
    const active = () => {
      if (this.closing) { update.reason = 'Waiting for the server to resume.'; return false; }
      const run = this.store.getRun(update.runId);
      if (update.kind === 'research_outcome') {
        if (run.researchCompletion?.revision === run.taskRevision && !run.sourceConflict && ['running', 'pausing', 'paused'].includes(run.status)) { update.reason = 'Waiting for confirmed research completion.'; return false; }
        if (run.status === 'completed' && run.researchOutcome?.revision === run.taskRevision && !run.sourceConflict) return true;
        update.state = 'blocked'; update.reason = 'The research outcome is no longer complete for current requirements.'; return false;
      }
      if (run.sourceConflict || run.desiredState === 'cancelled') { update.state = 'blocked'; update.reason = 'Run requirements changed or the run was cancelled. This update will not be sent.'; return false; }
      // Controller transitions follow provider receipts, not agent activity, so they do not wait for the run to execute.
      if (!controllerKinds.has(update.kind) && (run.status !== 'running' || run.desiredState !== 'running')) { update.reason = 'Waiting for the run to resume.'; return false; }
      return true;
    };
    if (!active()) return false;
    if (update.kind === 'research_outcome') {
      let current = false;
      try { current = this.researchSummary(update.runId) === update.body && this.research?.read(update.runId).assessment.completed === true; } catch {}
      if (!current) { update.state = 'blocked'; update.reason = 'The research outcome is no longer complete for current requirements.'; return false; }
      return true;
    }
    if (update.kind === 'evidence' || update.kind === 'pr_link') {
      let current = false;
      try { current = (update.kind === 'pr_link' ? await this.publicationText(update.runId, update.candidateId!) : await this.evidenceText(update.runId, update.candidateId!)) === update.body; } catch {}
      if (!current) { update.state = 'blocked'; update.reason = 'The recorded evidence is no longer current.'; return false; }
      return active();
    }
    if (controllerKinds.has(update.kind) && !this.transitionStillDue(update)) { update.state = 'blocked'; update.reason = 'The publication no longer supports this Jira transition.'; return false; }
    // A progress transition must never reopen a ticket after its PR merged.
    if (update.kind === 'in_progress' && this.store.publication(update.runId)?.merged) { update.state = 'blocked'; update.reason = 'Superseded by the merge.'; return false; }
    return true;
  }
  /** Review follows an open PR that is not merged; close follows a verified merge. */
  private transitionStillDue(update: JiraUpdate) {
    const record = this.store.publication(update.runId);
    return update.kind === 'done' ? !!record?.merged : !!record && record.pr.state === 'confirmed' && record.pr.providerState === 'OPEN' && !record.merged;
  }
  /**
   * Operator policy: a created PR moves the ticket to review, and a verified merge closes it. The intents are
   * derived from publication receipts once each; the merge supersedes a review transition that has not been sent.
   */
  private deriveTransitions() {
    const source = this.client.source;
    for (const record of this.store.publications()) {
      const run = this.store.getRun(record.runId); const task = this.store.getTask(run.taskId);
      if (task.source !== 'jira' || task.sourceSite !== source.siteUrl) continue;
      const updates = this.read(run.id);
      const intent = (kind: 'in_review' | 'done', statusId: string | undefined, transitionId: string | undefined) => {
        const key = `${kind === 'done' ? 'merged' : 'review'}-${record.id}`;
        if (!statusId || !transitionId || updates.some(item => item.eventKey === key)) return;
        this.store.saveJiraUpdate({ id: randomUUID(), runId: run.id, attemptId: record.attemptId, siteUrl: task.sourceSite!, issueId: task.sourceId!, eventKey: key, kind,
          statusId, transitionId, candidateId: record.candidateId, state: 'pending', createdAt: new Date().toISOString() });
      };
      if (record.merged) {
        // The merge supersedes every review or progress transition not confirmed, including one whose earlier outcome is unknown: it is never resent.
        for (const stale of updates.filter(item => ['in_review', 'in_progress'].includes(item.kind) && item.state !== 'confirmed' && !item.reason?.startsWith('Superseded by the merge'))) {
          this.store.saveJiraUpdate({ ...stale, state: 'blocked', reason: stale.state === 'uncertain' || stale.state === 'sending' ? "Superseded by the merge; the earlier transition's outcome is unknown." : 'Superseded by the merge.' });
        }
        intent('done', source.doneStatusId, source.doneTransitionId);
      } else if (record.pr.state === 'confirmed' && record.pr.providerState === 'OPEN') intent('in_review', source.reviewStatusId, source.reviewTransitionId);
    }
  }
  private async performFlush() {
    this.deriveTransitions();
    // A stale research revision must not hold the current revision's summary; other agent updates remain ordered.
    const held = new Set<string>(); const lane = (update: JiraUpdate) => `${update.runId}:${update.kind === 'research_outcome' ? `outcome:${update.eventKey}` : controllerKinds.has(update.kind) ? 'controller' : 'agent'}`;
    for (const update of this.store.jiraUpdates()) {
      if (update.state === 'confirmed' || held.has(lane(update))) continue;
      // Superseded transitions, and a review transition that could not run, never hold later updates.
      const releases = (item: JiraUpdate) => item.kind === 'in_review' || !!item.reason?.startsWith('Superseded by the merge');
      if (update.state === 'blocked') { if (!releases(update)) held.add(lane(update)); continue; }
      if (update.state === 'sending') update.state = 'uncertain';
      if (update.siteUrl !== this.client.source.siteUrl) { update.state = 'blocked'; update.reason = 'The configured Jira site changed.'; this.store.saveJiraUpdate(update); held.add(lane(update)); continue; }
      if (update.state === 'pending' && !(await this.maySend(update))) { this.store.saveJiraUpdate(update); if (!((update.state as JiraUpdate['state']) === 'blocked' && releases(update))) held.add(lane(update)); continue; }
      update.lastAttemptAt = new Date().toISOString();
      let dispatched = false;
      try {
        if (update.kind === 'in_progress' || controllerKinds.has(update.kind)) {
          const issue = await this.client.issue(update.issueId);
          if (issue.fields.status.id === update.statusId) { update.state = 'confirmed'; update.externalId = update.statusId; }
          else if (update.state !== 'uncertain') {
            const transition = (await this.client.transitions(update.issueId)).find(item => item.id === update.transitionId && item.to.id === update.statusId);
            if (!transition || Object.values(transition.fields ?? {}).some(field => field.required)) { update.state = 'blocked'; update.reason = 'The configured Jira transition is unavailable or requires additional fields.'; }
            else if (await this.maySend(update)) {
              update.state = 'sending'; this.store.saveJiraUpdate(update); dispatched = true;
              await this.client.transition(update.issueId, update.transitionId!, async () => { if (!(await this.maySend(update))) throw new JiraUnavailable('The run stopped before Jira dispatch.'); });
              update.state = 'confirmed'; update.externalId = update.statusId;
            }
          }
        } else {
          const existing = (await this.client.comments(update.issueId)).filter(item => descriptionText(item.body).includes(marker(update)));
          if (existing.length) {
            if (existing.length === 1 && descriptionText(existing[0].body) === descriptionText(document(update))) { update.state = 'confirmed'; update.externalId = existing[0].id; }
            else { update.state = 'blocked'; update.reason = 'The remote update marker has conflicting content. Inspect Jira before resolving it.'; }
          } else if (update.state !== 'uncertain' && await this.maySend(update)) {
            update.state = 'sending'; this.store.saveJiraUpdate(update); dispatched = true;
            const result = await this.client.addComment(update.issueId, document(update), async () => { if (!(await this.maySend(update))) throw new JiraUnavailable('The run stopped before Jira dispatch.'); });
            update.state = 'confirmed'; update.externalId = result.id;
          }
        }
        if (update.state === 'confirmed') update.reason = undefined;
        else if (update.state === 'uncertain') update.reason = 'The previous write outcome is unknown. Waiting for a matching Jira receipt; no duplicate will be sent.';
      } catch (error) {
        const rejected = error instanceof JiraUnavailable && error.status && error.status >= 400 && error.status < 500;
        const notSent = dispatched && error instanceof JiraUnavailable && !error.requestSent;
        update.state = notSent ? update.state === 'blocked' ? 'blocked' : 'pending' : rejected && update.state !== 'uncertain' ? (error.status === 429 ? 'pending' : 'blocked') : dispatched ? 'uncertain' : update.state;
        update.reason = error instanceof JiraUnavailable ? error.message : 'Jira returned an invalid response. The update is retained for reconciliation.';
      }
      this.store.saveJiraUpdate(update);
      if (update.state !== 'confirmed') held.add(lane(update));
    }
  }
}
