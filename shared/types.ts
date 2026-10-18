import type { FeedbackContinuation } from './publication';
import type { JiraSyncState, JiraUpdate } from './jira';
import type { CheckDefinition } from './evidence';
import type { LearningProposal, ProjectGuidance } from './learning';
import type { RefinementProposal } from './refinement';
export type Source = 'sample' | 'local' | 'jira';
export type NoteMode = 'explore' | 'grill' | 'decision';
export const effortLevels = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const;
export type RunStatus = 'awaiting_runner' | 'running' | 'pausing' | 'paused' | 'attention_required' | 'cancelled' | 'closed' | 'completed';

export interface Task {
  id: string;
  key: string;
  title: string;
  description: string;
  jiraStatus: string;
  jiraStatusId?: string;
  sourceUpdatedAt?: string;
  priority: string;
  source: Source;
  sourceUrl?: string;
  sourceId?: string;
  project: string;
  rank: number;
  revision: number;
  brief: string;
  criteria: string[];
  updatedAt: string;
  sourceSite?: string;
  sourceRevision?: number;
  needsReacceptance?: boolean;
  repositoryId?: string;
  /** Local operator declarations; imported Jira data never owns this field. */
  dependsOn?: string[];
  /** Independent optimistic token for local dependency declarations; absent means zero. */
  dependencyRevision?: number;
}

/** Immutable accepted brief; proposalId is set when a refinement decision created it. */
export interface RequirementRevision {
  taskId: string; revision: number; brief: string; criteria: string[]; sourceRevision: number;
  origin: 'operator' | 'jira-reacceptance' | 'refinement'; proposalId?: string;
  decisionNote?: string; createdAt: string;
}

export interface RequirementAdoption {
  requestId: string; fromRevision: number; toRevision: number; reason?: string; at: string;
}

export interface Note { id: string; taskId: string; mode: NoteMode; body: string; createdAt: string; proposalId?: string; revision?: number; kind?: 'decision' | 'unresolved_question' }
export interface Role {
  id: string;
  name: string;
  category: string;
  purpose: string;
  instructions: string;
  model: string;
  effort: 'default' | typeof effortLevels[number];
  enabled: boolean;
  /** Increments on every saved change; assignments snapshot the revision they were given. Absent means 1. */
  revision?: number;
  /** The approved learning proposal that produced this revision, if one did. */
  learningProposalId?: string;
}
/** Captured coding launch intent. Admission still resolves current role definitions and repository ownership. */
export interface LaunchSpec {
  team: { name: string; roleId: string; coordinator: boolean; input: string;
    criteria: { index: number; obligation: string; checkIds: string[] }[]; dependsOn: string[] }[];
  repository: { mode: 'new' } | { mode: 'continue'; branch: string; expectedHead: string };
}


export interface Run {
  id: string;
  taskId: string;
  /** Older runs without purpose are coding runs. */
  purpose?: 'coding' | 'explore' | 'grill' | 'research';
  requestId?: string;
  /** Explicit successor of a terminal cancellation. The source run never resumes. */
  continuedFrom?: { runId: string; expectedHead: string; worktreeHash: string; branch: string; publicationId?: string; prId?: number };
  continuedBy?: string;
  status: RunStatus;
  /** Zero-based local queue order while this coding run has no admitted assignments. */
  position?: number;
  launch?: LaunchSpec;
  /** Durable admission decision; retained after the slot is released. */
  schedule?: { id: string; at: string; position?: number };
  /** Last observed checkout-base eligibility, refreshed by the scheduler before slot claim. */
  dependencyBaseHold?: { merges: string; reason?: string };
  /** Preflight holds recheck on signals; preparation (including legacy unclassified) requires explicit retry. */
  launchHold?: { reason: string; at: string; source?: 'preflight' | 'preparation' };
  slotRelease?: { reason: 'review' | 'cancelled' | 'merged' | 'migration'; at: string; candidateId?: string };
  /** Operator-authorized continuation waiting for the single coding slot. */
  resumeRequest?: { requestedAt: string; state: 'queued' | 'starting' };
  /** Immutable feedback selected by an explicit Resume, retained after the queued control is delivered. */
  feedbackContinuations?: FeedbackContinuation[];
  /** A queued Resume made terminal by a closed PR or changed owned identity, including before any feedback was captured. */
  feedbackResumeHold?: { reason: string; terminal: boolean };
  /** Workspace projection of confirmed readiness invalidation for released review work. */
  readinessHold?: { state: 'pending' | 'held'; candidateId: string; since: string };
  launchCompletedAt?: string;
  /** First recorded execution start; older runs may not have this timestamp. */
  startedAt?: string;
  createdAt: string;
  taskRevision: number;
  taskSnapshot: Task;
  adoptions?: RequirementAdoption[];
  /** Read-only source identity fixed at research admission; never a worktree or writer lease. */
  researchSource?: { repositoryId: string; repositoryRevision?: number; path: string; commit: string };
  researchOutcome?: { revision: number; completedAt: string; requestId: string };
  researchCompletion?: { requestId: string; revision: number };
  roles: Role[];
  stages: string[];
  attentionReason?: string;
  activeMs?: number;
  activeLimitMs?: number;
  activeSince?: number;
  /** Excluded only while provider-confirmed CI is pending and all local execution is settled. */
  activeWait?: { kind: 'hosted-ci'; since: number };
  pauseReason?: 'operator' | 'budget' | 'recovery' | 'source' | 'requirements' | 'review' | 'validation' | 'merged';
  sourceConflict?: string;
  desiredState?: 'running' | 'paused' | 'cancelled';
  controlRevision?: number;
  codingTransition?: { completionId: string; controlRevision: number };
  teamTransition?: { proposalId: string; controlRevision: number };
}
export interface CodingQueue { revision: number; plans: (Run & { holdReason?: string })[]; slot?: { runId: string; key: string; reason: string; decisionId: string; decidedAt: string } }


