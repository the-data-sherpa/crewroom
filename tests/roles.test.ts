import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { defaultRoles, RoleCatalog, roleSchema } from '../server/roles';
import { startServer } from '../server/app';

function fixture(t: TestContext) {
  const dir = mkdtempSync(join(tmpdir(), 'crewroom-roles-test-'));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const path = join(dir, 'roles.json');
  return { dir, path, catalog: new RoleCatalog(path) };
}

test('SCF-08: Reading absent user configuration returns independent defaults without writing', t => {
  const { catalog, path } = fixture(t);
  const first = catalog.read();
  first[0].instructions = 'changed only in this read';
  assert.notEqual(catalog.read()[0].instructions, first[0].instructions);
  assert.notEqual(defaultRoles[0].instructions, first[0].instructions);
  assert.equal(existsSync(path), false);
});

test('SCF-09: Role edits persist only in the caller-selected file and preserve unrelated roles', t => {
  const { catalog, path, dir } = fixture(t);
  const original = catalog.read();
  const changed = { ...original[1], model: 'fixture-provider/fixture-model', instructions: 'Use fixture instructions.' };
  catalog.update(changed.id, { ...changed, revision: changed.revision ?? 1 });
  const reopened = new RoleCatalog(path).read();
  assert.deepEqual(reopened[1], { ...changed, revision: 2 }, 'A saved edit is the next role revision.');
  assert.deepEqual(reopened.filter(r => r.id !== changed.id), original.filter(r => r.id !== changed.id));
  assert.deepEqual(readdirSync(dir), ['roles.json']);
  assert.equal(statSync(path).mode & 0o777, 0o600);
});

test('SCF-10: Changing a role ID or updating an absent role leaves persisted data intact', t => {
  const { catalog, path } = fixture(t);
  catalog.update(defaultRoles[1].id, { ...defaultRoles[1], revision: 1 });
  const before = readFileSync(path, 'utf8');
  assert.throws(() => catalog.update(defaultRoles[1].id, { ...defaultRoles[1], revision: 2, id: 'different' }), /ID cannot change/);
  assert.throws(() => catalog.update('absent', { ...defaultRoles[1], revision: 1, id: 'absent' }), /not found/);
  assert.equal(readFileSync(path, 'utf8'), before);
});

test('SCF-11: Corrupt or duplicate role configuration fails without resetting the file', t => {
  const { catalog, path } = fixture(t);
  for (const content of ['{bad-json', JSON.stringify([...defaultRoles, defaultRoles[0]])]) {
    writeFileSync(path, content);
    assert.throws(() => catalog.read());
    assert.equal(readFileSync(path, 'utf8'), content);
  }
});

test('SCF-12: Role validation trims text and rejects invalid identifiers, effort and empty instructions', () => {
  assert.equal(roleSchema.parse({ ...defaultRoles[0], name: '  Fixture lead  ' }).name, 'Fixture lead');
  for (const invalid of [
    { ...defaultRoles[0], id: '../escape' }, { ...defaultRoles[0], effort: 'invented' },
    { ...defaultRoles[0], instructions: ' ' }, { ...defaultRoles[0], name: '' },
  ]) assert.equal(roleSchema.safeParse(invalid).success, false);
});

test('RTE-04: role configuration can select explicit OMP effort tiers without an automatic effort adviser', t => {
  const { catalog } = fixture(t);
  for (const effort of ['off', 'minimal', 'xhigh', 'max']) {
    catalog.update(defaultRoles[1].id, { ...defaultRoles[1], revision: catalog.read()[1].revision ?? 1, effort });
    assert.equal(catalog.read()[1].effort, effort);
  }
  assert.equal(roleSchema.safeParse({ ...defaultRoles[1], effort: 'auto' }).success, false);
});

test('ROL-01/07: user roles can be created and delivery reporting remains distinct from repository documentation', () => {
  const root = mkdtempSync(join(tmpdir(), 'crewroom-role-create-'));
  try {
    const catalog = new RoleCatalog(join(root, 'roles.json'));
    const initial = catalog.read();
    assert.ok(initial.some(role => role.id === 'coordinator'));
    assert.ok(initial.some(role => role.id === 'delivery-reporter'));
    assert.notEqual(initial.find(role => role.id === 'delivery-reporter')!.purpose, initial.find(role => role.id === 'tech-writer')!.purpose);
    const custom = { ...initial[0], id: 'performance-reviewer', name: 'Performance reviewer', purpose: 'Inspect performance regressions', model: 'fixture/model', effort: 'medium' };
    catalog.create(custom);
    assert.equal(new RoleCatalog(catalog.path).read().find(role => role.id === custom.id)?.model, 'fixture/model');
    assert.throws(() => catalog.create(custom), /already exists/i);
    catalog.update(custom.id, { ...custom, revision: 1, instructions: 'Measure before recommending a change.' });
    assert.equal(catalog.read().find(role => role.id === custom.id)?.instructions, 'Measure before recommending a change.');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('ROL-01: an existing scaffold catalog gains the new delivery roles without replacing user settings', t => {
  const { catalog, path } = fixture(t);
  const original = defaultRoles.slice(0, 17).map(role => ({ ...role, model: 'fixture/saved' }));
  const content = JSON.stringify(original);
  writeFileSync(path, content);
  const upgraded = catalog.read();
  assert.deepEqual(upgraded.slice(0, 17), original);
  assert.ok(upgraded.some(role => role.id === 'coordinator'));
  assert.ok(upgraded.some(role => role.id === 'delivery-reporter'));
  assert.equal(readFileSync(path, 'utf8'), content, 'Reading configuration must not rewrite it.');
});

test('ROL-02: two stale editors cannot overwrite a saved revision through HTTP', async t => {
  const { catalog, path, dir } = fixture(t);
  const server = await startServer({ port: 0, dataDir: join(dir, 'data'), serveUi: false, rolePath: path });
  t.after(server.close);
  const original = catalog.read().find(role => role.id === 'tech-writer')!;
  const post = (role: typeof original) => fetch(`${server.origin}/api/roles/${role.id}`, {
    method: 'POST', headers: { Origin: server.origin, 'Content-Type': 'application/json' }, body: JSON.stringify(role),
  });
  const first = await post({ ...original, revision: 1, model: 'fixture/first', effort: 'high', instructions: 'First operator edit.' });
  assert.equal(first.status, 200);
  const saved = readFileSync(path, 'utf8');
  const second = await post({ ...original, revision: 1, model: 'fixture/second', instructions: 'Second operator edit.' });
  assert.equal(second.status, 409);
  assert.match((await second.json()).error, /changed since revision 1; current revision is 2.*Reload/);
  assert.equal(readFileSync(path, 'utf8'), saved);
  assert.equal(catalog.read().find(role => role.id === original.id)?.instructions, 'First operator edit.');
  assert.equal(catalog.read().find(role => role.id === original.id)?.model, 'fixture/first');
});
