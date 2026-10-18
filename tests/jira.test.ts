import assert from 'node:assert/strict';
import { test } from 'node:test';
import { descriptionText, jiraImportSchema } from '../server/jira';

function issue(id = '10001') {
  return { id, key: 'SIM-1', fields: { summary: '  Fix a fixture  ', status: { name: 'Open' } } };
}

test('SCF-01: Jira text extraction preserves paragraph and hard-break boundaries', () => {
  assert.equal(descriptionText({ type: 'doc', content: [
    { type: 'heading', content: [{ type: 'text', text: 'Outcome' }] },
    { type: 'paragraph', content: [
      { type: 'text', text: 'First' }, { type: 'hardBreak' }, { type: 'text', text: 'Second' },
    ] },
  ] }), 'Outcome\nFirst\nSecond\n');
});

test('SCF-02: Empty and malformed Jira descriptions cannot crash extraction', () => {
  for (const value of [null, undefined, 42, [], { text: 12 }, { content: 'invalid' }]) {
    assert.equal(descriptionText(value), '');
  }
  assert.equal(descriptionText('plain description'), 'plain description');
});

test('SCF-03: Deeply nested descriptions terminate at the depth limit', () => {
  let value: unknown = { type: 'text', text: 'unreachable' };
  for (let depth = 0; depth < 40; depth++) value = { type: 'doc', content: [value] };
  assert.equal(descriptionText(value), '');
});

test('SCF-04: Jira imports normalize text and accept omitted optional fields', () => {
  const result = jiraImportSchema.parse({ siteUrl: 'https://jira.example.invalid/', issues: [issue()] });
  assert.equal(result.issues[0].fields.summary, 'Fix a fixture');
  assert.equal(result.issues[0].fields.priority, undefined);
});

test('SCF-05: Jira source URLs reject credentials, non-HTTPS, paths, queries and fragments', () => {
  for (const siteUrl of [
    'http://jira.example.invalid', 'https://user:secret@jira.example.invalid',
    'https://jira.example.invalid/rest', 'https://jira.example.invalid/?token=synthetic',
    'https://jira.example.invalid/#section', 'file:///tmp/fixture',
  ]) {
    assert.equal(jiraImportSchema.safeParse({ siteUrl, issues: [issue()] }).success, false, siteUrl);
  }
});

test('SCF-06: Duplicate stable issue IDs and empty imports are rejected', () => {
  for (const issues of [[], [issue(), { ...issue(), key: 'SIM-2' }]]) {
    assert.equal(jiraImportSchema.safeParse({ siteUrl: 'https://jira.example.invalid', issues }).success, false);
  }
});

test('SCF-07: Malformed issue identity, empty summary and oversized batches are rejected', () => {
  const siteUrl = 'https://jira.example.invalid';
  for (const entry of [{ ...issue(), key: 'no-number' }, { ...issue(), id: '' },
    { ...issue(), fields: { ...issue().fields, summary: ' ' } }]) {
    assert.equal(jiraImportSchema.safeParse({ siteUrl, issues: [entry] }).success, false);
  }
  assert.equal(jiraImportSchema.safeParse({ siteUrl, issues: Array.from({ length: 1001 }, (_, i) => issue(String(i))) }).success, false);
});
