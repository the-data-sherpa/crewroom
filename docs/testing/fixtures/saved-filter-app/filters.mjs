export function exportFilters(filters) {
  if (!Array.isArray(filters) || filters.some(filter => !filter || typeof filter.name !== 'string' || !filter.name.trim() || typeof filter.query !== 'string')) throw new Error('Enter filters with a name and query.');
  return JSON.stringify(filters, null, 2);
}
