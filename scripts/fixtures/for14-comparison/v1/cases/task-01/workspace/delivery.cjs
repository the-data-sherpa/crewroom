'use strict';
exports.deliveryLabel = function deliveryLabel(days) {
  if (!Number.isInteger(days) || days < 0) throw new TypeError('Expected whole nonnegative days');
  return days === 1 ? 'Tomorrow' : `In ${days} days`;
};