export interface Repository {
  id: string;
  /** Absent in older databases; treated as revision 1 on first edit. */
  revision?: number;
  retiredAt?: string;
  name: string;
  path: string;
  targetBranch: string;
  branchPrefix?: string;
  requestId?: string;
  checks?: CheckDefinition[];
  /** Operator-approved project guidance versions, newest last. New assignments snapshot the latest. */
  guidance?: ProjectGuidance[];
}

export interface Message {
  id: string;
  runId: string;
  body: string;
  createdAt: string;
  delivery: 'pending' | 'dispatched' | 'received' | 'applied' | 'rejected' | 'not_delivered';
  /** Recorded observations, in order. Missing legacy times remain unknown. */
  transitions?: { state: Message['delivery']; at?: string; reason?: string }[];
  kind?: 'requirements_adoption';
  attemptId?: string;
  inputId?: string;
  dispositionReason?: string;
  dispositionToolCallId?: string;
}

export interface Workspace {
  continuations?: Record<string, { branch?: string; head?: string; prUrl?: string; reason?: string }>;
  jira?: JiraSyncState;
  jiraUpdates?: JiraUpdate[];
  tasks: Task[];
  requirementRevisions: RequirementRevision[];
  notes: Note[];
  refinementProposals: RefinementProposal[];
  runs: Run[];
  /** Prepared coding runs retain their configuration even after the catalog changes. */
  repositorySnapshots?: Record<string, Repository>;
  queue: CodingQueue;
  dependencyStates: Record<string, { state: 'merged' | 'open' | 'closed' | 'unknown' | 'none'; reason?: string }>;
  messages: Message[];
  roles: Role[];
  learning: LearningProposal[];
  repositories: (Repository & { publicationAvailable: boolean })[];
  activity: { runId: string; assignmentName: string; state: string; ompPid?: number }[];
  connections: { jira: boolean; runner: boolean };
}

export const stages = ['Triage', 'Research & impact', 'Implementation', 'Independent review', 'Tests & regressions', 'Documentation', 'Final checks & PR', 'Human review'];
export const runLabels: Record<RunStatus, string> = { awaiting_runner: 'Waiting for runner', running: 'Coordinating', pausing: 'Pausing', paused: 'Paused', attention_required: 'Needs attention', cancelled: 'Cancelled', closed: 'Closed', completed: 'Research completed' };

export function runLabel(run: Run) {
  if (run.desiredState === 'cancelled' && run.status === 'pausing') return 'Cancelling · confirming stop';
  if (run.desiredState === 'cancelled' && run.status === 'attention_required') return 'Cancellation needs attention';
  if (run.status === 'attention_required') return run.resumeRequest ? 'Needs attention · Resume queued' : 'Needs attention';
  if (run.resumeRequest?.state === 'queued') return 'Paused · Resume queued';
  if (run.resumeRequest?.state === 'starting' || (run.status === 'paused' && run.desiredState === 'running')) return 'Resuming';
  if (run.readinessHold) return run.readinessHold.state === 'held' ? 'Paused · readiness unconfirmed' : 'Paused · waiting for coding slot';
  if (run.status === 'paused' && run.pauseReason === 'validation') return 'Paused · validation needs attention';
  if (run.status === 'paused' && run.pauseReason === 'review') return 'Paused · awaiting human review';
  if (run.status === 'pausing' && run.pauseReason === 'review') return 'Pausing · awaiting human review';
  if (run.status === 'paused' && run.pauseReason === 'merged') return 'Paused · PR merged';
  return run.status === 'awaiting_runner' && run.position !== undefined ? 'Queued' : runLabels[run.status];
}
