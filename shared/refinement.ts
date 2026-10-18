export interface RefinementProposal {
  id: string;
  runId: string;
  taskId: string;
  attemptId: string;
  toolCallId: string;
  baseRevision: number;
  sourceRevision: number;
  brief: string;
  criteria: string[];
  decisions: string[];
  unresolvedQuestions: string[];
  status: 'pending' | 'accepted' | 'discarded';
  createdAt: string;
  decision?: { requestId: string; at: string; reason?: string; revision?: number; brief?: string; criteria?: string[] };
}
