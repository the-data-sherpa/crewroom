(function (root) {
  function filterPeople(names, query) {
    const needle = query.trim().toLowerCase();
    return names.filter(name => name.toLowerCase().includes(needle));
  }
  if (typeof module !== 'undefined') module.exports = { filterPeople };
  else root.filterPeople = filterPeople;
})(globalThis);
