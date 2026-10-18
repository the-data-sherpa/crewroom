//! Ordered upgrades of retained Crewroom databases. Each version is atomic; no
//! migration starts a process or treats a legacy process receipt as stopped.
use crate::{
    Result, compact_observation,
    lifecycle::Transition,
    store::{Store, conflict, id, parse, remove, trim_text},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::HashSet;
mod proof;

impl Store {
    pub(crate) fn migrate(&self, from: u32) -> Result<()> {
        let schema: Value = serde_json::from_str(include_str!("migrations/schema.json"))?;
        for version in from + 1..=crate::store::SCHEMA_VERSION {
            self.transaction(|s| {
                if let Some(sql) = schema[version.to_string()].as_str() {
                    s.db.execute_batch(sql)?;
                }
                match version {
                    14 => s.migrate_observations()?,
                    15 => s.migrate_cursors()?,
                    17 => s.migrate_requirements()?,
                    20 => s.migrate_queue()?,
                    21 => s.migrate_slot()?,
                    22 => s.migrate_retries()?,
                    23 => s.migrate_ownership()?,
                    24 => s.migrate_steering()?,
                    31 => s.migrate_ticket_sources()?,
                    34 => s.migrate_github_project_sources()?,
                    _ => (),
                }
                s.db.pragma_update(None, "user_version", version)?;
                Ok(())
            })?;
        }
        Ok(())
    }
    fn migrate_observations(&self) -> Result<()> {
        let mut q = self.db.prepare("SELECT attempt_id,sequence,data FROM agent_events WHERE json_extract(data,'$.type')='message_update'")?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (attempt, sequence, data) in rows {
            self.db.execute(
                "UPDATE agent_events SET data=? WHERE attempt_id=? AND sequence=?",
                params![
                    compact_observation(&parse(data)?).to_string(),
                    attempt,
                    sequence
                ],
            )?;
        }
        self.db.execute_batch("CREATE INDEX IF NOT EXISTS agent_event_types ON agent_events(attempt_id, json_extract(data,'$.type'));")?;
        Ok(())
    }
    fn migrate_cursors(&self) -> Result<()> {
        let columns = |table: &str| -> Result<HashSet<String>> {
            Ok(self
                .db
                .prepare(&format!("PRAGMA table_xinfo({table})"))?
                .query_map([], |r| r.get(1))?
                .collect::<std::result::Result<_, _>>()?)
        };
        for (table, column, path) in [
            ("runs", "status", "status"),
            ("runs", "desired_state", "desiredState"),
            ("attempts", "state", "state"),
            ("attempts", "ownership", "ownership"),
        ] {
            if !columns(table)?.contains(column) {
                self.db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT GENERATED ALWAYS AS (json_extract(data,'$.{path}')) VIRTUAL"))?;
            }
        }
        self.db.execute_batch("CREATE INDEX IF NOT EXISTS runs_lifecycle ON runs(status, desired_state); CREATE INDEX IF NOT EXISTS attempts_lifecycle ON attempts(state, ownership);")?;
        if !columns("agent_events")?.contains("cursor") {
            self.db.execute_batch("CREATE TABLE agent_events_v15 (cursor INTEGER PRIMARY KEY AUTOINCREMENT, attempt_id TEXT NOT NULL REFERENCES attempts(id), sequence INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(attempt_id,sequence)); INSERT INTO agent_events_v15(cursor,attempt_id,sequence,data) SELECT rowid,attempt_id,sequence,data FROM agent_events ORDER BY rowid; DROP TABLE agent_events; ALTER TABLE agent_events_v15 RENAME TO agent_events;")?;
        }
        self.db.execute_batch("CREATE INDEX IF NOT EXISTS agent_event_types ON agent_events(attempt_id, json_extract(data,'$.type'));")?;
        let mut q=self.db.prepare("SELECT attempt_id,sequence,data FROM agent_events WHERE json_extract(data,'$.type')='message_end' ORDER BY cursor")?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (attempt, sequence, data) in rows {
            self.compact_completed_message(&attempt, sequence as u64, &parse(data)?)?;
        }
        Ok(())
    }
    fn migrate_requirements(&self) -> Result<()> {
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS requirement_revisions (task_id TEXT NOT NULL REFERENCES tickets(id), revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(task_id, revision));")?;
        let mut q = self
            .db
            .prepare("SELECT id,imported,refinement FROM tickets ORDER BY rowid")?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (task, source, refinement) in rows {
            let source = parse(source)?;
            let refinement = parse(refinement)?;
            let mut accepted = refinement
                .get("acceptedSourceRevision")
                .filter(|v| !v.is_null())
                .cloned();
            if accepted.is_none() {
                accepted = if refinement["needsReacceptance"] != true {
                    source.get("sourceRevision").cloned()
                } else {
                    self.db
                        .query_row(
                            "SELECT data FROM runs WHERE task_id=? ORDER BY rowid DESC LIMIT 1",
                            [&task],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?
                        .map(parse)
                        .transpose()?
                        .and_then(|run| {
                            run["taskSnapshot"]
                                .get("sourceRevision")
                                .filter(|v| !v.is_null())
                                .cloned()
                        })
                };
            }
            let accepted = accepted.unwrap_or_else(|| {
                json!((source["sourceRevision"].as_i64().unwrap_or(1) - 1).max(1))
            });
            let mut record = json!({"taskId":task,"revision":refinement["revision"],"brief":refinement["brief"],"criteria":refinement["criteria"],"sourceRevision":accepted,"origin":"operator"});
            if let Some(created) = source.get("updatedAt") {
                record["createdAt"] = created.clone();
            }
            self.db.execute(
                "INSERT OR IGNORE INTO requirement_revisions VALUES(?,?,?)",
                params![task, refinement["revision"].as_i64(), record.to_string()],
            )?;
        }
        Ok(())
    }
    fn migrate_queue(&self) -> Result<()> {
        let migrated: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='queue_state')",
            [],
            |r| r.get(0),
        )?;
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS queue_state (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL); INSERT OR IGNORE INTO queue_state VALUES(1,0); CREATE TABLE IF NOT EXISTS queue_requests (request_id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL, result TEXT NOT NULL);")?;
        if !migrated {
            let mut runs = self.all("runs")?;
            let mut queued = Vec::new();
            for (order, run) in runs.iter().enumerate() {
                if run["purpose"].as_str().unwrap_or("coding") == "coding"
                    && run["status"] == "awaiting_runner"
                    && !self.db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM assignments WHERE run_id=?)",
                        [run["id"].as_str()],
                        |r| r.get::<_, bool>(0),
                    )?
                {
                    queued.push(order);
                }
            }
            queued.sort_by(|a, b| {
                runs[*a]["createdAt"]
                    .as_str()
                    .cmp(&runs[*b]["createdAt"].as_str())
                    .then(a.cmp(b))
            });
            for (order, run) in runs.iter_mut().enumerate() {
                remove(run, "position");
                if let Some(position) = queued.iter().position(|i| *i == order) {
                    run["position"] = json!(position);
                }
                self.save_migrated_run(run)?;
            }
        }
        Ok(())
    }
    fn migrate_slot(&self) -> Result<()> {
        self.db.execute_batch("CREATE TABLE IF NOT EXISTS coding_slot (id INTEGER PRIMARY KEY CHECK(id=1), run_id TEXT REFERENCES runs(id)); INSERT OR IGNORE INTO coding_slot(id,run_id) VALUES(1,NULL);")?;
        let mut q=self.db.prepare("SELECT r.data FROM runs r WHERE COALESCE(json_extract(r.data,'$.purpose'),'coding')='coding' AND r.status NOT IN ('cancelled','closed','completed') AND json_extract(r.data,'$.position') IS NULL AND (EXISTS (SELECT 1 FROM assignments a WHERE a.run_id=r.id) OR EXISTS (SELECT 1 FROM repository_work w WHERE w.run_id=r.id)) ORDER BY r.rowid")?;
        let mut candidates = q
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| parse(r?))
            .collect::<Result<Vec<_>>>()?;
        let mut unmerged = Vec::new();
        for run in candidates.drain(..) {
            if self
                .publication(run["id"].as_str().unwrap())?
                .is_none_or(|p| !crate::repository_work::truthy(&p["merged"]))
            {
                unmerged.push(run);
            }
        }
        let mut claimed = HashSet::new();
        for run in &unmerged {
            if self.db.query_row("SELECT EXISTS(SELECT 1 FROM attempts a JOIN assignments s ON s.id=a.assignment_id WHERE s.run_id=? AND a.state<>'admitted' AND a.ownership IS NOT 'stopped')",[run["id"].as_str()],|r|r.get::<_,bool>(0))? {claimed.insert(run["id"].clone());}
        }
        let holder = if unmerged.len() == 1 {
            unmerged.first()
        } else {
            unmerged.iter().find(|r| claimed.contains(&r["id"]))
        }
        .map(|r| r["id"].clone());
        for mut run in unmerged {
            if holder.as_ref() == Some(&run["id"]) {
                if run["schedule"].is_null() {
                    run["schedule"] = json!({"id":id(),"at":run["createdAt"]});
                }
                if run["launchCompletedAt"].is_null() {
                    run["launchCompletedAt"] = run["createdAt"].clone();
                }
                self.db.execute(
                    "UPDATE coding_slot SET run_id=? WHERE id=1",
                    [run["id"].as_str()],
                )?;
            } else {
                if run["slotRelease"].is_null() {
                    run["slotRelease"] = json!({"reason":"migration","at":self.time()});
                }
                if claimed.contains(&run["id"]) {
                    run["status"] = json!("attention_required");
                    run["desiredState"] = json!("paused");
                    run["attentionReason"] = json!(
                        "Another retained coding run owns the slot. Reconcile this run’s process ownership before resuming."
                    );
                } else if ["running", "awaiting_runner"]
                    .contains(&run["status"].as_str().unwrap_or(""))
                {
                    run["status"] = json!("paused");
                    run["desiredState"] = json!("paused");
                    run["pauseReason"] = json!("recovery");
                    remove(&mut run, "activeSince");
                }
            }
            self.save_migrated_run(&run)?;
        }
        Ok(())
    }
    fn migrate_retries(&self) -> Result<()> {
        for mut work in self.all("repository_work")? {
            let Some(refreshes) = work["refreshes"].as_array_mut() else {
                continue;
            };
            let interrupted: Vec<usize> = refreshes
                .iter()
                .enumerate()
                .filter(|(_, r)| r["state"] == "interrupted" && !r["supersedes"].is_null())
                .map(|(i, _)| i)
                .collect();
            if interrupted.is_empty() {
                continue;
            }
            if refreshes
                .iter()
                .map(|r| r["requestId"].as_str())
                .collect::<HashSet<_>>()
                .len()
                != refreshes.len()
            {
                return Err(conflict(
                    "Interrupted retry history has duplicate request identities.",
                ));
            }
            for index in &interrupted {
                let retry = &refreshes[*index];
                let prior = refreshes
                    .iter()
                    .find(|r| r["requestId"] == retry["supersedes"]["requestId"]);
                let next = refreshes
                    .iter()
                    .find(|r| r["requestId"] == retry["supersededBy"]["requestId"]);
                let valid_prior = prior.is_some_and(|p| {
                    p["requestId"] != retry["requestId"]
                        && p["state"] == "conflicted"
                        && p["supersededBy"]["requestId"] == retry["requestId"]
                        && p["supersededBy"]["reason"] == retry["supersedes"]["reason"]
                        && p["supersededBy"]["at"] == retry["at"]
                });
                let valid_next = retry["supersededBy"].is_null()
                    || next.is_some_and(|n| {
                        n["requestId"] != retry["requestId"]
                            && n["supersedes"]["requestId"] == retry["requestId"]
                            && n["supersedes"]["reason"] == retry["supersededBy"]["reason"]
                            && n["at"] == retry["supersededBy"]["at"]
                    });
                if !valid_prior
                    || !valid_next
                    || trim_text(retry["supersedes"]["reason"].as_str().unwrap_or("")).is_empty()
                    || ["resolutionWorkspace", "candidateId", "toHead"]
                        .iter()
                        .any(|k| !retry[*k].is_null())
                    || retry["conflicts"].as_array().is_some_and(|a| !a.is_empty())
                {
                    return Err(conflict(
                        "Interrupted retry history is ambiguous; recover its persisted links and workspace before opening the workbench.",
                    ));
                }
                refreshes[*index]["state"] = json!("conflicted");
                refreshes[*index]["holdReason"] = json!("interrupted_retry");
                refreshes[*index]["conflicts"] = json!([]);
            }
            if refreshes
                .iter()
                .filter(|r| r["state"] == "conflicted" && r["supersededBy"].is_null())
                .count()
                > 1
            {
                return Err(conflict(
                    "Multiple active target holds need explicit recovery before opening the workbench.",
                ));
            }
            let active = interrupted
                .iter()
                .any(|i| refreshes[*i]["supersededBy"].is_null());
            self.save_migrated_work(&work)?;
            if active {
                let run = self.get_run(work["runId"].as_str().unwrap())?;
                if !terminal(&run)
                    && (run["desiredState"] != "paused"
                        || !["paused", "attention_required"]
                            .contains(&run["status"].as_str().unwrap_or("")))
                {
                    self.migration_hold(&run,"An interrupted target retry requires an operator decision; reconcile process ownership before resuming.")?;
                }
            }
        }
        Ok(())
    }
    fn migrate_ownership(&self) -> Result<()> {
        for mut work in self.all("repository_work")? {
            let record = self
                .coding_record(work["runId"].as_str().unwrap())?
                .unwrap_or(Value::Null);
            let original = work.clone();
            let Some(refreshes) = work["refreshes"].as_array_mut() else {
                continue;
            };
            let mut changed = false;
            for refresh in refreshes.iter_mut() {
                let state = &refresh["resolutionWorkspace"]["state"];
                if refresh["state"] == "conflicted"
                    && (state == "conflicted"
                        || state == "operator_required"
                            && !proof::checked_terminal(&original, &record, refresh))
                {
                    refresh["resolutionWorkspace"]["state"] = json!("continuing");
                    refresh["resolutionWorkspace"]["legacyOwnershipUnknown"] = json!(true);
                    changed = true;
                }
            }
            let unknown = refreshes.iter().any(|r| {
                r["state"] == "rebasing"
                    || r["state"] == "conflicted"
                        && ["preparing", "continuing"]
                            .contains(&r["resolutionWorkspace"]["state"].as_str().unwrap_or(""))
            });
            if changed {
                self.save_migrated_work(&work)?;
            }
            if unknown {
                let run = self.get_run(work["runId"].as_str().unwrap())?;
                if !terminal(&run) {
                    self.migration_hold(&run,"Git mutation ownership from an older workbench is unverified; retain source and workspace for operator inspection.")?;
                }
            }
        }
        Ok(())
    }
    fn save_migrated_run(&self, run: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE runs SET data=? WHERE id=?",
            params![run.to_string(), run["id"].as_str()],
        )?;
        Ok(())
    }
    fn save_migrated_work(&self, work: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE repository_work SET data=? WHERE run_id=?",
            params![work.to_string(), work["runId"].as_str()],
        )?;
        Ok(())
    }
    fn migration_hold(&self, run: &Value, reason: &str) -> Result<()> {
        let id = run["id"].as_str().unwrap();
        if run["desiredState"] != "paused" {
            self.change_run(
                id,
                Transition::Desire {
                    desired_state: "paused".into(),
                },
            )?;
        }
        self.change_run(
            id,
            Transition::Attention {
                reason: reason.into(),
            },
        )?;
        Ok(())
    }
    fn migrate_steering(&self) -> Result<()> {
        for run in self.all("runs")? {
            let Some(reason) = crate::messages::undelivered_reason(&run) else {
                continue;
            };
            for mut message in self.steering(run["id"].as_str().unwrap())? {
                if message["delivery"] != "pending" {
                    continue;
                }
                let mut transitions = message["transitions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                transitions.push(json!({"state":"not_delivered","reason":reason}));
                message["transitions"] = json!(transitions);
                message["delivery"] = json!("not_delivered");
                message["dispositionReason"] = json!(reason);
                self.db.execute(
                    "UPDATE steering SET data=? WHERE id=?",
                    params![message.to_string(), message["id"].as_str()],
                )?;
            }
        }
        Ok(())
    }
    /// Version 31 (FOR-40): rename the Jira-named source status to the
    /// provider-neutral `sourceStatus`/`sourceStatusId`, keeping key order.
    /// The "Not linked" placeholder on a local ticket was never a provider fact
    /// and is derived from the source kind, so it is dropped. Any other value is
    /// carried over unchanged. Historical run snapshots and message bodies are
    /// immutable records and are left as written.
    fn migrate_ticket_sources(&self) -> Result<()> {
        let rows = self
            .db
            .prepare("SELECT id,imported FROM tickets ORDER BY rowid")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (ticket, imported) in rows {
            let Value::Object(fields) = parse(imported)? else {
                return Err(conflict("Ticket source record is not an object."));
            };
            if !fields.contains_key("jiraStatus") && !fields.contains_key("jiraStatusId") {
                continue;
            }
            if fields.contains_key("sourceStatus") || fields.contains_key("sourceStatusId") {
                return Err(conflict(
                    "Ticket has both Jira-named and neutral source status fields.",
                ));
            }
            let external = crate::ticket_source::provider(&Value::Object(fields.clone())).is_some();
            let mut neutral = serde_json::Map::new();
            for (key, value) in fields {
                match key.as_str() {
                    "jiraStatus" if !external && value == crate::ticket_source::NOT_LINKED => {}
                    "jiraStatus" => {
                        neutral.insert("sourceStatus".into(), value);
                    }
                    "jiraStatusId" => {
                        neutral.insert("sourceStatusId".into(), value);
                    }
                    _ => {
                        neutral.insert(key, value);
                    }
                }
            }
            self.db.execute(
                "UPDATE tickets SET imported=? WHERE id=?",
                params![Value::Object(neutral).to_string(), ticket],
            )?;
        }
        Ok(())
    }
}
fn terminal(run: &Value) -> bool {
    ["cancelled", "closed", "completed"].contains(&run["status"].as_str().unwrap_or(""))
        || run["desiredState"] == "cancelled"
}
