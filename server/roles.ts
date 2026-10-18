import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { randomUUID } from 'node:crypto';
import { z } from 'zod';
import { effortLevels, type Role } from '../shared/types';

const roleSpecs = [
  ['triage', 'Triage lead', 'Delivery', 'Inspect existing work, define remaining scope, and select the required specialists.'],
  ['implementer', 'Implementer', 'Delivery', 'Make the smallest complete change that meets the accepted brief and project rules.'],
  ['reviewer', 'Independent reviewer', 'Delivery', 'Check the proposed revision against the brief, project rules, and regression risks.'],
  ['product-strategist', 'Product strategist', 'Product', 'Challenge the value, scope, and success criteria of the proposed work.'],
  ['customer-advocate', 'Customer advocate', 'Product', 'Trace customer needs to available evidence and distinguish facts from assumptions.'],
  ['ux-principal', 'UX principal', 'Product', 'Review the user journey, interaction clarity, accessibility, and error recovery.'],
  ['principal-architect', 'Principal architect', 'Engineering', 'Assess architecture, dependencies, compatibility, and change impact.'],
  ['security-engineer', 'Security engineer', 'Engineering', 'Evaluate threats, permissions, sensitive data, and security regressions.'],
  ['sre', 'SRE', 'Engineering', 'Assess deployment, observability, reliability, and recovery behavior.'],
  ['applied-ai-engineer', 'Applied AI engineer', 'Engineering', 'Evaluate model behavior, context, evaluations, costs, and failure modes.'],
  ['test-architect', 'Test architect', 'Quality', 'Define tests that demonstrate acceptance criteria and protect existing behavior.'],
  ['qa-breaker', 'Adversarial QA', 'Quality', 'Exercise edge cases and failure paths; produce reproducible defects.'],
  ['cribl-stream-expert', 'Cribl Stream expert', 'Domain', 'Check Stream behavior and domain assumptions against project evidence.'],
  ['cribl-platform-expert', 'Cribl platform expert', 'Domain', 'Check platform and control-plane behavior against project evidence.'],
  ['cribl-edge-expert', 'Cribl Edge expert', 'Domain', 'Check Edge fleet behavior and deployment coverage against project evidence.'],
  ['premortem-skeptic', 'Premortem skeptic', 'Quality', 'Identify plausible failure scenarios and the evidence needed to rule them out.'],
  ['tech-writer', 'Technical writer', 'Delivery', 'Update affected documentation and explain the resulting behavior clearly.'],
  ['coordinator', 'Coordinator', 'Delivery', 'Interpret the accepted brief, select the specialists needed for the work, reconcile their findings, and handle operator steering.'],
  ['delivery-reporter', 'Delivery reporter', 'Delivery', 'Report workflow progress, blockers, PR links and revision-bound evidence to Jira. Do not rewrite requirements or repository documentation without an explicit assignment.'],
];

export const defaultRoles: Role[] = roleSpecs.map(([id, name, category, purpose]) => ({
  id, name, category, purpose,
  instructions: `${purpose} Read the assigned brief and applicable project instructions. Cite evidence, identify unresolved questions, and leave a concise handoff.`,
  model: '', effort: 'default', enabled: true,
}));

export const roleSchema = z.object({
  id: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/),
  name: z.string().trim().min(1).max(100), category: z.string().trim().min(1).max(60),
  purpose: z.string().trim().min(1).max(1000), instructions: z.string().trim().min(1).max(12000),
  model: z.string().trim().max(200), effort: z.enum(['default', ...effortLevels]), enabled: z.boolean(),
  revision: z.number().int().positive().optional(),
});
const updatedRoleSchema = roleSchema.extend({ revision: z.number().int().positive() });
/** Saved roles also record which learning approval, if any, produced their current revision; operators cannot set it. */
const storedRoleSchema = roleSchema.extend({ learningProposalId: z.uuid().optional() });
const rolesSchema = z.array(storedRoleSchema).min(3).max(100).refine(roles => new Set(roles.map(r => r.id)).size === roles.length, 'Role IDs must be unique.');
export class RoleConflict extends Error {}

export class RoleCatalog {
  constructor(readonly path: string) {}
  read(): Role[] {
    if (!existsSync(this.path)) return structuredClone(defaultRoles);
    const roles = rolesSchema.parse(JSON.parse(readFileSync(this.path, 'utf8')));
    for (const id of ['coordinator', 'delivery-reporter']) {
      if (!roles.some(role => role.id === id)) roles.push(structuredClone(defaultRoles.find(role => role.id === id)!));
    }
    return rolesSchema.parse(roles);
  }
  create(input: unknown): Role[] {
    const role = { ...roleSchema.parse(input), revision: 1 };
    const roles = this.read();
    if (roles.some(item => item.id === role.id)) throw new RoleConflict('Role already exists.');
    roles.push(role); return this.save(rolesSchema.parse(roles));
  }
  /**
   * Every saved change is a new revision; assignments keep the revision they were admitted with. Only an
   * approved learning proposal passes `learningProposalId`, which identifies that proposal's save.
   */
  update(id: string, input: unknown, provenance?: { learningProposalId: string }): Role[] {
    const role = updatedRoleSchema.parse(input);
    if (role.id !== id) throw new RoleConflict('The role ID cannot change.');
    const roles = this.read();
    const index = roles.findIndex(r => r.id === id);
    if (index < 0) throw new RoleConflict('Role not found.');
    const currentRevision = roles[index].revision ?? 1;
    if (role.revision !== currentRevision) throw new RoleConflict(`Role ${id} changed since revision ${role.revision}; current revision is ${currentRevision}. Reload the current definition before saving.`);
    if (['coordinator', 'triage', 'implementer', 'reviewer'].includes(id) && !role.enabled) throw new RoleConflict('The core delivery roles must stay enabled.');
    roles[index] = { ...role, revision: currentRevision + 1, ...(provenance ? { learningProposalId: provenance.learningProposalId } : {}) };
    return this.save(roles);
  }
  private save(roles: Role[]): Role[] {
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    writeFileSync(temporary, JSON.stringify(roles, null, 2) + '\n', { mode: 0o600 });
    renameSync(temporary, this.path);
    return roles;
  }
}
