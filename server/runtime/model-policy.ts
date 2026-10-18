import { effortLevels, type Role } from '../../shared/types';
import { z } from 'zod';

export interface ModelCapability {
  provider: string;
  id: string;
  api: string;
  inputs: string[];
  efforts: string[];
  tools: boolean | null;
}
export interface ModelTarget { model: string; effort: string }
export interface RoutingPolicy { default?: ModelTarget; fallbacks?: Record<string, ModelTarget[]> }
export interface Requirements { inputs?: string[]; tools?: boolean; allowedModels?: string[] }
export interface AssignmentModel {
  requested: { model: string; effort: string };
  resolved: ModelCapability & { selector: string; effort: string };
  fallbackFrom?: string;
}

export function resolveAssignment(role: Role, catalog: ModelCapability[], policy: RoutingPolicy, requirements: Requirements): AssignmentModel {
  if (!role.enabled) throw new Error('Role is disabled.');
  const selectors = catalog.map(item => `${item.provider}/${item.id}`);
  if (new Set(selectors).size !== selectors.length) throw new Error('Model catalog contains ambiguous duplicate selectors. Refresh the catalog before admission.');
  let model = role.model || policy.default?.model;
  let effort = role.effort === 'default' ? policy.default?.effort : role.effort;
  if (!model || !effort) throw new Error('Configure an explicit default model and effort.');
  let capability = catalog.find(item => `${item.provider}/${item.id}` === model);
  let fallbackFrom: string | undefined;
  if (!capability) {
    const fallback = policy.fallbacks?.[model]?.find(target => catalog.some(item => `${item.provider}/${item.id}` === target.model));
    if (fallback) {
      fallbackFrom = model; model = fallback.model; effort = fallback.effort;
      capability = catalog.find(item => `${item.provider}/${item.id}` === model);
    }
  }
  if (!capability) throw new Error(`Model is unavailable in the admitted catalog: ${model}`);
  if (requirements.allowedModels && !requirements.allowedModels.includes(model)) throw new Error(`Project disallows model ${model}.`);
  if (!(effortLevels as readonly string[]).includes(effort) || !capability.efforts.includes(effort)) throw new Error(`Unsupported effort ${effort} for ${model}.`);
  if (requirements.tools && capability.tools !== true) throw new Error('Required tool capability is unavailable or unverified.');
  if (requirements.inputs?.some(input => !capability.inputs.includes(input))) throw new Error('Required input capability is unavailable.');
  return { requested: { model: role.model, effort: role.effort }, resolved: { ...structuredClone(capability), selector: model, effort }, fallbackFrom };
}

/** Runtime substitution is a new attempt selected only from the failed model's explicit policy. */
export function resolveRuntimeFallback(role: Role, current: AssignmentModel, catalog: ModelCapability[], policy: RoutingPolicy,
  requirements: Requirements, tried: string[]): AssignmentModel | undefined {
  const selectors = catalog.map(item => `${item.provider}/${item.id}`);
  if (new Set(selectors).size !== selectors.length) throw new Error('Model catalog contains ambiguous duplicate selectors. Refresh the catalog before admission.');
  for (const target of policy.fallbacks?.[current.resolved.selector] ?? []) {
    const effort = effortLevels.find(level => level === target.effort);
    if (!effort || tried.includes(target.model) || target.model === current.resolved.selector) continue;
    try {
      const resolved = resolveAssignment({ ...role, model: target.model, effort }, catalog, {}, requirements);
      return { ...resolved, requested: structuredClone(current.requested), fallbackFrom: current.resolved.selector };
    } catch { /* An incompatible alternative grants no authority; try the next explicitly configured target. */ }
  }
  return undefined;
}

const identitySchema = z.object({
  model: z.object({ provider: z.string(), id: z.string(), api: z.string() }),
  thinkingLevel: z.string().nullish(), sessionId: z.string().min(1),
  isStreaming: z.boolean(), autoCompactionEnabled: z.boolean(),
});

export function verifyAssignmentIdentity(plan: AssignmentModel, input: unknown) {
  const state = identitySchema.parse(input);
  const actualEffort = state.thinkingLevel ?? 'off';
  const expected = plan.resolved;
  if (state.model.provider !== expected.provider || state.model.id !== expected.id || state.model.api !== expected.api || actualEffort !== expected.effort) {
    throw new Error('Runner identity or effort mismatch. Assignment cannot execute.');
  }
  if (state.autoCompactionEnabled) throw new Error('Automatic compaction must be disabled for this assignment.');
  if (state.isStreaming) throw new Error('Runner is already executing; admission requires an idle session.');
  return { ...state, thinkingLevel: actualEffort };
}

export interface RpcCommands { command(type: string, args?: Record<string, unknown>): Promise<unknown> }

export async function configureAssignment(rpc: RpcCommands, plan: AssignmentModel) {
  const available = z.object({ models: z.array(z.object({ provider: z.string(), id: z.string(), api: z.string() })) })
    .parse(await rpc.command('get_available_models'));
  const expected = plan.resolved;
  if (!available.models.some(model => model.provider === expected.provider && model.id === expected.id && model.api === expected.api)) {
    throw new Error('Model is unavailable in the current runner catalog. Refresh the assignment.');
  }
  await rpc.command('set_model', { provider: expected.provider, modelId: expected.id });
  const levels = z.object({ levels: z.array(z.string()) }).parse(await rpc.command('get_available_thinking_levels'));
  if (!levels.levels.includes(expected.effort) && !(expected.effort === 'off' && levels.levels.length === 0)) {
    throw new Error('Unsupported effort in the current runner catalog. Assignment cannot execute.');
  }
  await rpc.command('set_thinking_level', { level: expected.effort });
  await rpc.command('set_auto_compaction', { enabled: false });
  await rpc.command('set_auto_retry', { enabled: false });
  return verifyAssignmentIdentity(plan, await rpc.command('get_state'));
}

// Applied only to a private per-attempt configuration. Never merge the operator's
// global or the target repository's model policy into this host configuration.
export function isolatedModelSettings(plan: AssignmentModel) {
  const selector = plan.resolved.selector;
  const chatRoles = ['default', 'smol', 'slow', 'vision', 'plan', 'commit', 'tiny', 'memory', 'task', 'advisor'];
  return {
    modelRoles: Object.fromEntries(chatRoles.map(role => [role, `${selector}:${plan.resolved.effort}`])),
    enabledModels: [selector], enabledProviders: [plan.resolved.provider],
    defaultThinkingLevel: plan.resolved.effort,
    retry: { enabled: false, maxRetries: 0, modelFallback: false,
      fallbackChains: Object.fromEntries([...chatRoles, 'image', 'web', 'speech', 'dictation', 'judge'].map(role => [role, []])) },
    async: { enabled: false }, advisor: { enabled: false }, prewalk: { enabled: false },
    compaction: { enabled: false }, contextPromotion: { enabled: false },
    recap: { enabled: false }, memory: { backend: 'off' }, autolearn: { enabled: false },
    magicKeywords: { enabled: false },
    providers: { anthropic: { serverSideFallback: false } },
  };
}
