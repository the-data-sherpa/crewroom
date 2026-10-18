import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

export interface SavedInput { id: string; message?: string; disposition: string }
export function savedInputs(root: string): SavedInput[] {
  const path = join(root, 'inputs.jsonl');
  if (!existsSync(path)) return [];
  const inputs = new Map<string, SavedInput>();
  for (const line of readFileSync(path, 'utf8').split('\n').filter(Boolean)) {
    // A partial/corrupt record is an uncertain dispatch, never proof of absence.
    const value = JSON.parse(line);
    if (typeof value.id !== 'string' || typeof value.disposition !== 'string') throw new Error('Input journal is incomplete; reconcile it before continuation.');
    const previous = inputs.get(value.id);
    if (previous && ['completed', 'error', 'aborted'].includes(previous.disposition) && value.disposition === 'received') continue;
    inputs.set(value.id, { ...previous, ...value });
  }
  return [...inputs.values()];
}
