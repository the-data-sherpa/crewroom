'use strict';
const { assert, invoke, finish } = require('./helper.cjs');
const names = ['Ada Lovelace', 'Grace Hopper', 'Katherine Johnson'];
assert.deepEqual(invoke('people.cjs', 'filterPeople', [[names, '  GRACE '], [names, 'absent'], [names, '']]), [
  { value: ['Grace Hopper'] }, { value: [] }, { value: names },
]);
finish('unit');
