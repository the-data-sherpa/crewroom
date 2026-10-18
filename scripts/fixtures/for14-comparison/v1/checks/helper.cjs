'use strict';
// Trusted definitions import only Node builtins and this immutable helper.
// Workspace modules execute in a separate child, so they cannot replace the
// parent assertions or turn process.exit(0) into a successful check.
const { execFileSync } = require('node:child_process');
const { lstatSync } = require('node:fs');
const { resolve } = require('node:path');
const assert = require('node:assert/strict');

function invoke(file, name, calls) {
  const path = resolve(process.cwd(), file);
  assert.equal(lstatSync(path).isFile(), true, 'Source must be a regular file');
  const script = `
    const fs = require('node:fs');
    const calls = JSON.parse(fs.readFileSync(0, 'utf8'));
    const fn = require(process.argv[1])[process.argv[2]];
    const results = calls.map(args => {
      try { return { value: fn(...args) }; }
      catch (error) { return { error: error.name }; }
    });
    process.stdout.write(JSON.stringify(results));
  `;
  const output = execFileSync(process.execPath, ['-e', script, path, name], {
    cwd: process.cwd(), input: JSON.stringify(calls), encoding: 'utf8',
    timeout: 5000, maxBuffer: 1024 * 1024,
    env: { PATH: process.env.PATH, LANG: 'C.UTF-8' },
  });
  const values = JSON.parse(output);
  assert.equal(values.length, calls.length, 'Every invocation must return a result');
  return values;
}
function finish(checkId) {
  console.log(JSON.stringify({ version: 1, checkId, surface: 'logic', observations: { executed: true } }));
}
module.exports = { assert, invoke, finish };
