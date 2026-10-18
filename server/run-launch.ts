import { z } from 'zod';
import { Conflict } from './workbench';
import { briefRequest } from './runtime/brief-request';
import { repositoryRequestSchema } from './repository-request';
import type { Role } from '../shared/types';

/** Admission and queueing parse exactly the same team shape and coordinator/name invariants. */
export const teamRequestSchema = z.object({ team: z.array(z.object({ name: z.string().regex(/^[A-Za-z][A-Za-z0-9_-]{0,60}$/), roleId: z.string(), coordinator: z.boolean(), input: z.string().min(1).max(40000), ...briefRequest }).strict()).min(1).max(4) }).strict();
export function parseTeam(input: unknown) {
  const team = teamRequestSchema.parse(input);
  if (team.team.filter(item => item.coordinator).length !== 1 || new Set(team.team.map(item => item.name)).size !== team.team.length) throw new Conflict('Select one coordinator and distinct assignment names.');
  return team;
}
export const launchSchema = z.object({ team: teamRequestSchema.shape.team, repository: repositoryRequestSchema.default({ mode: 'new' }) }).strict();
export function parseLaunch(input: unknown, roles?: Role[]) {
  const launch = launchSchema.parse(input);
  parseTeam({ team: launch.team });
  if (roles) for (const item of launch.team) if (!roles.some(role => role.id === item.roleId && role.enabled)) throw new Conflict('Selected role is unavailable.');
  return launch;
}
