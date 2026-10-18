'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
assert.deepEqual(invoke('delivery.cjs', 'deliveryLabel', [[1], [3]]), [{ value: 'Tomorrow' }, { value: 'In 3 days' }]);
finish('unit');
