import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { buildCasePack, canonical, counterbalancedOrder, digest, fixtureRoot, materializeCase, readCasePack, sourceManifestHash, type ComparisonCase } from './for14-case-pack';

function temporary(t: TestContext) {
  const path = mkdtempSync(join(tmpdir(), 'crewroom-for14-case-'));
  t.after(() => rmSync(path, { recursive: true, force: true }));
  return path;
}
function check(item: ComparisonCase, id: string, workspace: string) {
  const definition = item.requiredChecks.find(check => check.id === id)!;
  return spawnSync(process.execPath, [join(fixtureRoot, 'checks', definition.script)], {
    cwd: workspace, encoding: 'utf8', timeout: 10000,
  });
}

test('frozen four-case pack exactly binds ordinary prompts, source, check dependencies and private oracle', () => {
  const pack = readCasePack();
  assert.deepEqual(pack, buildCasePack());
  assert.equal(pack.cases.length, 4);
  for (const item of pack.cases) {
    assert.equal(item.sourceHash, sourceManifestHash(item.files));
    assert.equal(item.promptHash, digest(item.prompt));
    assert.equal(item.projectGuidance, item.files.find(file => file.path === 'AGENTS.md')!.content);
    assert.equal(item.requiredChecks.length, 2);
    assert.ok(item.requiredChecks.every(check => check.definitionHash.length === 64));
    assert.ok(!/baseline|candidate|scor(?:e|ing)|STEP\s*\d|variant|oracle/.test(item.prompt));
    assert.ok(!/baseline|candidate|scor(?:e|ing)|STEP\s*\d|variant|oracle/.test(item.projectGuidance));
  }
});

test('materialization exposes only explicit public workspace files and refuses alias or traversal paths', t => {
  const root = temporary(t), pack = readCasePack();
  for (const item of pack.cases) {
    const workspace = join(root, item.id);
    materializeCase(item, workspace);
    assert.deepEqual(readdirSync(workspace).sort(), item.files.map(file => file.path).sort());
    for (const file of item.files) assert.equal(digest(readFileSync(join(workspace, file.path))), file.sha256);
    assert.throws(() => materializeCase(item, workspace), /empty/);
  }
  const item = structuredClone(pack.cases[0]);
  item.files[0].path = '../outside';
  assert.throws(() => materializeCase(item, join(root, 'unsafe')), /Unsafe/);
  const corrupt = structuredClone(pack.cases[0]);
  corrupt.files[0].content += 'unbound';
  assert.throws(() => materializeCase(corrupt, join(root, 'corrupt')), /hash mismatch/);
});

test('changing ordinary files or trusted transitive checks invalidates the frozen pack', t => {
  const root = temporary(t), copy = join(root, 'pack');
  cpSync(fixtureRoot, copy, { recursive: true });
  assert.deepEqual(readCasePack(copy), readCasePack());
  writeFileSync(join(copy, 'checks', 'helper.cjs'), 'process.exit(0);');
  assert.throws(() => readCasePack(copy), /stale|changed/);
  rmSync(join(copy, 'checks', 'helper.cjs'));
  symlinkSync(join(fixtureRoot, 'checks', 'helper.cjs'), join(copy, 'checks', 'helper.cjs'));
  assert.throws(() => readCasePack(copy), /symbolic/);
});

test('ordinary helper checks reject broken source and accept a real corrected implementation', t => {
  const root = temporary(t), item = readCasePack().cases[0];
  materializeCase(item, root);
  assert.equal(check(item, 'unit', root).status, 0);
  assert.equal(check(item, 'acceptance', root).status, 1);
  const source = join(root, 'delivery.cjs');
  writeFileSync(source, readFileSync(source, 'utf8').replace("return days === 1", "if (days === 0) return 'Today';\n  return days === 1"));
  for (const id of ['unit', 'acceptance']) {
    const result = check(item, id, root);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), { version: 1, checkId: id, surface: 'logic', observations: { executed: true } });
  }
});

test('an actual source-A pass cannot cover changed source-B or later edits', t => {
  const root = temporary(t), item = readCasePack().cases[1];
  const prior = join(root, 'prior'), current = join(root, 'current');
  materializeCase(item, prior, true);
  const result = check(item, item.priorCheckId!, prior);
  assert.equal(result.status, 0, result.stderr);
  materializeCase(item, current);
  assert.notEqual(item.priorSourceHash, item.sourceHash);
  assert.equal(check(item, 'unit', current).status, 1);
  assert.equal(check(item, 'acceptance', current).status, 1);
  const source = join(current, 'pagination.cjs');
  writeFileSync(source, readFileSync(source, 'utf8').replace('cursor || 1', 'cursor ?? 0'));
  assert.equal(check(item, 'unit', current).status, 0);
  assert.equal(check(item, 'acceptance', current).status, 0);
  writeFileSync(source, readFileSync(source, 'utf8').replace('cursor ?? 0', 'cursor || 1'));
  assert.equal(check(item, 'acceptance', current).status, 1);
});

test('routine case permits independently chosen wording without a secret copy requirement', t => {
  const root = temporary(t), item = readCasePack().cases[2];
  materializeCase(item, root);
  assert.equal(check(item, 'acceptance', root).status, 1);
  for (const heading of ['Nothing here yet', 'Start your contacts list']) {
    writeFileSync(join(root, 'empty-state.cjs'), `exports.emptyState = query => { const text = query.trim(); return { heading: ${JSON.stringify(heading)}, detail: text ? 'No results for ' + text : 'Add your first contact.', canClear: !!text }; };\n`);
    assert.equal(check(item, 'unit', root).status, 0);
    assert.equal(check(item, 'acceptance', root).status, 0);
  }
});

test('workspace code cannot pass by exiting early or replacing parent assertion functions', t => {
  const root = temporary(t), item = readCasePack().cases[0];
  materializeCase(item, root);
  for (const body of [
    'process.exit(0);',
    "require('node:assert/strict').deepEqual = () => {}; exports.deliveryLabel = () => 'wrong';",
    "require('node:assert/strict').ok = () => {}; exports.deliveryLabel = () => 'wrong';",
  ]) {
    writeFileSync(join(root, 'delivery.cjs'), body);
    assert.equal(check(item, 'acceptance', root).status, 1);
  }
});

test('people logic passes while the initial actual page still needs wiring and layout work', t => {
  const root = temporary(t), item = readCasePack().cases[3];
  materializeCase(item, root);
  assert.equal(check(item, 'unit', root).status, 0);
  assert.match(readFileSync(join(root, 'index.html'), 'utf8'), /addEventListener\('change'/);
  assert.equal(item.requiredChecks[1].surface, 'browser');
});

test('fixed counterbalance contains sixteen unique cells and reverses order by case and repetition', () => {
  const cases = readCasePack().cases, order = counterbalancedOrder(cases);
  assert.equal(order.length, 16);
  assert.equal(new Set(order.map(canonical)).size, 16);
  assert.deepEqual(order.slice(0, 4).map(cell => cell.variant), ['current', 'proposed', 'proposed', 'current']);
  assert.deepEqual(order.slice(8, 12).map(cell => cell.variant), ['proposed', 'current', 'current', 'proposed']);
  for (const item of cases) for (const variant of ['current', 'proposed']) assert.equal(order.filter(cell => cell.caseId === item.id && cell.variant === variant).length, 2);
  assert.throws(() => counterbalancedOrder(cases.slice(0, 3)), /four/);
});
