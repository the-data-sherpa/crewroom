import { attemptModel } from './runtime/types';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import { Conflict, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import type { ResearchArtifact, ResearchCitation } from '../shared/research';

const citation = z.object({ path: z.string().min(1).max(1000), commit: z.string().min(1).max(64), startLine: z.number().int(), endLine: z.number().int(), excerpt: z.string().max(20000) }).strict();
const artifactInput = z.object({ title: z.string().trim().min(1).max(200), kind: z.enum(['research', 'design', 'documentation-draft']),
  body: z.string().min(1).max(1024 * 1024), citations: z.array(citation).max(100),
  limitations: z.string().max(20000).optional(), coverage: z.array(z.object({ criterionId: z.number().int(), section: z.string().trim().min(1).max(200) }).strict()).max(100).optional() }).strict();
const normalize = (text: string) => text.replace(/\s+/g, ' ').trim();

/** A separate evidence gate; research never creates a candidate, check, review or publication record. */
export class Research {
  constructor(private store: Workbench, private source: RepositoryWork, private evidence: Evidence) {}

  read(runId: string) {
    const run = this.store.getRun(runId);
    if (run.purpose !== 'research') throw new Conflict('This is not a research run.');
    const artifacts = this.store.researchArtifacts(runId);
    const current = artifacts.filter(item => item.state === 'verified' && !item.supersededBy && item.requirementRevision === run.taskRevision && item.pinnedCommit === run.researchSource?.commit);
    const valid = current.filter(item => { try { this.evidence.readArtifact(item.body); return true; } catch { return false; } });
    const covered = new Set(valid.flatMap(item => item.coverage.map(entry => entry.criterionId)));
    const blockers = run.taskSnapshot.criteria.flatMap((text, id) => covered.has(id) ? [] : [`Criterion ${id}: ${text} has no verified current artifact.`]);
    blockers.push(...this.store.insertionBlockers(runId));
    if (current.length !== valid.length) blockers.push('A current artifact is missing or its SHA-256 changed.');
    return { purpose: 'research' as const, source: run.researchSource, criteria: run.taskSnapshot.criteria, artifacts,
      assessment: { ready: blockers.length === 0, blockers, covered: [...covered].sort((a, b) => a - b), revision: run.taskRevision,
        completed: run.status === 'completed' && run.researchOutcome?.revision === run.taskRevision }, outcome: run.researchOutcome };
  }

  async submit(attemptId: string, toolCallId: string, input: unknown): Promise<ResearchArtifact> {
    const value = artifactInput.parse(input); const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
    if (run.purpose !== 'research') throw new Conflict('Research artifacts require a research run.');
    const prior = this.store.researchArtifacts(run.id).find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (prior) {
      if (JSON.stringify([prior.title, prior.kind, this.evidence.readArtifact(prior.body), prior.citations.map(({ verified, reason, ...rest }) => rest), prior.limitations, prior.coverage]) !== JSON.stringify([value.title, value.kind, value.body, value.citations, value.limitations ?? '', value.coverage ?? []])) throw new Conflict('Artifact tool identity has different content.');
      return prior;
    }
    const assignment = this.store.runtime(run.id).assignments.find(item => item.id === attempt.assignmentId && item.currentAttemptId === attemptId && !item.retiredAt);
    if (!assignment || attempt.ownership !== 'held' || !['idle', 'executing'].includes(attempt.state) || run.status !== 'running' || run.desiredState !== 'running' || run.sourceConflict) throw new Conflict('Only an active research agent may submit an artifact.');
    const reasons: string[] = [];
    if (!value.limitations?.trim()) reasons.push('Limitations are required and cannot be empty.');
    const coverage = value.coverage ?? [];
    if (!coverage.length) reasons.push('Criterion coverage is required.');
    const headings = new Set(value.body.split(/\r?\n/).flatMap(line => { const match = /^#{1,4}\s+(.+?)\s*#*\s*$/.exec(line); return match ? [normalize(match[1])] : []; }));
    for (const entry of coverage) {
      if (run.taskSnapshot.criteria[entry.criterionId] === undefined) reasons.push(`Criterion ${entry.criterionId} is not in accepted revision ${run.taskRevision}.`);
      if (!headings.has(normalize(entry.section))) reasons.push(`Criterion ${entry.criterionId}: heading "${entry.section}" is absent from the artifact.`);
    }
    if (new Set(coverage.map(item => item.criterionId)).size !== coverage.length) reasons.push('Criterion coverage entries must be distinct.');
    const citations: ResearchArtifact['citations'] = [];
    for (const item of value.citations) {
      let reason: string | undefined;
      if (!run.researchSource) reason = 'No repository was pinned for this run.';
      else if (item.commit !== run.researchSource.commit) reason = `Citation commit is not the pinned commit ${run.researchSource.commit}.`;
      else {
        try {
          const read = await this.source.readResearch(run.id, item.path);
          const lines = read.content.replace(/\r\n/g, '\n').split('\n');
          if (lines.at(-1) === '') lines.pop();
          if (item.startLine < 1 || item.endLine < item.startLine || item.endLine > lines.length) reason = `Line range ${item.startLine}-${item.endLine} is outside ${item.path} (${lines.length} lines).`;
          else if (normalize(item.excerpt) !== normalize(lines.slice(item.startLine - 1, item.endLine).join('\n'))) reason = `Excerpt does not match ${item.path}:${item.startLine}-${item.endLine}.`;
        } catch (error) { reason = error instanceof Error ? error.message : 'Pinned source is unavailable.'; }
      }
      citations.push({ ...item, verified: !reason, ...(reason ? { reason } : {}) });
      if (reason) reasons.push(`Citation ${citations.length}: ${reason}`);
    }
    if (run.researchSource && !citations.length) reasons.push('A repository-backed artifact needs at least one verified source citation.');
    const model = attemptModel(assignment, attempt);
    const artifact: ResearchArtifact = { id: randomUUID(), runId: run.id, assignmentId: assignment.id, attemptId, toolCallId,
      model: { requested: model.requested, selector: model.resolved.selector, provider: model.resolved.provider, id: model.resolved.id, effort: model.resolved.effort, fallbackFrom: model.fallbackFrom }, requirementRevision: run.taskRevision,
      pinnedCommit: run.researchSource?.commit, title: value.title, kind: value.kind, body: this.evidence.artifact(value.body), citations,
      limitations: value.limitations ?? '', coverage, state: reasons.length ? 'rejected' : 'verified', reasons, createdAt: new Date().toISOString() };
    return this.store.saveResearchArtifact(artifact);
  }

  requestCompletion(attemptId: string, toolCallId: string, input: unknown) {
    z.object({}).strict().parse(input);
    const attempt = this.store.getAttempt(attemptId); const run = this.store.getRun(attempt.runId);
    const assignment = this.store.runtime(run.id).assignments.find(item => item.coordinator && item.currentAttemptId === attemptId && !item.retiredAt);
    if (run.purpose !== 'research' || run.status !== 'running' || run.desiredState !== 'running' || !assignment || attempt.ownership !== 'held') throw new Conflict('Only the active research coordinator may request completion.');
    const result = this.read(run.id);
    if (!result.assessment.ready) throw new Conflict(`Research outcome is incomplete: ${result.assessment.blockers.join(' ')}`);
    return this.store.requestResearchCompletion(run.id, attemptId, toolCallId);
  }
}
