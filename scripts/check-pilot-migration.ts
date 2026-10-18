/**
 * Store migration check: migrate a COPY of the retained pilot database and confirm nothing the
 * workflow relies on changed. The pilot database itself is only read, never opened by Workbench.
 *
 *   npx tsx scripts/check-pilot-migration.ts [.crewroom/pilot/data/workflow.sqlite]
 */
import assert from 'node:assert/strict';
import { copyFileSync, existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { Workbench } from '../server/workbench';

const source = resolve(process.argv[2] ?? '.crewroom/pilot/data/workflow.sqlite');
if (!existsSync(source)) throw new Error(`Pilot database not found at ${source}. Pass its path as the first argument.`);
const root = mkdtempSync(join(tmpdir(), 'crewroom-pilot-migration-'));
const copy = join(root, 'workflow.sqlite');
try {
  // Read the original through SQLite's backup-safe path (WAL included), then work only on the copy.
  const original = new DatabaseSync(source, { readOnly: true });
  original.exec(`VACUUM INTO '${copy.replaceAll("'", "''")}'`);
  const count = (db: DatabaseSync, sql: string) => Number(Object.values(db.prepare(sql).get()!)[0]);
  const before = {
    version: count(original, 'PRAGMA user_version'),
    runs: original.prepare('SELECT id, data FROM runs ORDER BY id').all().map(row => [row.id, row.data]),
    queuedIds: original.prepare("SELECT r.id FROM runs r WHERE json_extract(r.data,'$.status')='awaiting_runner' AND COALESCE(json_extract(r.data,'$.purpose'),'coding')='coding' AND NOT EXISTS (SELECT 1 FROM assignments a WHERE a.run_id=r.id) ORDER BY json_extract(r.data,'$.createdAt'), r.rowid").all().map(row => String(row.id)),
    tickets: original.prepare('SELECT id, imported, refinement FROM tickets ORDER BY id').all(),
    attempts: original.prepare('SELECT id, data FROM attempts ORDER BY id').all().map(row => [row.id, row.data]),
    assignments: original.prepare('SELECT id, data FROM assignments ORDER BY id').all().map(row => [row.id, row.data]),
    receipts: count(original, "SELECT count(*) FROM agent_events WHERE json_extract(data,'$.type') <> 'message_update'"),
    progress: count(original, "SELECT count(*) FROM agent_events WHERE json_extract(data,'$.type') = 'message_update'"),
    coding: original.prepare('SELECT run_id, data FROM coding ORDER BY run_id').all().map(row => [row.run_id, row.data]),
    repositories: original.prepare('SELECT id, data FROM repositories ORDER BY id').all().map(row => [row.id, row.data]),
  };
  original.close();

  const store = new Workbench(copy); store.close();
  const migrated = new DatabaseSync(copy, { readOnly: true });
  assert.equal(count(migrated, 'PRAGMA user_version'), 21);
  assert.deepEqual(migrated.prepare('SELECT id, data FROM repositories ORDER BY id').all().map(row => [row.id, row.data]), before.repositories, 'Repository configuration is unchanged.');
  assert.equal(count(migrated, 'SELECT count(*) FROM learning_proposals'), 0, 'Learning proposals start empty.');
  assert.equal(count(migrated, 'SELECT count(*) FROM refinement_proposals'), 0, 'Refinement proposals start empty.');
  assert.equal(count(migrated, 'SELECT count(*) FROM research_artifacts'), 0, 'Existing runs do not acquire fabricated research artifacts.');
  assert.deepEqual(migrated.prepare('SELECT id, imported, refinement FROM tickets ORDER BY id').all(), before.tickets, 'Jira source and operator refinements remain separate and unchanged.');
  for (const row of before.tickets) {
    const refinement = JSON.parse(String(row.refinement));
    const entries = migrated.prepare('SELECT revision, data FROM requirement_revisions WHERE task_id=?').all(row.id);
    assert.equal(entries.length, 1, 'Each retained ticket gets one migrated history entry.');
    const entry = JSON.parse(String(entries[0].data));
    assert.equal(entry.revision, refinement.revision);
    assert.equal(entry.brief, refinement.brief);
    assert.deepEqual(entry.criteria, refinement.criteria);
  }
  const expectedRuns = before.runs.map(([id, data]) => {
    const run = JSON.parse(String(data));
    const position = before.queuedIds.indexOf(String(id));
    if (position < 0) delete run.position; else run.position = position;
    return [id, JSON.stringify(run)];
  });
  assert.deepEqual(migrated.prepare('SELECT id, data FROM runs ORDER BY id').all().map(row => [row.id, row.data]), expectedRuns, 'Only the obsolete position is replaced with the local queue order; run history and specs remain unchanged.');
  assert.equal(count(migrated, 'SELECT revision FROM queue_state WHERE id=1'), 0, 'Migrated queue begins at revision zero.');
  assert.equal(migrated.prepare('SELECT run_id FROM coding_slot WHERE id=1').get()?.run_id, null, 'The paused, merged pilot run does not hold the coding slot.');
  assert.deepEqual(migrated.prepare('SELECT id, data FROM attempts ORDER BY id').all().map(row => [row.id, row.data]), before.attempts, 'Attempts and ownership are unchanged.');
  assert.deepEqual(migrated.prepare('SELECT id, data FROM assignments ORDER BY id').all().map(row => [row.id, row.data]), before.assignments, 'Automatic cycle counters are unchanged.');
  assert.deepEqual(migrated.prepare('SELECT run_id, data FROM coding ORDER BY run_id').all().map(row => [row.run_id, row.data]), before.coding, 'Candidates, checks and check policy are unchanged.');
  assert.equal(count(migrated, "SELECT count(*) FROM agent_events WHERE json_extract(data,'$.type') <> 'message_update'"), before.receipts, 'Every non-progress event (receipts, tool results, replies) is kept.');
  const mismatched = count(migrated, "SELECT count(*) FROM runs WHERE status IS NOT json_extract(data,'$.status') OR desired_state IS NOT json_extract(data,'$.desiredState')")
    + count(migrated, "SELECT count(*) FROM attempts WHERE state IS NOT json_extract(data,'$.state') OR ownership IS NOT json_extract(data,'$.ownership')");
  assert.equal(mismatched, 0, 'Indexed lifecycle columns match the retained records.');
  const progress = count(migrated, "SELECT count(*) FROM agent_events WHERE json_extract(data,'$.type') = 'message_update'");
  migrated.close();
  // Reopening an already-migrated copy is a no-op.
  new Workbench(copy).close();
  console.log(JSON.stringify({ source, fromVersion: before.version, toVersion: 21, queuedPlans: before.queuedIds.length, runs: before.runs.length, attempts: before.attempts.length,
    ticketsMigrated: before.tickets.length, receiptsKept: before.receipts, progressFramesBefore: before.progress, progressFramesAfter: progress, passed: true }, null, 2));
} finally { rmSync(root, { recursive: true, force: true }); }
