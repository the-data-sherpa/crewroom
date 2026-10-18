-- Schema-30 store written by the pre-FOR-40 daemon (d08fc59) through its HTTP API:
-- two imported Jira tickets and two local tickets with operator refinements, notes and
-- requirement revisions. Synthetic content only; produced by scripts/verify_source_migration_restore.py.
-- Tables are dumped alphabetically, so foreign keys are checked only after loading.
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE "agent_events" (cursor INTEGER PRIMARY KEY AUTOINCREMENT, attempt_id TEXT NOT NULL REFERENCES attempts(id), sequence INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,sequence));
CREATE TABLE assignments (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), name TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id, name));
CREATE TABLE attempts (id TEXT PRIMARY KEY, assignment_id TEXT NOT NULL REFERENCES assignments(id), data TEXT NOT NULL, state TEXT GENERATED ALWAYS AS (json_extract(data,'$.state')) VIRTUAL, ownership TEXT GENERATED ALWAYS AS (json_extract(data,'$.ownership')) VIRTUAL);
CREATE TABLE choreography_revisions (run_id TEXT NOT NULL REFERENCES runs(id), revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(run_id,revision));
CREATE TABLE coding (run_id TEXT PRIMARY KEY REFERENCES runs(id), data TEXT NOT NULL);
CREATE TABLE coding_slot (id INTEGER PRIMARY KEY CHECK(id=1), run_id TEXT REFERENCES runs(id));
INSERT INTO "coding_slot" VALUES(1,NULL);
CREATE TABLE comparison_cells (id TEXT PRIMARY KEY, comparison_id TEXT NOT NULL REFERENCES comparisons(id), case_id TEXT NOT NULL, repetition INTEGER NOT NULL, variant TEXT NOT NULL, ordinal INTEGER NOT NULL, run_id TEXT UNIQUE REFERENCES runs(id), data TEXT NOT NULL, UNIQUE(comparison_id,case_id,repetition,variant), UNIQUE(comparison_id,ordinal));
CREATE TABLE comparison_events (id TEXT PRIMARY KEY, comparison_id TEXT NOT NULL REFERENCES comparisons(id), cell_id TEXT REFERENCES comparison_cells(id), data TEXT NOT NULL);
CREATE TABLE comparison_requests (request_id TEXT PRIMARY KEY, operation TEXT NOT NULL, payload TEXT NOT NULL, result TEXT NOT NULL);
CREATE TABLE comparisons (id TEXT PRIMARY KEY, request_id TEXT NOT NULL UNIQUE, manifest TEXT NOT NULL, manifest_hash TEXT NOT NULL, data TEXT NOT NULL);
CREATE TABLE delivery_notices (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), data TEXT NOT NULL);
CREATE TABLE implementation_plans (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
CREATE TABLE intake_requests (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
CREATE TABLE jira_sync (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);
CREATE TABLE jira_updates (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), event_key TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id,event_key));
CREATE TABLE learning_proposals (id TEXT PRIMARY KEY, run_id TEXT REFERENCES runs(id), attempt_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id, tool_call_id));
CREATE TABLE notes (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
INSERT INTO "notes" VALUES('83c1525a-d13b-446e-a1c3-876d98eee06e','5bf022e8-03e1-47da-9496-dbcfe88b29f6','{"id":"83c1525a-d13b-446e-a1c3-876d98eee06e","taskId":"5bf022e8-03e1-47da-9496-dbcfe88b29f6","mode":"decision","body":"Zero is a valid input.","createdAt":"2026-10-18T10:54:49.203Z"}');
INSERT INTO "notes" VALUES('60279d33-e5b4-404e-9f08-708f63fc4a3b','ba9bd816-c171-46cb-9496-efb82aa859d6','{"id":"60279d33-e5b4-404e-9f08-708f63fc4a3b","taskId":"ba9bd816-c171-46cb-9496-efb82aa859d6","mode":"explore","body":"No provider for this ticket.","createdAt":"2026-10-18T10:54:49.203Z"}');
CREATE TABLE observations (assignment_id TEXT NOT NULL REFERENCES assignments(id), cursor INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(assignment_id,cursor));
CREATE TABLE observer_cursors (assignment_id TEXT PRIMARY KEY REFERENCES assignments(id), cursor INTEGER NOT NULL);
CREATE TABLE peer_messages (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));
CREATE TABLE plan_decisions (plan_id TEXT PRIMARY KEY REFERENCES implementation_plans(id), data TEXT NOT NULL);
CREATE TABLE plan_sources (task_id TEXT PRIMARY KEY REFERENCES tickets(id), data TEXT NOT NULL);
CREATE TABLE publications (run_id TEXT PRIMARY KEY REFERENCES runs(id), data TEXT NOT NULL);
CREATE TABLE queue_requests (request_id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL, result TEXT NOT NULL);
CREATE TABLE queue_state (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
INSERT INTO "queue_state" VALUES(1,0);
CREATE TABLE refinement_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id),
          attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL,
          data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));
