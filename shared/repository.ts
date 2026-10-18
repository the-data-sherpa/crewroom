import type { CheckedProcess } from '../server/runtime/processes';
import type { Repository } from './types';
export interface RepositoryRevision {
  base: string; head: string; tree: string; worktreeHash: string; dirty: boolean; requirementsHash: string; instructionsHash: string;
}
export interface RepositoryCheckpoint {
  id: string; at: string; reason: 'prepared' | 'writer_started' | 'interrupted' | 'completed' | 'refreshed'; attemptId?: string; revision: RepositoryRevision;
}
/**
 * Target verification history. Each new publication-backed request pins a distinct, immutable fetched
 * target ref; only a successful rebase changes the accepted work.base and work.targetRef together.
 * New ordinary and reasoned no-ops both record completed, candidate-bound receipts without changing Git.
 * Historical records may still name the legacy shared ref. A safely interrupted ordinary rebase was undone;
 * an interrupted reasoned retry remains an operator hold with no invented resolution workspace.
 */
export interface TargetRefresh {
  requestId: string; at: string; fromBase: string; fromHead: string; targetRef: string; toBase: string;
  sourceCandidateId?: string; // Missing only on historical receipts; ambiguous legacy sources require operator recovery.
  state: 'rebasing' | 'completed' | 'conflicted' | 'interrupted'; toHead?: string; conflicts?: string[]; candidateId?: string;
  holdReason?: 'interrupted_retry'; // Workspace-less, safely undone superseding retry; not a Git conflict or verification receipt.
  rebaseRecovery?: 'rebase_stopped' | 'undo_started' | 'restored'; // Durable positive observation; absence is not proof the Git child stopped.
  resolutionWorkspace?: TargetRefreshWorkspace;
  supersedes?: { requestId: string; reason: string }; supersededBy?: { requestId: string; reason: string; at: string };
}
export interface TargetConflictContinuation {
  attemptId: string; toolCallId: string; requestId: string; path: string; branch: string; sourceHead: string; paths: string[];
}
export interface TargetConflictContinuationResult { state: 'conflicted' | 'ready' | 'operator_required'; conflicts: string[]; head?: string }
/** `conflicted` and `ready` live in the isolated worktree; promotion touches the owned branch only after a stopped completion. */
export interface TargetRefreshWorkspace {
  path: string; branch: string; state: 'preparing' | 'conflicted' | 'continuing' | 'operator_required' | 'ready' | 'promoting' | 'promoted'; gitFile?: string; toHead?: string; operationId?: string;
  continuation?: TargetConflictContinuation; // A missing identity on a legacy continuing workspace is an operator-only hold.
  legacyOwnershipUnknown?: true; // v22 had no proof that a conflict operation or unpromoted workspace stopped.
  continuationReceipts?: (TargetConflictContinuation & { result: TargetConflictContinuationResult })[];
  /** Immutable for new promoting intents; absent before admission and on historical versionless promoting records. */
  promotionProcess?: CheckedProcess & { command: string; args: string[]; cwd: string; env: Record<string, string> };
  requirementsHash?: string; instructionsHash?: string;
}
export interface RepositoryWorkRecord {
  /** Historical owner after an atomic handoff to a linked successor. */
  transferredTo?: string;
  /** A moved target is verified through the existing refresh flow after fresh implementation; targetRef pins the old base meanwhile. */
  continuationTargetRef?: string;
  runId: string; repository: Repository; commonDir: string; path: string; branch: string; targetRef: string;
  mode: 'new' | 'continue'; base: string; startHead: string; requirementsHash: string;
  state: 'preparing' | 'ready'; gitFile?: string;
  instructions: { path: string; body: string; sha256: string }[];
  writer?: { attemptId: string; acquiredAt: string };
  checkpoints: RepositoryCheckpoint[];
  refreshes?: TargetRefresh[];
}
export interface RepositoryStatus extends RepositoryWorkRecord {
  revision: RepositoryRevision;
  drift: { base: boolean; head: boolean; worktree: boolean; requirements: boolean; instructions: boolean };
}
