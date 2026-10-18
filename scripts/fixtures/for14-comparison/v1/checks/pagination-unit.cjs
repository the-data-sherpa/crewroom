'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
assert.deepEqual(invoke('pagination.cjs', 'page', [[['a', 'b', 'c'], 0, 2], [['a', 'b', 'c'], 2, 2]]), [
  { value: { items: ['a', 'b'], nextCursor: 2 } }, { value: { items: ['c'], nextCursor: null } },
]);
finish('unit');
