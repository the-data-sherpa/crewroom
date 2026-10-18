import { test } from 'node:test';
import assert from 'node:assert/strict';
import { exportFilters } from './filters.mjs';
test('empty collection exports an empty array',()=>assert.equal(exportFilters([]),'[]'));
test('valid saved filters round trip',()=>{const filters=[{name:'Assigned',query:'assignee = currentUser()'}]; assert.deepEqual(JSON.parse(exportFilters(filters)),filters);});
test('invalid structures and blank names are rejected',()=>{for(const value of [null,{},[null],[{name:' ',query:''}],[{name:'Valid',query:3}]]) assert.throws(()=>exportFilters(value),/name and query/);});
