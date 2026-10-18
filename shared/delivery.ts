/** A controller observation, distinct from operator steering and peer mail. */
export interface DeliveryNotice {
  id: string; runId: string; createdAt: string;
  payload: { kind: string; subjectId: string; [key: string]: unknown };
  disposition: 'pending' | 'dispatched' | 'received';
  attemptId?: string;
}
