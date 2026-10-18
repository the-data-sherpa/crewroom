import type { EvidenceArtifact } from './evidence';

export interface ResearchCitation {
  path: string; commit: string; startLine: number; endLine: number; excerpt: string;
}
export interface ResearchArtifact {
  id: string; runId: string; assignmentId: string; attemptId: string; toolCallId: string;
  title: string; kind: 'research' | 'design' | 'documentation-draft'; body: EvidenceArtifact;
  model: { requested: { model: string; effort: string }; selector: string; provider: string; id: string; effort: string; fallbackFrom?: string }; requirementRevision: number; pinnedCommit?: string;
  citations: (ResearchCitation & { verified: boolean; reason?: string })[];
  limitations: string; coverage: { criterionId: number; section: string }[];
  state: 'verified' | 'rejected'; reasons: string[]; supersededBy?: string; createdAt: string;
}
