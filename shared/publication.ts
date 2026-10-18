import type { BitbucketComment, BitbucketObservation, BitbucketRepository } from '../server/bitbucket-client';
import type { RepositoryRevision } from './repository';
export type DeliveryState = 'pending' | 'sending' | 'uncertain' | 'confirmed' | 'blocked';
export interface HostedPolicy { statusKeys: string[]; pipelineSteps: string[]; optionalSteps: string[] }
export interface FeedbackObservation {
  state: 'complete' | 'partial' | 'unavailable'; observedAt: string; reason?: string;
  /** Absence from a later response cannot prove deletion; only provider deleted=true can. */
  comments: (BitbucketComment & { currentness: 'known' | 'unknown' | 'conflict' })[];
}
export interface FeedbackContinuation {
  id: string; requestedAt: string; publicationId: string; repository: BitbucketRepository; prId: number; branch: string;
  sourceCandidateId: string; sourceHead: string; feedback: FeedbackObservation;
  state: 'queued' | 'starting' | 'handed_off' | 'held' | 'blocked' | 'cancelled' | 'no_change' | 'candidate'; candidateId?: string;
  /** A closed PR or changed owned identity makes this captured feedback terminal; unknown delivery remains recoverable. */
  holdReason?: string;
}
export interface PublicationRecord {
  id: string; runId: string; attemptId: string; candidateId: string; repositoryPath: string; repository: BitbucketRepository;
  branch: string; target: string; revision: RepositoryRevision; title: string; description: string; policy: HostedPolicy; createdAt: string;
  /** Reviewer-facing account of the change, written by the coordinator; Crewroom appends its evidence section. */
  summary?: string;
  push: { state: DeliveryState; expectedHead: string | null };
  /** `describedSha256`: the description last written to the PR, so a changed description is written once. */
  /** `providerState`: the PR's state in its latest read from Bitbucket, kept even when hosted observation fails. */
  pr: { state: DeliveryState; id?: number; url?: string; describedSha256?: string; providerState?: 'OPEN' | 'MERGED' | 'DECLINED' | 'SUPERSEDED' };
  /** Candidates this publication carried before a target refresh moved it to the current one. */
  history?: { candidateId: string; revision: RepositoryRevision; pushedHead?: string; supersededAt: string }[];
  /** Provider-observed merge of this PR at the published head, by the operator. Crewroom never merges. */
  merged?: { commit: string; observedAt: string };
  /** A confirmed revision/requirements change or failing required check revoked readiness; unavailable observations alone do not. */
  reviewInvalidated?: boolean;
  remoteTargetHead?: string; remoteSourceHead?: string; observation?: BitbucketObservation; lastAttemptAt?: string; error?: string; ready: boolean; blockers: string[];
  feedback?: FeedbackObservation;
}
