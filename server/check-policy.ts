import { z } from 'zod';
import { isAbsolute } from 'node:path';
export const checkSchema = z.object({ id: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/), image: z.string().regex(/^sha256:[a-f0-9]{64}$/),
  command: z.string().refine(isAbsolute), args: z.array(z.string().max(10000)).max(100), timeoutMs: z.number().int().min(100).max(3600000),
  memoryMiB: z.number().int().min(128).max(8192).optional(), tmpExecutable: z.boolean().optional(), required: z.boolean() }).strict();
