import { Conflict } from '../workbench';
export { briefRequest } from './brief-request';

/**
 * An assignment's structured brief, snapshotted at admission. Criterion text is copied from the
 * run's accepted requirements so a later refinement cannot rewrite what the assignment was given.
 */
export interface AssignmentBrief {
  criteria: { index: number; criterion: string; obligation: string; checkIds: string[] }[];
  /** Names of other assignments in the same run whose results this assignment depends on. */
  dependsOn: string[];
}

export type BriefRequest = { name: string; criteria: { index: number; obligation: string; checkIds: string[] }[]; dependsOn: string[] };

/**
 * Validates requested briefs for a set of new assignments against the accepted criteria, configured
 * checks and the team (existing active assignments plus the new ones), then snapshots them.
 */
export function snapshotBriefs(requests: BriefRequest[], context: { criteria: string[]; checkIds: string[]; existing: { name: string; brief?: AssignmentBrief }[] }) {
  const names = new Set([...context.existing.map(item => item.name), ...requests.map(item => item.name)]);
  const briefs = new Map<string, AssignmentBrief>();
  for (const request of requests) {
    if (new Set(request.criteria.map(item => item.index)).size !== request.criteria.length) throw new Conflict(`${request.name}: each accepted criterion may appear once in a brief.`);
    const criteria = request.criteria.map(item => {
      const criterion = context.criteria[item.index];
      if (criterion === undefined) throw new Conflict(`${request.name}: criterion ${item.index} is not an accepted criterion.`);
      const unknown = item.checkIds.find(id => !context.checkIds.includes(id));
      if (unknown) throw new Conflict(`${request.name}: check ${unknown} is not configured for this repository.`);
      return { index: item.index, criterion, obligation: item.obligation, checkIds: [...new Set(item.checkIds)] };
    });
    const dependsOn = [...new Set(request.dependsOn)];
    if (dependsOn.includes(request.name)) throw new Conflict(`${request.name}: an assignment cannot depend on itself.`);
    const missing = dependsOn.find(name => !names.has(name));
    if (missing) throw new Conflict(`${request.name}: dependency ${missing} is not an assignment in this team.`);
    briefs.set(request.name, { criteria, dependsOn });
  }
  // Dependencies across the whole team must stay acyclic, including those of existing assignments.
  const edges = new Map<string, string[]>([...context.existing.map(item => [item.name, item.brief?.dependsOn ?? []] as const), ...[...briefs].map(([name, brief]) => [name, brief.dependsOn] as const)]);
  const state = new Map<string, 'visiting' | 'done'>();
  const visit = (name: string): void => {
    if (state.get(name) === 'done') return;
    if (state.get(name) === 'visiting') throw new Conflict(`Assignment dependencies form a cycle through ${name}.`);
    state.set(name, 'visiting');
    for (const next of edges.get(name) ?? []) visit(next);
    state.set(name, 'done');
  };
  for (const name of edges.keys()) visit(name);
  return briefs;
}

/**
 * The brief text given to an agent: its own obligations and every active teammate's, so a reviewer sees what was
 * owed. A retired assignment that an active one depends on stays in the context, marked retired, with its brief.
 */
export function presentBriefs(own: { name: string; brief?: AssignmentBrief }, assignments: { name: string; role: { name: string }; brief?: AssignmentBrief; retiredAt?: string }[]) {
  const active = assignments.filter(item => !item.retiredAt);
  const referenced = new Set(active.flatMap(item => item.brief?.dependsOn ?? []));
  const team = assignments.filter(item => !item.retiredAt || referenced.has(item.name));
  const describe = (brief?: AssignmentBrief) => brief ? { criteria: brief.criteria, dependsOn: brief.dependsOn } : 'No structured brief (admitted before structured briefs).';
  return `Structured brief for ${own.name}: ${JSON.stringify(describe(own.brief))}\nTeam briefs (criterion index is zero-based; reviewers assess coverage against these obligations): ${JSON.stringify(team.map(item => ({ name: item.name, role: item.role.name, ...(item.retiredAt ? { retired: true } : {}), brief: describe(item.brief) })))}`;
}
