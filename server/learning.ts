import { z } from 'zod';
import { Conflict, Workbench } from './workbench';
import { RoleConflict, type RoleCatalog } from './roles';
import type { LearningProposal } from '../shared/learning';

const proposalSchema = z.object({
  target: z.object({ kind: z.enum(['role', 'project']), roleId: z.string().min(1).max(64).optional() }).strict()
    .refine(target => (target.kind === 'role') === (target.roleId !== undefined), 'A role target names roleId; a project target does not.'),
  change: z.string().trim().min(1).max(12000),
  rationale: z.string().trim().min(1).max(4000),
  evidence: z.array(z.object({ kind: z.enum(['check', 'review', 'steering', 'check_policy']), id: z.string().min(1).max(200) }).strict()).min(1).max(20),
}).strict();
const decisionSchema = z.object({ decision: z.enum(['approve', 'reject']), reason: z.string().trim().min(1).max(2000), requestId: z.uuid() }).strict();

/**
 * Coordinator-proposed, operator-decided updates to reusable role instructions or a repository's project
 * guidance. A proposal cites the run's recorded evidence and names the version it changes. Approval creates
 * a new version; assignments already admitted keep their snapshots, and nothing here touches models,
 * enabled roles, checks or readiness policy.
 */
export class Learning {
  constructor(private store: Workbench, private roles: RoleCatalog) {}

  propose(attemptId: string, toolCallId: string, input: unknown): LearningProposal {
    const value = proposalSchema.parse(input); z.string().min(1).max(200).parse(toolCallId);
    // A replayed tool call returns its saved proposal before current role or guidance versions are consulted.
    const saved = this.store.learningProposals().find(item => item.attemptId === attemptId && item.toolCallId === toolCallId);
    if (saved) {
      const same = saved.change === value.change && saved.rationale === value.rationale && JSON.stringify(saved.evidence) === JSON.stringify(value.evidence)
        && saved.target.kind === value.target.kind && (saved.target.kind !== 'role' || saved.target.roleId === value.target.roleId);
      if (!same) throw new Conflict('Learning proposal identity has different content.');
      return saved;
    }
    const run = this.store.getRun(this.store.getAttempt(attemptId).runId);
    const record = this.store.codingRecord(run.id); const steering = this.store.steering(run.id);
    for (const item of value.evidence) {
      const recorded = item.kind === 'check' ? record?.checks.some(check => check.id === item.id)
        : item.kind === 'review' ? record?.reviews.some(review => review.id === item.id)
          : item.kind === 'steering' ? steering.some(message => message.id === item.id)
            : record?.checkPolicyChanges?.some(change => change.requestId === item.id);
      if (!recorded) throw new Conflict(`Evidence ${item.kind} ${item.id} is not recorded in this run.`);
    }
    let target: LearningProposal['target']; let baseVersion: number;
    if (value.target.kind === 'role') {
      const roleId = value.target.roleId!;
      const role = this.roles.read().find(item => item.id === roleId);
      if (!role) throw new Conflict('The proposed role does not exist.');
      if (role.instructions === value.change) throw new Conflict('The proposal does not change the role instructions.');
      target = { kind: 'role', roleId }; baseVersion = role.revision ?? 1;
    } else {
      const repositoryId = run.taskSnapshot.repositoryId;
      const repository = repositoryId && this.store.snapshot().repositories.find(item => item.id === repositoryId);
      if (!repositoryId || !repository) throw new Conflict('This run has no configured repository for project guidance.');
      if (repository.guidance?.at(-1)?.text === value.change) throw new Conflict('The proposal does not change the project guidance.');
      target = { kind: 'project', repositoryId }; baseVersion = repository.guidance?.at(-1)?.version ?? 0;
    }
    return this.store.proposeLearning({ runId: run.id, attemptId, toolCallId, target, change: value.change, baseVersion, rationale: value.rationale, evidence: value.evidence });
  }

  decide(id: string, input: unknown): LearningProposal {
    const value = decisionSchema.parse(input);
    const proposal = this.store.getLearningProposal(id);
    const status = value.decision === 'approve' ? 'approved' : 'rejected';
    // An approval whose role save failed is retried, not decided again.
    if (proposal.status === 'approved' && proposal.decision?.pendingRoleSave && status === 'approved') return this.applyRole(proposal);
    if (proposal.target.kind === 'role' && proposal.status === 'pending' && status === 'approved') {
      const roleId = proposal.target.roleId;
      const role = this.roles.read().find(item => item.id === roleId);
      if (!role) throw new Conflict('The proposed role no longer exists.');
      if ((role.revision ?? 1) !== proposal.baseVersion) throw new Conflict('The role changed since this proposal. Ask the coordinator for a new proposal.');
      // The approval is durable before the role catalog changes, so an unapproved change can never take effect.
      return this.applyRole(this.store.decideLearning(id, { requestId: value.requestId, reason: value.reason, status, pendingRoleSave: true }));
    }
    const decided = this.store.decideLearning(id, { requestId: value.requestId, reason: value.reason, status });
    return decided.decision?.pendingRoleSave ? this.applyRole(decided) : decided;
  }

  /** Complete approved role changes interrupted before the catalog save; run at startup. */
  reconcile() {
    for (const proposal of this.store.learningProposals()) if (proposal.decision?.pendingRoleSave) this.applyRole(proposal);
  }

  /** Save an approved role change once, identified by its proposal, then record the saved revision. */
  private applyRole(proposal: LearningProposal): LearningProposal {
    if (proposal.target.kind !== 'role') return proposal;
    const roleId = proposal.target.roleId;
    const role = this.roles.read().find(item => item.id === roleId);
    if (!role) return this.store.completeLearningApproval(proposal.id, { notApplied: 'The role no longer exists.' });
    if (role.learningProposalId !== proposal.id) {
      if ((role.revision ?? 1) !== proposal.baseVersion) return this.store.completeLearningApproval(proposal.id, { notApplied: 'The role changed before the approved version was saved. Ask the coordinator for a new proposal.' });
      try {
        this.roles.update(roleId, { ...role, revision: proposal.baseVersion, instructions: proposal.change }, { learningProposalId: proposal.id });
      } catch (error) {
        if (error instanceof RoleConflict) return this.store.completeLearningApproval(proposal.id, { notApplied: 'The role changed before the approved version was saved. Ask the coordinator for a new proposal.' });
        throw error;
      }
    }
    return this.store.completeLearningApproval(proposal.id, { appliedVersion: this.roles.read().find(item => item.id === roleId)!.revision ?? 1 });
  }
}
