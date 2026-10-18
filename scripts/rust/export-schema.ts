import { Workbench } from '../../server/workbench';
import { DatabaseSync } from 'node:sqlite';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const root = mkdtempSync(join(tmpdir(), 'crewroom-schema-'));
try {
  const path = join(root, 'workflow.sqlite');
  new Workbench(path).close();
  const db = new DatabaseSync(path);
  const sql = db.prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, rowid").all().map(row => `${row.sql};`).join('\n\n');
  writeFileSync('crates/crewroom-core/src/schema.sql', '-- Schema 26 exported from the accepted TypeScript service. No user data.\n' + sql + '\nINSERT INTO queue_state VALUES(1,0);\nINSERT INTO coding_slot VALUES(1,NULL);\nPRAGMA user_version=26;\n');
  db.close();
} finally { rmSync(root, { recursive: true, force: true }); }
