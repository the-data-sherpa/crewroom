'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
const queries = ['', '   ', 'Ada', '  Grace  '];
const results = invoke('empty-state.cjs', 'emptyState', queries.map(query => [query]));
for (let i = 0; i < queries.length; i++) {
  const value = results[i].value;
  assert.equal(typeof value.heading, 'string');
  assert.ok(value.heading.trim().length > 0);
  assert.equal(typeof value.detail, 'string');
  assert.ok(value.detail.trim().length > 0);
  assert.equal(value.canClear, queries[i].trim().length > 0);
  if (queries[i].trim()) assert.ok(value.detail.includes(queries[i].trim()));
}
assert.deepEqual(results[0], results[1], 'Whitespace-only queries use the empty state');
finish('acceptance');
