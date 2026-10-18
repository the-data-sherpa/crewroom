import { z } from 'zod';
import { briefLimits } from './types';

/** Operator and coordinator request shape, independent of store and admission. */
export const briefRequest = {
  criteria: z.array(z.object({ index: z.number().int().nonnegative(), obligation: z.string().trim().min(1).max(2000), checkIds: z.array(z.string().min(1).max(200)).max(briefLimits.checkIds).default([]) }).strict()).max(100).default([]),
  dependsOn: z.array(z.string().min(1).max(100)).max(briefLimits.dependsOn).default([]),
};
