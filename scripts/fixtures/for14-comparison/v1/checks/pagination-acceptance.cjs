'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
assert.deepEqual(invoke('pagination.cjs', 'page', [[['a', 'b'], null, 1], [['a', 'b'], 0, 1], [[], 0, 2], [['a'], 4, 1], [['a'], -1, 1], [['a'], 0, 0], [['a'], 0.5, 1], [['a'], 0, 1.5]]), [
  { value: { items: ['a'], nextCursor: 1 } }, { value: { items: ['a'], nextCursor: 1 } },
  { value: { items: [], nextCursor: null } }, { value: { items: [], nextCursor: null } },
  { error: 'TypeError' }, { error: 'TypeError' }, { error: 'TypeError' }, { error: 'TypeError' },
]);
finish('acceptance');
