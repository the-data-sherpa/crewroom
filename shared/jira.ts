export interface JiraSource {
  siteUrl: string; cloudId: string; filterId: string; assigneeAccountId: string; name: string;
  inProgressStatusId?: string; inProgressTransitionId?: string;
  /** Operator policy: move to review when the PR is created, and close after a verified merge. */
  reviewStatusId?: string; reviewTransitionId?: string; doneStatusId?: string; doneTransitionId?: string;
}
export interface JiraSyncState {
  source: JiraSource; state: 'not_synced' | 'connected' | 'unavailable'; taskIds: string[];
  lastSyncedAt?: string; lastAttemptAt?: string; error?: string;
}
export interface JiraUpdate {
  id: string; runId: string; attemptId: string; siteUrl: string; issueId: string; eventKey: string;
  /** `in_review` and `done` are controller transitions derived from publication receipts; agents cannot request them. */
  kind: 'milestone' | 'blocker' | 'in_progress' | 'in_review' | 'done' | 'evidence' | 'pr_link' | 'research_outcome'; body?: string; candidateId?: string;
  state: 'pending' | 'sending' | 'uncertain' | 'confirmed' | 'blocked';
  createdAt: string; lastAttemptAt?: string; externalId?: string; reason?: string;
  statusId?: string; transitionId?: string;
}
