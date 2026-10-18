'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
const [{ value }] = invoke('empty-state.cjs', 'emptyState', [['']]);
assert.equal(typeof value.heading, 'string');
assert.ok(value.heading.trim().length > 0);
assert.equal(typeof value.detail, 'string');
assert.ok(value.detail.trim().length > 0);
assert.equal(value.canClear, false);
finish('unit');
