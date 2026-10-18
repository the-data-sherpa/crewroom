import type { RepositoryRevision } from './repository';
export interface CheckDefinition { id: string; image: string; command: string; args: string[]; timeoutMs: number; memoryMiB?: number; tmpExecutable?: boolean; required: boolean }
export interface EvidenceArtifact { id: string; sha256: string; bytes: number }
/** A candidate names its verified source lineage; feedback work cannot inherit the predecessor's check receipts. */
export interface Candidate { id: string; runId: string; revision: RepositoryRevision; implementationAttempts: string[]; createdAt: string;
  refreshedFrom?: string; feedbackFrom?: { continuationId: string; sourceCandidateId: string } }
export interface CheckEvidence {
  id: string; candidateId: string; attemptId: string; checkId: string; definition: CheckDefinition;
  environment: { image: string; network: 'none'; platform: string; memoryMiB?: number; tmpExecutable?: boolean }; exitCode: number; timedOut: boolean;
  output: EvidenceArtifact; source: EvidenceArtifact; sourceIntact: boolean; started: boolean; completed: boolean; completedAt: string;
}
export interface ReviewEvidence {
  checkPolicyRevision?: number;
  id: string; candidateId: string; attemptId: string; assignmentId: string; sessionId: string;
  verdict: 'pass' | 'changes_requested'; summary: string;
  findings: { path?: string; line?: number; severity: 'blocker' | 'major' | 'minor'; body: string }[];
  coverage: { criterion: number; assessment: string; checkIds: string[] }[];
  artifact: EvidenceArtifact; createdAt: string;
}
export interface EditOperation {
  id: string; attemptId: string; toolCallId: string; input: { path: string; content: string; expectedHash: string | null }; afterHash: string; state: 'pending' | 'applied' | 'rejected'; reason?: string;
  /** SHA-256 of a partial-edit request; the ledger retains the resulting full content in `input`. */
  request?: string;
}
export interface CompletionOperation {
  id: string; attemptId: string; toolCallId: string; summary: string; head: string; worktreeHash: string; state: 'requested' | 'completed' | 'rejected'; candidateId?: string;
  conflictRefreshId?: string;
  rejection?: { code: 'target_changed'; at: string; expectedTarget: string; observedTarget: string; reason: string }
    | { code: 'promotion_interrupted'; at: string; expectedTarget: string; reason: string; observedTarget?: never };
}
export interface CheckOperation {
  id: string; attemptId: string; toolCallId: string; candidateId: string; definition: CheckDefinition; workspace: string; source?: EvidenceArtifact; state: 'preparing' | 'prepared' | 'recorded' | 'rejected'; reason?: string; evidenceId?: string;
}
export interface ImplementationClaim {
  id: string; coordinatorAttemptId: string; toolCallId: string; writerAttemptId: string; instruction: string; candidateId?: string; automatic: boolean; targetRefreshId?: string; feedbackContinuationId?: string;
  /** Only a controller restoration names the stopped attempt whose saved grant is being resumed. */
  restoredFrom?: string;
}
export interface ReviewAssignment {
  id: string; coordinatorAttemptId: string; toolCallId: string; assignmentId: string; candidateId: string; instruction: string;
}
export interface CodingRecord {
  checkPolicyChanges?: { requestId: string; checkId: string; expectedImage: string; image: string; expectedMemoryMiB?: number; memoryMiB?: number; reason: string; revision: number; createdAt: string }[];
  reviewAssignments?: ReviewAssignment[];
  claims?: ImplementationClaim[]; repairAssignmentId?: string;
  checkOperations?: CheckOperation[];
  completions?: CompletionOperation[];
  edits?: EditOperation[];
  runId: string; requiredChecks: CheckDefinition[]; candidates: Candidate[]; checks: CheckEvidence[]; reviews: ReviewEvidence[];
}
