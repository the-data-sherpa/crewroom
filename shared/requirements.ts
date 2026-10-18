import { createHash } from 'node:crypto';
import type { Task } from './types';

/** Preserve the pre-adoption hash for retained coding evidence; adopted revisions get a new identity. */
export function requirementsHash(task: Task, adoptedRevision?: number) {
  return createHash('sha256').update(JSON.stringify({ title: task.title, description: task.description,
    brief: task.brief, criteria: task.criteria, repositoryId: task.repositoryId,
    ...(adoptedRevision === undefined ? {} : { adoptedRevision }) })).digest('hex');
}
