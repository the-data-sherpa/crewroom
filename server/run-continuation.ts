import type { Role } from '../shared/types';
import { continuationRequest } from './continuation-request';
import { RepositoryWork } from './repository-work';
import { Workbench } from './workbench';

/** Explicit operator intent; the scheduler owns later admission and re-verification. */
export class RunContinuation {
  constructor(private store: Workbench, private work: RepositoryWork) {}
  async queue(runId: string, input: unknown, roles: Role[]) {
    const value = continuationRequest.parse(input);
    const replay = this.store.continuationReplay(runId, value); if (replay) return replay;
    try {
      const source = await this.work.verifyCancelledWork(runId);
      return this.store.queueCancelledContinuation(runId, value, roles, source.work);
    } catch (error) {
      // A concurrent identical request may have committed while this request was observing Git.
      const replay = this.store.continuationReplay(runId, value); if (replay) return replay;
      throw error;
    }
  }
}
