import { z } from 'zod';

/** The same repository selection is validated when a coding plan is queued and when Git work is prepared. */
export const repositoryRequestSchema = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('new') }).strict(),
  z.object({ mode: z.literal('continue'), branch: z.string().min(1).max(200).refine(value => !value.startsWith('-'), 'Use a branch name, not a Git option.'), expectedHead: z.string().regex(/^[a-f0-9]{40,64}$/) }).strict(),
]);
