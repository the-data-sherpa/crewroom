import { z } from 'zod';

/** Stable operator request, independent of coordinator attempts and role revisions. */
export const insertionRequest = z.object({
  requestId: z.string().uuid(), roleId: z.string().min(1).max(100),
  input: z.string().trim().min(1).max(40000),
  dependsOn: z.array(z.string().min(1).max(100)).max(10).default([]),
  criteria: z.array(z.number().int().nonnegative()).max(100).default([]),
}).strict();
export type InsertionRequest = z.infer<typeof insertionRequest>;
