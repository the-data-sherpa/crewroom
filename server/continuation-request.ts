import { z } from 'zod';

export const continuationRequest = z.object({ requestId: z.string().uuid(), revision: z.number().int().positive(), roleId: z.string().min(1) }).strict();
