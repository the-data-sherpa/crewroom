// Test-only extraction of the retained host's pure schema/route/redaction expressions.
// Never import the executable host: it binds a port and starts OMP at module load.
import { readFileSync } from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
import { codingTools } from '../../server/runtime/coding-tools';
import { jiraTools } from '../../server/runtime/jira-tools';
import { publicationTools } from '../../server/runtime/publication-tools';
import { researchTools } from '../../server/runtime/research-tools';
import { briefLimits } from '../../server/runtime/types';
export const source = readFileSync('server/runtime/host.ts', 'utf8');
const start = "await rpc.command('set_host_tools', { tools: ";
const schema = source.slice(source.indexOf(start) + start.length, source.indexOf(' }).catch(async error', source.indexOf(start)));
const schemaFunction = new Function('config', 'codingTools', 'jiraTools', 'publicationTools', 'researchTools', 'briefLimits', `return ${stripTypeScriptTypes(`(${schema})`, { mode: 'strip' })};`);
export const hostTools = (config: any): any[] => schemaFunction(config, codingTools, jiraTools, publicationTools, researchTools, briefLimits);
const routeStart = source.indexOf('const endpoints:');
const routeEnd = source.indexOf('const endpoint =', routeStart);
const routeFunction = new Function('config', stripTypeScriptTypes(source.slice(routeStart, routeEnd), { mode: 'strip' }) + '\nreturn endpoints;');
export const hostRoutes = (config: any): Record<string, string> => routeFunction(config);
const redactStart = source.indexOf('function redact(');
const redactEnd = source.indexOf('\nfunction observe(', redactStart);
const redactFunction = new Function('config', 'input', stripTypeScriptTypes(source.slice(redactStart, redactEnd), { mode: 'strip' }) + '\nreturn redact(input);');
export const hostRedact = (input: unknown, secrets: string[]) => redactFunction({ secrets }, input);

export function catalog() {
  const groups: Record<string, any[]> = {
    common: hostTools({ recipients: [] }),
    team: hostTools({ recipients: [], coordinator: true }).filter(t => !['send_message', 'propose_learning'].includes(t.name)),
    learning: hostTools({ recipients: [], coordinator: true }).filter(t => t.name === 'propose_learning'),
    refinement: hostTools({ recipients: [], refinement: true }).slice(1),
    command: hostTools({ recipients: [], tools: [{ id: 'configured' }] }).slice(1),
  };
  for (const coordinator of [false, true]) {
    groups[`publication-${+coordinator}`] = publicationTools(coordinator);
    for (const flag of [false, true]) {
      groups[`coding-${+coordinator}-${+flag}`] = codingTools(coordinator, flag);
      groups[`research-${+coordinator}-${+flag}`] = researchTools(coordinator, flag);
    }
  }
  for (const reporter of [false, true]) for (const research of [false, true]) for (const fallback of [false, true])
    groups[`jira-${+reporter}-${+research}-${+fallback}`] = jiraTools(reporter, research, fallback);
  const definitions: Record<string, any> = {}, references: Record<string, string[]> = {};
  for (const [group, items] of Object.entries(groups)) references[group] = items.map(tool => {
    const existing = Object.entries(definitions).find(([, value]) => JSON.stringify(value) === JSON.stringify(tool));
    if (existing) return existing[0];
    let key = tool.name;
    for (let i = 2; definitions[key]; i++) key = `${tool.name}-${i}`;
    definitions[key] = tool;
    return key;
  });
  return { definitions, groups: references };
}
