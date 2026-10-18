import { z } from 'zod';

const pageSchema = z.object({
  after: z.number().int().nonnegative(), hasMore: z.boolean(),
  events: z.array(z.object({
    cursor: z.number().int().positive(), attemptId: z.string(), sequence: z.number().int().positive(),
    event: z.object({ type: z.string(), id: z.string().optional(), toolName: z.string().optional(), isError: z.boolean().optional(),
      identity: z.unknown().optional(), message: z.object({ role: z.string().optional() }).passthrough().optional() }).passthrough(),
  })),
});

/** Scripted drivers retain their local assertions; the HTTP snapshot never carries event history. */
export function createRuntimeObserver<T extends object>(request: (path: string) => Promise<T>) {
  const cursors = new Map<string, number>();
  const histories = new Map<string, z.infer<typeof pageSchema>['events']>();
  return async (runId: string) => {
    const snapshot = await request(`/runs/${runId}/runtime`);
    let after = cursors.get(runId) ?? 0;
    const events = histories.get(runId) ?? [];
    let more;
    do {
      const page = pageSchema.parse(await request(`/runs/${runId}/events?after=${after}&limit=100`));
      if (page.hasMore && page.after <= after) throw new Error('Runtime event cursor made no progress.');
      events.push(...page.events); after = page.after; more = page.hasMore;
    } while (more);
    cursors.set(runId, after); histories.set(runId, events);
    return { ...snapshot, events };
  };
}
