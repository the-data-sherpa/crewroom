import { join } from 'node:path';
import { writeFileSync } from 'node:fs';
import { Workbench } from '../../server/workbench';
import { defaultRoles } from '../../server/roles';
import { resolveAssignment } from '../../server/runtime/model-policy';
import type { RuntimeConfig } from '../../server/runtime/supervisor';
import { seedRepository, saveRepositoryFixture } from './repository';

/** Queued local work and a model catalog fixture; no model requests or live providers. */
export function insertionFixture(root: string) {
  seedRepository(root);
  const path = join(root, 'workflow.sqlite'); const store = new Workbench(path);
  const repository = saveRepositoryFixture(store, { name: 'Insertion fixture', path: root, targetBranch: 'main' });
  const task = store.createTask({ title: 'Insert a specialist', description: '', repositoryId: repository.id });
  store.refine(task.id, { revision: 1, sourceRevision: 1, brief: 'Inspect the accepted work', criteria: ['Explain the result'] });
  const run = store.queue(task.id, { revision: 2 });
  const roles = defaultRoles.map(role => ({ ...role, model: 'fixture/model', effort: 'low' as const, revision: 1 }));
  roles.find(role => role.id === 'sre')!.enabled = false;
  const rolePath = join(root, 'roles.json'); writeFileSync(rolePath, JSON.stringify(roles));
  const role = roles.find(role => role.id === 'tech-writer')!;
  const catalog = [{ provider: 'fixture', id: 'model', api: 'openai-responses', inputs: ['text'], efforts: ['low'], tools: true }];
  const model = resolveAssignment(role, catalog, {}, {});
  const runtimeConfig: RuntimeConfig = { root: join(root, 'hosts'), socket: join(root, 'tmux.sock'), ompBinary: '/unused',
    mail: { url: 'http://127.0.0.1:9/mcp/', token: 'fixture', projectRoot: root, retentionSeconds: 86400, atcDisabled: true },
    catalog, policy: {}, models: { providers: {} } };
  return { path, store, run, role, roles, model, rolePath, runtimeConfig };
}
