import { attemptModel } from './runtime/types';
import { NotFound, Workbench } from './workbench';
import { RepositoryWork } from './repository-work';
import { Evidence } from './evidence';
import type { Research } from './research';
import type { Publication } from './publication';
import type { RepositoryStatus } from '../shared/repository';
import { readRunUsage } from './run-usage';

/** Read-only, run-scoped presentation of verified evidence. */
export class Results {
  constructor(private store: Workbench, private work: RepositoryWork, private evidence: Evidence, private publication?: Publication, private research?: Research) {}
  async read(runId: string, observed?: RepositoryStatus | Error) {
    const runtime = this.store.runtime(runId);
    const run = this.store.getRun(runId); const record = this.store.codingRecord(runId);
    if (run.purpose === 'research') {
      const research = this.research!.read(runId);
      return { configured: false, repository: undefined, criteria: research.criteria, assessment: { ready: false, candidate: undefined, blockers: [], confirmedInvalidation: false },
        candidates: [], checks: [], reviews: [], requiredChecks: [], checkOperations: [], checkPolicyChanges: [], completions: [], refreshes: [], refreshable: false,
        assignments: runtime.assignments.map(item => ({ id: item.id, name: item.name, model: item.model.resolved.selector, retiredAt: item.retiredAt })),
        attempts: runtime.attempts.map(item => ({ id: item.id, assignmentId: item.assignmentId, sessionId: item.sessionId, model: attemptModel(runtime.assignments.find(assignment => assignment.id === item.assignmentId)!, item) })),
        usage: readRunUsage(this.store, runId), publication: { record: undefined, available: false, reason: 'Research has no PR publication.' }, research };
    }
    const assessment = record ? await this.evidence.assess(runId, observed) : { ready: false, candidate: undefined, blockers: ['Repository work has not been prepared.'], confirmedInvalidation: false };
    const repository = this.store.repositoryWork(runId); const refreshes = repository?.refreshes ?? [];
    const unresolved = this.work.unresolvedConflict(runId);
    const operatorRetry = unresolved?.state === 'conflicted' && !unresolved.supersededBy && (!unresolved.resolutionWorkspace || unresolved.resolutionWorkspace.state === 'operator_required');
    const attempts = this.store.runtime(runId).attempts;
    const ownershipStopped = !repository?.writer && attempts.every(item => item.state === 'admitted' || item.ownership === 'stopped');
    const publication = this.store.publication(runId);
    const publicationBusy = !!publication && (['sending', 'uncertain', 'blocked'].includes(publication.push.state)
      || ['sending', 'uncertain', 'blocked'].includes(publication.pr.state));
    const refreshable = !!record?.candidates.length && ['paused', 'attention_required'].includes(run.status) && ownershipStopped
      && !record.completions?.some(item => item.state === 'requested') && !publicationBusy
      && !refreshes.some(item => item.state === 'rebasing' || ['preparing', 'continuing'].includes(item.resolutionWorkspace?.state ?? '')) && (!unresolved || operatorRetry);
    return { configured: !!record, repository: repository?.repository, criteria: run.taskSnapshot.criteria,
      assessment,
      candidates: record?.candidates ?? [], checks: record?.checks ?? [], reviews: record?.reviews ?? [], requiredChecks: record?.requiredChecks ?? [],
      checkOperations: record?.checkOperations ?? [],
      checkPolicyChanges: record?.checkPolicyChanges ?? [],
      completions: record?.completions ?? [], refreshes,
      feedbackContinuations: run.feedbackContinuations ?? [],
      /** A normal refresh needs a stopped run; an operator hold additionally needs an explicit reasoned retry. */
      refreshable,
      assignments: runtime.assignments.map(item => ({ id: item.id, name: item.name, model: item.model.resolved.selector, retiredAt: item.retiredAt })),
      attempts: runtime.attempts.map(item => ({ id: item.id, assignmentId: item.assignmentId, sessionId: item.sessionId, model: attemptModel(runtime.assignments.find(assignment => assignment.id === item.assignmentId)!, item) })),
      research: undefined,
      usage: readRunUsage(this.store, runId),
      publication: await this.publication?.view(runId, assessment) ?? { record: undefined, available: false, reason: 'PR publication is unavailable. Local validation can complete; no PR has been created.' },
    };
  }
  artifact(runId: string, id: string) {
    const record = this.store.codingRecord(runId);
    const research = this.store.researchArtifacts(runId).find(item => item.body.id === id);
    if (research) return { ...research.body, content: this.evidence.readArtifact(research.body) };
    const artifact = [...(record?.checks.flatMap(item => [item.output, item.source]) ?? []), ...(record?.reviews.map(item => item.artifact) ?? [])].find(item => item.id === id);
    if (!artifact) throw new NotFound('Evidence artifact is not admitted to this run.');
    return { ...artifact, content: this.evidence.readArtifact(artifact) };
  }
  async diff(runId: string, candidateId: string) {
    const candidate = this.store.codingRecord(runId)?.candidates.find(item => item.id === candidateId);
    if (!candidate) throw new NotFound('Candidate is not admitted to this run.');
    return this.work.diffRevision(runId, candidate.revision);
  }
}
