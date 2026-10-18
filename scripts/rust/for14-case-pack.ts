/** Immutable inputs for the bounded FOR-14 trial, not an execution authority.
 * The Rust daemon consumes the generated JSON directly. Private oracle and
 * counterbalance data must never be copied into a worker workspace. */
import { createHash } from 'node:crypto';
import { lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

export const fixtureRoot = fileURLToPath(new URL('../fixtures/for14-comparison/v1/', import.meta.url));
export const digest = (bytes: string | Uint8Array) => createHash('sha256').update(bytes).digest('hex');
export function canonical(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  return '{' + Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
    .map(([key, item]) => JSON.stringify(key) + ':' + canonical(item)).join(',') + '}';
}
export const hashValue = (value: unknown) => digest(canonical(value));
export type FixtureFile = { path: string; content: string; sha256: string };
export type RequiredCheck = { id: string; script: string; surface: 'logic' | 'browser'; definitionHash: string; dependencies: string[] };
export type ComparisonCase = {
  id: string; version: 1; prompt: string; promptHash: string; projectGuidance: string; files: FixtureFile[];
  sourceHash: string; requiredChecks: RequiredCheck[]; priorFiles?: FixtureFile[];
  priorSourceHash?: string; priorCheckId?: string;
};
export type ComparisonCasePack = {
  id: 'crewroom-instruction-comparison'; version: 1; cases: ComparisonCase[];
  checks: FixtureFile[]; privateOracleHash: string; packHash: string;
};

export function sourceManifestHash(files: Pick<FixtureFile, 'path' | 'sha256'>[]): string {
  return hashValue([...files].sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0)
    .map(({ path, sha256 }) => ({ path, sha256 })));
}
function filesIn(directory: string, prefix = ''): FixtureFile[] {
  return readdirSync(directory).sort().flatMap(name => {
    const path = join(directory, name), subpath = prefix ? prefix + '/' + name : name;
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) throw new Error('Fixture inputs must not contain symbolic links');
    if (stat.isDirectory()) return filesIn(path, subpath);
    if (!stat.isFile()) throw new Error('Fixture inputs must be regular files');
    const content = readFileSync(path, 'utf8');
    return [{ path: subpath, content, sha256: digest(content) }];
  });
}
export function buildCasePack(root = fixtureRoot): ComparisonCasePack {
  const checks = filesIn(join(root, 'checks'));
  const cases = readdirSync(join(root, 'cases')).sort().map(id => {
    const directory = join(root, 'cases', id);
    const definition = JSON.parse(readFileSync(join(directory, 'case.json'), 'utf8')) as {
      id: string; version: 1; checks: Pick<RequiredCheck, 'id' | 'script' | 'surface'>[]; priorCheckId?: string;
    };
    if (definition.id !== id || definition.version !== 1) throw new Error('Case identity mismatch');
    const files = filesIn(join(directory, 'workspace'));
    const prompt = readFileSync(join(directory, 'task.txt'), 'utf8');
    const requiredChecks = definition.checks.map(check => {
      const dependencies = check.surface === 'browser' ? [check.script] : [check.script, 'helper.cjs'];
      const sources = dependencies.map(path => {
        const source = checks.find(item => item.path === path);
        if (!source) throw new Error('Missing check dependency: ' + path);
        return { path: source.path, sha256: source.sha256 };
      });
      return { ...check, dependencies, definitionHash: hashValue(sources) };
    });
    const projectGuidance = files.find(file => file.path === 'AGENTS.md')?.content;
    if (!projectGuidance) throw new Error('Every case must have discoverable project instructions');
    const result: ComparisonCase = { id, version: 1, prompt, promptHash: digest(prompt), projectGuidance, files,
      sourceHash: sourceManifestHash(files), requiredChecks };
    if (definition.priorCheckId) {
      const priorDirectory = join(directory, 'prior');
      const overrides = readdirSync(directory).includes('prior') ? filesIn(priorDirectory) : [];
      result.priorFiles = files.map(file => overrides.find(prior => prior.path === file.path) ?? file);
      result.priorSourceHash = sourceManifestHash(result.priorFiles);
      result.priorCheckId = definition.priorCheckId;
    }
    return result;
  });
  const content = { id: 'crewroom-instruction-comparison' as const, version: 1 as const, cases, checks,
    privateOracleHash: digest(readFileSync(join(root, 'private', 'oracles.json'))) };
  return { ...content, packHash: hashValue(content) };
}
export function readCasePack(root = fixtureRoot): ComparisonCasePack {
  const saved = JSON.parse(readFileSync(join(root, 'case-pack.json'), 'utf8')) as ComparisonCasePack;
  const actual = buildCasePack(root);
  if (canonical(saved) !== canonical(actual)) throw new Error('Case pack is stale or its source/check/oracle bytes changed; freeze a new version');
  return saved;
}
/** Only writes the explicit public case file list into an empty disposable root. */
export function materializeCase(item: ComparisonCase, destination: string, prior = false): void {
  mkdirSync(destination, { recursive: true, mode: 0o700 });
  const root = realpathSync(destination);
  if (readdirSync(root).length) throw new Error('Case destination must be empty');
  for (const file of prior ? item.priorFiles ?? item.files : item.files) {
    const path = resolve(root, file.path), inside = relative(root, path);
    if (!inside || inside === '..' || inside.startsWith('..' + sep) || file.path.includes('\\') || file.path.startsWith('/') || inside !== file.path.split('/').join(sep)) throw new Error('Unsafe fixture path');
    if (digest(file.content) !== file.sha256) throw new Error('Fixture content hash mismatch');
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    writeFileSync(path, file.content, { flag: 'wx', mode: 0o600 });
  }
}
export type Variant = 'current' | 'proposed';
export function counterbalancedOrder(cases: Pick<ComparisonCase, 'id'>[]): { caseId: string; repetition: number; variant: Variant }[] {
  if (cases.length !== 4 || new Set(cases.map(item => item.id)).size !== 4) throw new Error('The fixed design requires four distinct cases');
  return [1, 2].flatMap(repetition => cases.flatMap((item, index) => {
    const order: Variant[] = (index + repetition) % 2 === 1 ? ['current', 'proposed'] : ['proposed', 'current'];
    return order.map(variant => ({ caseId: item.id, repetition, variant }));
  }));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] !== '--freeze') throw new Error('Usage: tsx scripts/rust/for14-case-pack.ts --freeze');
  writeFileSync(join(fixtureRoot, 'case-pack.json'), JSON.stringify(buildCasePack(), null, 2) + '\n');
}
