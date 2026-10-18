import assert from 'node:assert/strict';
import { test } from 'node:test';
import { sampleTasks } from '../server/seed';

test('SCF-13: Sample tickets have distinct identities and are clearly marked as sample data', () => {
  const tasks = sampleTasks();
  assert.ok(tasks.length > 0);
  assert.equal(new Set(tasks.map(task => task.id)).size, tasks.length);
  for (const task of tasks) {
    assert.equal(task.source, 'sample');
    assert.match(task.key, /^DEMO-/);
    assert.equal(task.sourceId, undefined);
    assert.equal(task.sourceUrl, undefined);
  }
});

test('SCF-14: Editing one sample workspace cannot mutate a newly seeded workspace', () => {
  const tasks = sampleTasks();
  tasks[0].brief = 'Local refinement';
  tasks[0].criteria.push('Local criterion');
  const fresh = sampleTasks();
  assert.equal(fresh[0].brief, '');
  assert.deepEqual(fresh[0].criteria, []);
});