CREATE TABLE repositories (id TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE repository_work (run_id TEXT PRIMARY KEY REFERENCES runs(id), common_dir TEXT NOT NULL, branch TEXT NOT NULL, data TEXT NOT NULL);
CREATE TABLE requirement_revisions (
        task_id TEXT NOT NULL REFERENCES tickets(id), revision INTEGER NOT NULL, data TEXT NOT NULL,
        PRIMARY KEY(task_id, revision)
      );
INSERT INTO "requirement_revisions" VALUES('5bf022e8-03e1-47da-9496-dbcfe88b29f6',2,'{"taskId":"5bf022e8-03e1-47da-9496-dbcfe88b29f6","revision":2,"brief":"Handle zero","criteria":["Handle zero is verified"],"sourceRevision":1,"origin":"operator","createdAt":"2026-10-18T10:54:49.200Z"}');
INSERT INTO "requirement_revisions" VALUES('908231e2-84f4-4552-8dc1-0d584916aa44',2,'{"taskId":"908231e2-84f4-4552-8dc1-0d584916aa44","revision":2,"brief":"List every total","criteria":["List every total is verified"],"sourceRevision":1,"origin":"operator","createdAt":"2026-10-18T10:54:49.201Z"}');
INSERT INTO "requirement_revisions" VALUES('ba9bd816-c171-46cb-9496-efb82aa859d6',2,'{"taskId":"ba9bd816-c171-46cb-9496-efb82aa859d6","revision":2,"brief":"Keep it local","criteria":["Keep it local is verified"],"sourceRevision":1,"origin":"operator","createdAt":"2026-10-18T10:54:49.202Z"}');
CREATE TABLE research_artifacts (
          id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id),
          tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id, tool_call_id)
        );
CREATE TABLE runs (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL, status TEXT GENERATED ALWAYS AS (json_extract(data,'$.status')) VIRTUAL, desired_state TEXT GENERATED ALWAYS AS (json_extract(data,'$.desiredState')) VIRTUAL);
CREATE TABLE steering (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), request_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id, request_id));
CREATE TABLE team_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));
CREATE TABLE tickets (
        id TEXT PRIMARY KEY, source_site TEXT, source_id TEXT,
        imported TEXT NOT NULL, refinement TEXT NOT NULL,
        UNIQUE(source_site, source_id)
      );
INSERT INTO "tickets" VALUES('5bf022e8-03e1-47da-9496-dbcfe88b29f6','https://fixture.atlassian.net','100','{"key":"SIM-100","title":"Fix sum","description":"Synthetic description 100","jiraStatus":"In Progress","priority":"High","project":"Simulated","rank":0,"jiraStatusId":"3","sourceUpdatedAt":"2026-10-01T00:00:00.000Z","id":"5bf022e8-03e1-47da-9496-dbcfe88b29f6","source":"jira","sourceSite":"https://fixture.atlassian.net","sourceId":"100","sourceUrl":"https://fixture.atlassian.net/browse/SIM-100","sourceRevision":1,"updatedAt":"2026-10-18T10:54:49.197Z"}','{"revision":2,"brief":"Handle zero","criteria":["Handle zero is verified"],"needsReacceptance":false,"acceptedSourceRevision":1}');
INSERT INTO "tickets" VALUES('908231e2-84f4-4552-8dc1-0d584916aa44','https://fixture.atlassian.net','101','{"key":"SIM-101","title":"Document totals","description":"Synthetic description 101","jiraStatus":"Backlog","priority":"High","project":"Simulated","rank":1,"jiraStatusId":"1","sourceUpdatedAt":"2026-10-01T00:00:00.000Z","id":"908231e2-84f4-4552-8dc1-0d584916aa44","source":"jira","sourceSite":"https://fixture.atlassian.net","sourceId":"101","sourceUrl":"https://fixture.atlassian.net/browse/SIM-101","sourceRevision":1,"updatedAt":"2026-10-18T10:54:49.197Z"}','{"revision":2,"brief":"List every total","criteria":["List every total is verified"],"needsReacceptance":false,"acceptedSourceRevision":1}');
INSERT INTO "tickets" VALUES('ba9bd816-c171-46cb-9496-efb82aa859d6',NULL,NULL,'{"id":"ba9bd816-c171-46cb-9496-efb82aa859d6","key":"LOCAL-ba9bd816","title":"Local work","description":"Operator-created ticket","jiraStatus":"Not linked","priority":"Unspecified","source":"local","project":"","rank":0,"sourceRevision":1,"updatedAt":"2026-10-18T10:54:49.199Z"}','{"revision":2,"brief":"Keep it local","criteria":["Keep it local is verified"],"needsReacceptance":false,"acceptedSourceRevision":1}');
INSERT INTO "tickets" VALUES('6223e046-ebec-4c5d-88a0-afcacaf6aeb1',NULL,NULL,'{"id":"6223e046-ebec-4c5d-88a0-afcacaf6aeb1","key":"LOCAL-6223e046","title":"Unrefined local","description":"","jiraStatus":"Not linked","priority":"Unspecified","source":"local","project":"","rank":0,"sourceRevision":1,"updatedAt":"2026-10-18T10:54:49.199Z"}','{"revision":1,"brief":"","criteria":[],"needsReacceptance":false}');
CREATE INDEX runs_lifecycle ON runs(status, desired_state);
CREATE INDEX attempts_lifecycle ON attempts(state, ownership);
CREATE INDEX agent_event_types ON agent_events(attempt_id, json_extract(data,'$.type'));
CREATE UNIQUE INDEX one_open_run_per_ticket ON runs(task_id) WHERE status NOT IN ('cancelled','closed','completed');
CREATE UNIQUE INDEX operator_proposal_request ON team_proposals(run_id,tool_call_id) WHERE attempt_id IS NULL;
CREATE UNIQUE INDEX current_branch_owner ON repository_work(common_dir,branch) WHERE json_extract(data,'$.transferredTo') IS NULL;
CREATE UNIQUE INDEX one_active_intake ON intake_requests(task_id) WHERE json_extract(data,'$.state') NOT IN ('cancelled','handed_off');
DELETE FROM "sqlite_sequence";
COMMIT;
PRAGMA user_version=30;
