'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
assert.deepEqual(invoke('delivery.cjs', 'deliveryLabel', [[0], [1], [12], [-1], [1.5], ['2']]), [
  { value: 'Today' }, { value: 'Tomorrow' }, { value: 'In 12 days' },
  { error: 'TypeError' }, { error: 'TypeError' }, { error: 'TypeError' },
]);
finish('acceptance');
