import type { Role } from '../../shared/types';
import type { AssignmentModel } from './model-policy';
import type { AssignmentBrief } from './brief';
import type { InsertionRequest } from '../../shared/insertion';

/** Brief bounds shared by the server schema and the coordinator's host tool schema. */
export const briefLimits = { dependsOn: 10, checkIds: 100 } as const;
/** Largest request body a host accepts; the supervisor checks continuation prompts against it before dispatch. */
export const hostInputLimit = 150000;
/** Backpressure refuses new input without discarding accepted messages or blocking keyed retries. */
export const pendingMessageLimit = 100;

export interface Assignment {
  id: string; runId: string; name: string; role: Role; model: AssignmentModel;
  coordinator: boolean; currentAttemptId: string; mailboxName?: string;
  automaticCycles?: number;
  /** Operator-granted extra automatic repair/replacement cycles beyond the default two. Never removed. */
  repairAllowances?: { id: string; grantedAt: string; reason: string }[];
  retiredAt?: string; retirementReason?: string;
  /** Snapshotted at admission. Absent only for assignments admitted before structured briefs existed. */
  brief?: AssignmentBrief;
  /** Adoption leaves the historical brief intact; only a replacement through the team proposal clears this obligation. */
  rebriefRequired?: { adoptedRevision: number; criterionIndices: number[] };
  /** The operator-approved project guidance version current at admission; later approvals never change it. */
  projectGuidance?: { version: number; text: string };
}
export interface Attempt {
  /** Controller-recorded milestones. Absent for legacy attempts; never inferred from run time. */
  createdAt?: string; startedAt?: string; stoppedAt?: string;
  ownershipNotes?: { at: string; reason: string }[];
  id: string; assignmentId: string; runId: string; generation: number; input: string;
  /** Immutable identity for this attempt. Legacy attempts inherit the unchanged admission identity. */
  model?: AssignmentModel;
  fallback?: { fromAttemptId: string; fromModel: string; reason: string; eventSequence: number };
  state: 'admitted' | 'starting' | 'idle' | 'executing' | 'failed' | 'exited' | 'attention_required';
  runnerKind?: 'omp' | 'codex' | 'claude-code'; runnerPid?: number;
  observedModel?: string;
  hostPid?: number; ompPid?: number; sessionId?: string; hostUrl?: string; pane?: string;
  activeInputMessageId?: string;
  ownership?: 'held' | 'stopped' | 'uncertain';
  ownershipReason?: string;
  previousAttemptId?: string;
  recoveryMode?: 'session' | 'replacement';
  sessionDirectory?: string;
  runnerCwd?: string;
}
export interface PeerMessage {
  id: string; runId: string; attemptId: string; toolCallId: string; from: string; to: string;
  body: string; replyTo?: string; createdAt: string; transportId?: number;
  disposition: 'pending' | 'stored' | 'accepted' | 'dispatched' | 'received' | 'uncertain';
  recipientAttemptId?: string;
}
export interface ProposedSpecialist { name: string; role: Role; model: AssignmentModel; input: string; brief: AssignmentBrief }
export interface TeamProposal {
  id: string; runId: string; attemptId?: string; toolCallId: string;
  /** Missing origin identifies a legacy coordinator proposal. */
  origin?: 'operator'; request?: InsertionRequest; createdAt?: string; requirementRevision?: number;
  specialists: ProposedSpecialist[]; retirements?: { assignmentId: string; reason: string }[]; status: 'pending' | 'applied' | 'rejected'; assignmentIds?: string[]; reason?: string;
}

/** Automatic repair/replacement cycles an assignment may consume: two, plus explicit operator allowances. */
export const automaticCycleLimit = (assignment: Pick<Assignment, 'repairAllowances'>) => 2 + (assignment.repairAllowances?.length ?? 0);
export const cyclesExhausted = (assignment: Pick<Assignment, 'repairAllowances'>, kind: string) => {
  const limit = automaticCycleLimit(assignment);
  return `${limit === 2 ? 'The two' : `All ${limit}`} automatic ${kind} cycles are exhausted. Allow another cycle explicitly to continue.`;
};

/** Historical attempts keep their actual model when a later attempt uses a configured fallback. */
export const attemptModel = (assignment: Pick<Assignment, 'model'>, attempt: Pick<Attempt, 'model'>) => attempt.model ?? assignment.model;
