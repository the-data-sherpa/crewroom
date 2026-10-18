/** A recorded fact from the run that a learning proposal cites. */
export interface LearningEvidence { kind: 'check' | 'review' | 'steering' | 'check_policy'; id: string }

/**
 * A coordinator's proposed update to a reusable role or a repository's project guidance, derived from a
 * run's recorded evidence. Only the operator's decision applies it; historical assignments keep their
 * snapshots, and the change cannot touch models, enabled state, checks or readiness policy.
 */
export interface LearningProposal {
  id: string; runId: string; attemptId: string; toolCallId: string;
  target: { kind: 'role'; roleId: string } | { kind: 'project'; repositoryId: string };
  /** Role: the complete replacement instructions. Project: the complete replacement guidance text. */
  change: string;
  /** The version the change was proposed against: the role revision, or the project guidance version (0 = none). */
  baseVersion: number;
  rationale: string; evidence: LearningEvidence[];
  /** `superseded`: approved, but the role changed before the approved version could be saved; nothing was applied. */
  status: 'pending' | 'approved' | 'rejected' | 'superseded'; createdAt: string;
  /** `pendingRoleSave`: an approved role change recorded durably but not yet saved to the role catalog; startup completes it. */
  decision?: { requestId: string; at: string; reason: string; appliedVersion?: number; pendingRoleSave?: boolean; notApplied?: string };
}

/** One operator-approved version of a repository's project guidance. */
export interface ProjectGuidance { version: number; text: string; approvedAt: string; proposalId: string }
