'use strict';
exports.page = function page(items, cursor, size) {
  if (!Number.isInteger(size) || size < 1 || (cursor !== null && (!Number.isInteger(cursor) || cursor < 0))) throw new TypeError('Invalid pagination');
  const start = cursor ?? 0;
  return { items: items.slice(start, start + size), nextCursor: start + size < items.length ? start + size : null };
};
