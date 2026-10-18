-- Schema 26 exported from the accepted TypeScript service. No user data.
CREATE TABLE tickets (
        id TEXT PRIMARY KEY, source_site TEXT, source_id TEXT,
        imported TEXT NOT NULL, refinement TEXT NOT NULL,
        UNIQUE(source_site, source_id)
      );

CREATE TABLE repositories (id TEXT PRIMARY KEY, data TEXT NOT NULL);

CREATE TABLE runs (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL, status TEXT GENERATED ALWAYS AS (json_extract(data,'$.status')) VIRTUAL, desired_state TEXT GENERATED ALWAYS AS (json_extract(data,'$.desiredState')) VIRTUAL);

CREATE TABLE notes (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);

CREATE TABLE steering (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), request_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id, request_id));

CREATE TABLE assignments (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), name TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id, name));

CREATE TABLE attempts (id TEXT PRIMARY KEY, assignment_id TEXT NOT NULL REFERENCES assignments(id), data TEXT NOT NULL, state TEXT GENERATED ALWAYS AS (json_extract(data,'$.state')) VIRTUAL, ownership TEXT GENERATED ALWAYS AS (json_extract(data,'$.ownership')) VIRTUAL);

CREATE TABLE peer_messages (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));

CREATE TABLE observer_cursors (assignment_id TEXT PRIMARY KEY REFERENCES assignments(id), cursor INTEGER NOT NULL);

CREATE TABLE observations (assignment_id TEXT NOT NULL REFERENCES assignments(id), cursor INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(assignment_id,cursor));

CREATE TABLE coding (run_id TEXT PRIMARY KEY REFERENCES runs(id), data TEXT NOT NULL);

CREATE TABLE jira_sync (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);

CREATE TABLE jira_updates (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), event_key TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id,event_key));

CREATE TABLE publications (run_id TEXT PRIMARY KEY REFERENCES runs(id), data TEXT NOT NULL);

CREATE TABLE delivery_notices (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), data TEXT NOT NULL);

CREATE TABLE "agent_events" (cursor INTEGER PRIMARY KEY AUTOINCREMENT, attempt_id TEXT NOT NULL REFERENCES attempts(id), sequence INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,sequence));

CREATE TABLE learning_proposals (id TEXT PRIMARY KEY, run_id TEXT REFERENCES runs(id), attempt_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id, tool_call_id));

CREATE TABLE requirement_revisions (
        task_id TEXT NOT NULL REFERENCES tickets(id), revision INTEGER NOT NULL, data TEXT NOT NULL,
        PRIMARY KEY(task_id, revision)
      );

CREATE TABLE refinement_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id),
          attempt_id TEXT NOT NULL REFERENCES attempts(id), tool_call_id TEXT NOT NULL,
          data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));

CREATE TABLE research_artifacts (
          id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT NOT NULL REFERENCES attempts(id),
          tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id, tool_call_id)
        );

CREATE TABLE queue_state (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);

CREATE TABLE queue_requests (request_id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL, result TEXT NOT NULL);

CREATE TABLE coding_slot (id INTEGER PRIMARY KEY CHECK(id=1), run_id TEXT REFERENCES runs(id));

CREATE TABLE team_proposals (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), attempt_id TEXT REFERENCES attempts(id), tool_call_id TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,tool_call_id));

CREATE TABLE repository_work (run_id TEXT PRIMARY KEY REFERENCES runs(id), common_dir TEXT NOT NULL, branch TEXT NOT NULL, data TEXT NOT NULL);

CREATE INDEX runs_lifecycle ON runs(status, desired_state);

CREATE INDEX attempts_lifecycle ON attempts(state, ownership);

CREATE INDEX agent_event_types ON agent_events(attempt_id, json_extract(data,'$.type'));

CREATE UNIQUE INDEX one_open_run_per_ticket ON runs(task_id) WHERE status NOT IN ('cancelled','closed','completed');

CREATE UNIQUE INDEX operator_proposal_request ON team_proposals(run_id,tool_call_id) WHERE attempt_id IS NULL;

CREATE UNIQUE INDEX current_branch_owner ON repository_work(common_dir,branch) WHERE json_extract(data,'$.transferredTo') IS NULL;
INSERT INTO queue_state VALUES(1,0);
INSERT INTO coding_slot VALUES(1,NULL);
CREATE TABLE plan_sources (task_id TEXT PRIMARY KEY REFERENCES tickets(id), data TEXT NOT NULL);
CREATE TABLE implementation_plans (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
CREATE TABLE plan_decisions (plan_id TEXT PRIMARY KEY REFERENCES implementation_plans(id), data TEXT NOT NULL);
CREATE TABLE intake_requests (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tickets(id), data TEXT NOT NULL);
CREATE UNIQUE INDEX one_active_intake ON intake_requests(task_id) WHERE json_extract(data,'$.state') NOT IN ('cancelled','handed_off');
PRAGMA user_version=33;

CREATE TABLE choreography_revisions (run_id TEXT NOT NULL REFERENCES runs(id), revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(run_id,revision));

CREATE TABLE comparisons (id TEXT PRIMARY KEY, request_id TEXT NOT NULL UNIQUE, manifest TEXT NOT NULL, manifest_hash TEXT NOT NULL, data TEXT NOT NULL);
CREATE TABLE comparison_cells (id TEXT PRIMARY KEY, comparison_id TEXT NOT NULL REFERENCES comparisons(id), case_id TEXT NOT NULL, repetition INTEGER NOT NULL, variant TEXT NOT NULL, ordinal INTEGER NOT NULL, run_id TEXT UNIQUE REFERENCES runs(id), data TEXT NOT NULL, UNIQUE(comparison_id,case_id,repetition,variant), UNIQUE(comparison_id,ordinal));
CREATE TABLE comparison_events (id TEXT PRIMARY KEY, comparison_id TEXT NOT NULL REFERENCES comparisons(id), cell_id TEXT REFERENCES comparison_cells(id), data TEXT NOT NULL);
CREATE TABLE comparison_requests (request_id TEXT PRIMARY KEY, operation TEXT NOT NULL, payload TEXT NOT NULL, result TEXT NOT NULL);

CREATE TABLE github_sync (id INTEGER PRIMARY KEY CHECK(id=1), data TEXT NOT NULL);

CREATE TABLE github_updates (id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), event_key TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(run_id,event_key));
